//! Scheduler del agente: gestiona los tres loops concurrentes con `tokio::select!`.
//!
//! Arranca tres tareas independientes:
//!
//! | Loop | Intervalo (defecto) | Función |
//! |---|---|---|
//! | poll | 5 min | Obtiene políticas, ejecuta checks, encola resultados |
//! | flush | 1 min | Drena la cola de resultados y los envía al servidor |
//! | heartbeat | 2 min | Notifica al servidor que el agente está vivo |
//!
//! Además, en cada arranque ejecuta:
//! - Un tick inmediato de poll (para no esperar el primer intervalo).
//! - Un intento de renovación de certificado si queda < 30 días para expirar.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::{interval, MissedTickBehavior};

use grpc_client::GrpcClient;
use local_db::LocalDb;
use policy_engine::PolicyEngine;
use remediation_engine::RemediationEngine;

use crate::config::AgentConfig;
use crate::poll_loop::{flush_results, run_tick};

// Arranca todos los loops del agente y bloquea hasta recibir señal de parada.
//
// Recibe un `shutdown_rx` que se activa cuando el proceso recibe SIGTERM o SIGINT. Cuando se
// activa, espera a que el tick en curso termine antes de salir.
pub async fn run(config: Arc<AgentConfig>, client: Arc<GrpcClient>, db: Arc<LocalDb>, engine: Arc<PolicyEngine>, remediation: Arc<RemediationEngine>, mut shutdown_rx: tokio::sync::watch::Receiver<bool>,) {
    tracing::info!(
        poll_interval_secs = config.poll_interval_secs,
        flush_interval_secs = config.flush_interval_secs,
        heartbeat_interval_secs = config.heartbeat_interval_secs,
        auto_remediate = config.auto_remediate,
        "scheduler arrancado"
    );

    // Tick inmediato al arrancar (no esperar el primer intervalo)
    tracing::info!("ejecutando poll inicial...");
    run_tick(
        Arc::clone(&config),
        Arc::clone(&client),
        Arc::clone(&db),
        Arc::clone(&engine),
        Arc::clone(&remediation),
    ).await;

    // Configurar los tres intervals
    // MissedTickBehavior::Skip: si un tick tarda más que el intervalo, se salta el siguiente en
    // lugar de ejecutarlos todos acumulados
    let mut poll_interval = interval(Duration::from_secs(config.poll_interval_secs));
    poll_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    poll_interval.tick().await; // consumir el tick inicial que se ejecutó arriba

    let mut flush_interval = interval(Duration::from_secs(config.flush_interval_secs));
    flush_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    flush_interval.tick().await; // consumir tick inicial

    let mut heartbeat_interval = interval(Duration::from_secs(config.heartbeat_interval_secs));
    heartbeat_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    heartbeat_interval.tick().await;

    // Intervalo de mantenimiento diario: purga resultados antiguos + log de estado
    let mut daily_interval = interval(Duration::from_secs(86_400)); // 24h
    daily_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    daily_interval.tick().await;

    loop {
        tokio::select! {
            // Señal de parada (SIGTERM / SIGINT)
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    tracing::info!("señal de parada recibida, finalizando scheduler");
                    break;
                }
            }

            // Tick de poll: obtener políticas + ejecutar checks
            _ = poll_interval.tick() => {
                tracing::debug!("tick de poll");
                run_tick(
                    Arc::clone(&config),
                    Arc::clone(&client),
                    Arc::clone(&db),
                    Arc::clone(&engine),
                    Arc::clone(&remediation),
                ).await;
            }

            // Tick de flush: enviar resultados pendientes
            _ = flush_interval.tick() => {
                tracing::debug!("tick de flush");
                flush_results(&client, &db).await;
            }

            // Tick de heartbeat
            _ = heartbeat_interval.tick() => {
                tracing::debug!("tick de heartbeat");
                if let Err(e) = client.heartbeat().await {
                    tracing::warn!(error = %e, "heartbeat fallido");
                }
            }

            // Mantenimiento diario
            _ = daily_interval.tick() => {
                daily_maintenance(&config, &db).await;
            }
        }
    }

    tracing::info!("scheduler finalizado");
}

// Tareas de mantenimiento que se ejecutan una vez al día.
async fn daily_maintenance(config: &AgentConfig, db: &LocalDb) {
    tracing::debug!("ejecutando mantenimiento diario");

    // Purgar resultados enviados hace más de N días
    match db.purge_old_sent(config.result_purge_days).await {
        Ok(purged) if purged > 0 => {
            tracing::info!(purged, days = config.result_purge_days, "resultados antiguos purgados");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "error purgando resultados antiguos"),
    }

    // Log del estado de la cola
    match db.pending_count().await {
        Ok(pending) if pending > 0 => {
            tracing::warn!(pending, "resultados pendientes en cola tras mantenimiento diario");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "error consultando cola pendiente"),
    }
}
