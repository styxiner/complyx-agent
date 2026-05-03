//! Loop principal del agente: poll → checks → remediación → envío de resultados.
//!
//! Este módulo implementa el ciclo completo de un tick del agente:
//!
//! ```text
//! 1. PollPolicies(hash_actual) → servidor
//! 2. Si policies_changed:
//!    a. Guardar bundle en local-db
//!    b. Ejecutar todos los checks (policy-engine)
//!    c. Para cada check fallido con remediación configurada:
//!       - Si auto_remediate = true → aplicar remediación (remediation-engine)
//!    d. Encolar todos los resultados en local-db
//! 3. Intentar flush inmediato de la cola al servidor
//! ```
//!
//! Si el servidor no está disponible en el paso 3, los resultados permanecen
//! en la cola SQLite y el `result_flush` del scheduler los enviará más tarde.

use std::sync::Arc;

use grpc_client::GrpcClient;
use local_db::LocalDb;
use policy_engine::PolicyEngine;
use proto::PolicyBundle;
use remediation_engine::RemediationEngine;

use crate::config::AgentConfig;

// Ejecuta un tick completo del poll loop.
//
// Diseñado para llamarse desde `scheduler.rs` en cada intervalo de poll.
// Nunca devuelve `Err` — los fallos parciales (servidor caído, check con error)
// se registran en el log y en la base de datos local, pero no interrumpen
// el ciclo del agente.
pub async fn run_tick(config: Arc<AgentConfig>, client: Arc<GrpcClient>, db: Arc<LocalDb>, engine: Arc<PolicyEngine>, remediation: Arc<RemediationEngine>,) {
    // 1. Obtener el hash del bundle actual para el poll
    let current_hash = match db.get_bundle_hash().await {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(error = %e, "no se pudo leer el hash del bundle de la BD local");
            String::new()
        }
    };

    // 2. Poll al servidor
    let poll_response = match client.poll_policies(&current_hash).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "poll al servidor falló, usando bundle en caché");
            // Intentar usar el bundle cacheado si existe
            run_with_cached_bundle(&config, &db, &engine, &remediation).await;
            return;
        }
    };

    // 3. Si no hay cambios en las políticas, nada que hacer en este tick
    if !poll_response.policies_changed {
        tracing::debug!("sin cambios en políticas, tick completado");
        return;
    }

    // 4. Bundle nuevo recibido
    let bundle = match poll_response.bundle {
        Some(b) => b,
        None => {
            tracing::warn!("servidor indicó policies_changed pero no envió bundle");
            return;
        }
    };

    let total_checks = bundle.total_checks();
    tracing::info!(
        bundle_hash = %bundle.bundle_hash,
        policies = bundle.policies.len(),
        total_checks,
        "bundle de políticas actualizado"
    );

    // 5. Persistir el bundle en la caché local
    if let Err(e) = db.save_bundle(&bundle).await {
        tracing::error!(error = %e, "no se pudo guardar el bundle en la BD local");
        // Continuamos: podemos ejecutar con el bundle recibido aunque no se haya persistido
    }

    // 6. Ejecutar checks y gestionar resultados
    process_bundle(&config, &bundle, &db, &engine, &remediation).await;

    // 7. Flush inmediato: intentar enviar los resultados recién generados
    flush_results(&client, &db).await;
}

// Ejecuta los checks del bundle y, si procede, aplica remediaciones.
async fn process_bundle(config: &AgentConfig, bundle: &PolicyBundle, db: &LocalDb, engine: &PolicyEngine, remediation: &RemediationEngine,) {
    // Ejecutar todos los checks
    let results = engine.run_all(bundle).await;

    let passed = results.iter().filter(|r| r.passed).count();
    let failed = results.iter().filter(|r| !r.passed).count();

    tracing::info!(
        passed,
        failed,
        total = results.len(),
        "ejecución de checks completada"
    );

    // Aplicar remediaciones para los checks fallidos (si auto_remediate está activo)
    if config.auto_remediate && failed > 0 {
        apply_remediations(config, bundle, &results, remediation).await;
    }

    // Encolar todos los resultados en la BD local
    if let Err(e) = db.enqueue_results(&results).await {
        tracing::error!(error = %e, "no se pudieron encolar los resultados");
    }
}

// Aplica remediaciones para los checks fallidos que tengan una remediación configurada.
async fn apply_remediations(_config: &AgentConfig, bundle: &PolicyBundle, results: &[proto::CheckResult], remediation: &RemediationEngine,) {
    // Construir mapa check_id → PolicyRemediation para acceso O(1)
    let rem_map: std::collections::HashMap<&str, &proto::PolicyRemediation> = bundle
        .policies
        .iter()
        .flat_map(|p| &p.elements)
        .flat_map(|e| &e.checks)
        .filter_map(|c| c.remediation.as_ref().map(|r| (c.id.as_str(), r)))
        .filter(|(_, r)| r.is_configured())
        .collect();

    let failed_with_rem: Vec<_> = results
        .iter()
        .filter(|r| !r.passed)
        .filter_map(|r| rem_map.get(r.check_id.as_str()).map(|rem| (&r.check_id, *rem)))
        .collect();

    if failed_with_rem.is_empty() {
        tracing::debug!("no hay remediaciones automáticas configuradas para los checks fallidos");
        return;
    }

    tracing::info!(
        count = failed_with_rem.len(),
        "aplicando remediaciones automáticas"
    );

    for (check_id, rem) in failed_with_rem {
        match remediation.apply(check_id, rem).await {
            Ok(outcome) => {
                if outcome.applied {
                    tracing::info!(
                        check_id = %check_id,
                        remediation_type = %outcome.remediation_type,
                        detail = %outcome.detail,
                        "remediación aplicada"
                    );
                } else {
                    tracing::warn!(
                        check_id = %check_id,
                        remediation_type = %outcome.remediation_type,
                        detail = %outcome.detail,
                        "remediación omitida"
                    );
                }
            }
            Err(e) => {
                tracing::error!(
                    check_id = %check_id,
                    error = %e,
                    "error de infraestructura aplicando remediación"
                );
            }
        }
    }
}

// Intenta usar el bundle cacheado en la BD local cuando el servidor no está disponible. Permite
// que el agente siga ejecutando checks sin conexion
async fn run_with_cached_bundle(config: &AgentConfig, db: &LocalDb, engine: &PolicyEngine, remediation: &RemediationEngine,) {
    match db.load_bundle().await {
        Ok(Some(bundle)) => {
            tracing::info!(
                bundle_hash = %bundle.bundle_hash,
                "ejecutando checks con bundle en caché (modo offline)"
            );
            process_bundle(config, &bundle, db, engine, remediation).await;
        }
        Ok(None) => {
            tracing::warn!("sin bundle en caché y servidor no disponible, omitiendo tick");
        }
        Err(e) => {
            tracing::error!(error = %e, "no se pudo cargar el bundle de la caché");
        }
    }
}

// Drena la cola de resultados pendientes y los envía al servidor. Se llama tras cada poll exitoso
// y tambien desde el scheduler de forma periodica
pub async fn flush_results(client: &GrpcClient, db: &LocalDb) {
    const BATCH_SIZE: i64 = 100;

    let rows = match db.drain_pending(BATCH_SIZE).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "no se pudo leer la cola de resultados");
            return;
        }
    };

    if rows.is_empty() {
        return;
    }

    let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();

    let (results, failed_ids) = match db.deserialize_pending(rows).await {
        Ok(pair) => pair,
        Err(e) => {
            tracing::error!(error = %e, "error deserializando resultados pendientes");
            return;
        }
    };

    // Marcar los corruptos como fallidos (ya lo hizo deserialize_pending internamente, pero se
    // excluyen del envio)
    let send_ids: Vec<String> = ids
        .iter()
        .filter(|id| !failed_ids.contains(id))
        .cloned()
        .collect();

    if results.is_empty() {
        return;
    }

    match client.submit_results(results).await {
        Ok(response) if response.accepted => {
            if let Err(e) = db.mark_sent(&send_ids).await {
                tracing::error!(error = %e, "no se pudo marcar resultados como enviados");
            } else {
                tracing::debug!(count = send_ids.len(), "resultados enviados al servidor");
            }
        }
        Ok(response) => {
            tracing::warn!(
                count = send_ids.len(),
                rejected = response.rejected_count,
                "el servidor no aceptó todos los resultados"
            );
            if let Err(e) = db.mark_failed(&send_ids, "servidor rechazó los resultados").await {
                tracing::error!(error = %e, "no se pudo marcar resultados como fallidos");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "no se pudo enviar resultados, permanecen en cola");
            // Devolver a pending para el próximo flush
            if let Err(db_err) = db.mark_failed(&send_ids, &e.to_string()).await {
                tracing::error!(error = %db_err, "error marcando resultados como fallidos");
            }
        }
    }
}
