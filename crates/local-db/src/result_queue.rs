//! Cola de resultados pendientes de enviar al servidor.
//!
//! Los resultados de checks se insertan en esta cola inmediatamente despues de que el
//! `policy-engine` los produce. El `result_flush` del scheduler los drena periódicamente y envia
//! al servidor.
//!
//! Garantias necesarias
//!
//! * Sin pérdida: un resultado insertado en la cola permanece hasta que el servidor lo confirma o
//! se marca como fallido defenitivo.
//! * Sin duplicados en envio: los resultados pasan a estado `sending` antes de enviarse. Si el
//! proceso muere durante el envio, vuelven a `pending` en el siguiente arranque.
//! * Orden: los resultados se drenan ordenados por `created_at` (FIFO), así el servidor recibe los
//! checks en orden cronologico

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use proto::CheckResult;

use crate::LocalDbError;

// Numero maximo de reintentos antes de marcar un resultado como fallido definitivo. Evita que
// resultados con check_id invalido (rechazados por el servidor) bloqueen la cola para siempre.

pub const MAX_RETRIES: i64 = 5;

// Fila de la tabla `result_queue` como la devuelve SQLite

#[derive(Debug)]
pub struct QueueRow {
    pub id: String,
    pub check_id: String,
    pub data_json: String,
    pub retries: i64,
}

// Inserta un lote de resultados en la cola con estado `pending`.
//
// Cada `CheckResult` se serializa a JSON individualmente. Si la serializacion de alguno falla, el
// error se registra y ese resultado se omite (para que un resultado malformado no bloquee el lote
// entero).
//
// Errores
//
// * `LocalDbError::Database` si falla la transaccion SQLite
pub async fn enqueue(pool: &SqlitePool, results: &[CheckResult]) -> Result<(), LocalDbError> {
    if results.is_empty() {
        return Ok(());
    }

    let mut tx = pool.begin().await.map_err(LocalDbError::Database)?;
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let mut inserted = 0usize;

    for result in results {
        let id = Uuid::new_v4().to_string();
        let check_id = &result.check_id;

        let data_json = match serde_json::to_string(result) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(
                check_id = %check_id,
                error = %e,
                "no se pudo serializar CheckResult, omitiendo"
                );

                continue;
            }
        };

        sqlx::query!(
            r#"
            INSERT INTO result_queue (id, check_id, data_json, status, created_at)
            VALUES (?, ?, ?, 'pending', ?)
            "#,
            id,
            check_id,
            data_json,
            now,
        )
        .execute(&mut *tx)
        .await
        .map_err(LocalDbError::Database)?;

        inserted += 1;
    }

    tx.commit().await.map_err(LocalDbError::Database)?;

    tracing::debug!(
        inserted,
        total = results.len(),
        "resultados añadidos a la cola para envio al servidor"
    );

    Ok(())
}

// Obtiene un lote de resultados `pending` listos para enviar y los marca como `sending`.
//
// Al marcarlos como `sending` antes de devolverlos se evita que otro flush concurrente (no creo q
// pase en verdad pero por poder es posible) los procese dos veces.
//
// Solo devuelve resultados con `retries < MAX_RETRIES`. Los que han superado el maximo se ignoran
// aqui. Usa `purge_failed` para limpiarlos.
//
// Argumentos
// * `limit`: numero maximo de resultados a devolver en un lote.

pub async fn drain_pending(pool: &SqlitePool, limit: i64) -> Result<Vec<QueueRow>, LocalDbError> {
    // Seleccionar los pending con menos retries que el maximo
    let rows = sqlx::query_as!(
        QueueRow,
        r#"
        SELECT id, check_id, data_json, retries
        FROM result_queue
        WHERE status = 'pending' AND retries < ?
        ORDER BY created_at ASC
        LIMIT ?
        "#,
        MAX_RETRIES,
        limit
    )
    .fetch_all(pool)
    .await
    .map_err(LocalDbError::Database)?;

    if rows.is_empty() {
        return Ok(rows);
    }

    // Marcamos el lote como `sending` en una sola query para minimizar el tiempo con el lock de
    // escritura.
    let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let query = format!("UPDATE result_queue SET status = 'sending' WHERE id IN ({placeholders})");

    let mut q = sqlx::query(&query);
    for id in &ids {
        q = q.bind(*id);
    }
    q.execute(pool).await.map_err(LocalDbError::Database)?;

    tracing::debug!(count = rows.len(), "resultados marcados como 'sending'");

    Ok(rows)
}

// Deserializa las filas de la cola a tipos `CheckResult` proto.
//
// Las filas que no se pueden deserializar se marcan como `failed` directamente (estan corruptas y
// no tiene sentido enviarlas)

pub async fn deserialize_rows(
    pool: &SqlitePool,
    rows: Vec<QueueRow>,
) -> Result<(Vec<CheckResult>, Vec<String>), LocalDbError> {
    let mut results = Vec::with_capacity(rows.len());
    let mut failed_ids = Vec::new();

    for row in rows {
        match serde_json::from_str::<CheckResult>(&row.data_json) {
            Ok(r) => results.push(r),
            Err(e) => {
                tracing::warn!(
                id = %row.id,
                check_id = %row.check_id,
                error = %e,
                "resultado corrupto en cola, marcando como fallido"
                );

                failed_ids.push(row.id.clone());
                mark_failed(pool, &[row.id], "JSON corrupto en la cola local").await?;
            }
        }
    }

    Ok((results, failed_ids))
}

// Marca un lote de resultados como `sent` tras confirmar que el servidor los aceptó
//
// Registra el timestamp de envio para auditoria

pub async fn mark_sent(pool: &SqlitePool, ids: &[String]) -> Result<(), LocalDbError> {
    if ids.is_empty() {
        return Ok(());
    }

    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let query = format!(
        "UPDATE result_queue SET status = 'sent', sent_at = ? WHERE id IN ({placeholders})"
    );

    let mut q = sqlx::query(&query).bind(&now);
    for id in ids {
        q = q.bind(id.as_str());
    }

    q.execute(pool).await.map_err(LocalDbError::Database)?;

    tracing::debug!(count = ids.len(), "resultados marcados como enviados");

    Ok(())
}

// Marca un lote de resultados como `failed` e incrementa el contador de reintentos.
//
// Si el motivo es un error de red transitorio, en el siguiente flush volverán a `pending`
// (mientras `retries < MAX_RETRIES`). Si el servidor los rechazo permanentemente, quedaran en
// `failed` definitivamente

pub async fn mark_failed(
    pool: &SqlitePool,
    ids: &[String],
    reason: &str,
) -> Result<(), LocalDbError> {
    if ids.is_empty() {
        return Ok(());
    }

    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let query = format!(
        r#"
        UPDATE result_queue
        SET status = 'failed', retries = retries + 1, error_detail = ?
        WHERE id IN ({placeholders})
        "#
    );

    let mut q = sqlx::query(&query).bind(reason);
    for id in ids {
        q = q.bind(id.as_str());
    }

    q.execute(pool).await.map_err(LocalDbError::Database)?;

    tracing::warn!(
        count = ids.len(),
        reason,
        "resultados marcados como fallidos"
    );

    Ok(())
}

// Resetea los resultados `sending` a `pending`.
//
// Se llama en el arranque del agente para recuperar cualquier resultado que quedo en estado
// `sending` porque el proceso murio durante un flush. Sin esto, esos resultados quedarian
// bloqueados para siempre.

pub async fn reset_sending_to_pending(pool: &SqlitePool) -> Result<u64, LocalDbError> {
    let result =
        sqlx::query!("UPDATE result_queue SET status = 'pending' WHERE status = 'sending'")
            .execute(pool)
            .await
            .map_err(LocalDbError::Database)?;

    let recovered = result.rows_affected();
    if recovered > 0 {
        tracing::warn!(
            count = recovered,
            "resultados en estado 'sending' recuperados a 'pending' tras reinicio"
        );
    }

    Ok(recovered)
}

// Devuelve el numero de resultados pendientes de enviar. Util para metricas y el log de arranque
// del agente.

pub async fn pending_count(pool: &SqlitePool) -> Result<i64, LocalDbError> {
    let count = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM result_queue WHERE status = 'pending' AND retries < ?",
        MAX_RETRIES
    )
    .fetch_one(pool)
    .await
    .map_err(LocalDbError::Database)?;

    Ok(count)
}

// Elimina de la cola los resultados enviados de mas de x dias.
//
// Mantenimiento periodico para que la base de datos local no crezca indefinidamente. Los
// resultados enviados ya no son necesarios localmente pues estan en el servidor.

pub async fn purge_old_sent(pool: &SqlitePool, days: i64) -> Result<u64, LocalDbError> {
    let result = sqlx::query!(
        r#"
        DELETE FROM result_queue
        WHERE status = 'sent' AND sent_at < datetime('now', '-' || ? || ' days')
        "#,
        days
    )
    .execute(pool)
    .await
    .map_err(LocalDbError::Database)?;

    let purged = result.rows_affected();
    if purged > 0 {
        tracing::info!(
            purged,
            days,
            "resultados enviados antiguos eliminados de la cola"
        );
    }

    Ok(purged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connect_test;
    use proto::CheckResult;

    fn make_result(check_id: &str, passed: bool) -> CheckResult {
        CheckResult {
            check_id: check_id.to_string(),
            passed,
            detail: if passed { "OK".into() } else { "FAIL".into() },
            executed_at: 1_700_000_000,
            actual_value: "15".into(),
            expected_value: ">= 15".into(),
        }
    }

    #[tokio::test]
    async fn enqueue_and_drain() {
        let pool = connect_test().await;

        let results = vec![make_result("chk-1", true), make_result("chk-2", false)];
        enqueue(&pool, &results).await.unwrap();

        let count = pending_count(&pool).await.unwrap();
        assert_eq!(count, 2);

        let rows = drain_pending(&pool, 10).await.unwrap();
        assert_eq!(rows.len(), 2);

        // Deben estar en 'sending' ahora
        let still_pending = pending_count(&pool).await.unwrap();
        assert_eq!(still_pending, 0);
    }

    #[tokio::test]
    async fn mark_sent_removes_from_pending() {
        let pool = connect_test().await;

        enqueue(&pool, &[make_result("chk-1", true)]).await.unwrap();
        let rows = drain_pending(&pool, 10).await.unwrap();
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();

        mark_sent(&pool, &ids).await.unwrap();

        let count = sqlx::query_scalar!("SELECT COUNT(*) FROM result_queue WHERE status = 'sent'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn mark_failed_increments_retries() {
        let pool = connect_test().await;

        enqueue(&pool, &[make_result("chk-1", false)])
            .await
            .unwrap();
        let rows = drain_pending(&pool, 10).await.unwrap();
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();

        mark_failed(&pool, &ids, "servidor rechazó").await.unwrap();

        let row = sqlx::query!(
            "SELECT retries, error_detail FROM result_queue WHERE id = ?",
            ids[0]
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(row.retries, 1);
        assert_eq!(row.error_detail.unwrap(), "servidor rechazó");
    }

    #[tokio::test]
    async fn drain_respects_max_retries() {
        let pool = connect_test().await;

        enqueue(&pool, &[make_result("chk-exhausted", false)])
            .await
            .unwrap();

        // Agotamos los reintentos manualmente
        let rows = drain_pending(&pool, 10).await.unwrap();
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();

        // Marcamos como failed MAX_RETRIES veces
        for _ in 0..MAX_RETRIES {
            // Reseteamos a pending para poder drenar de nuevo
            sqlx::query!(
                "UPDATE result_queue SET status = 'pending' WHERE id = ?",
                ids[0]
            )
            .execute(&pool)
            .await
            .unwrap();
            let drained = drain_pending(&pool, 10).await.unwrap();
            mark_failed(
                &pool,
                &drained.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
                "error",
            )
            .await
            .unwrap();
        }

        // A partir de aquí drain_pending no debe devolver este resultado
        sqlx::query!(
            "UPDATE result_queue SET status = 'pending' WHERE id = ?",
            ids[0]
        )
        .execute(&pool)
        .await
        .unwrap();

        let drained = drain_pending(&pool, 10).await.unwrap();
        assert!(
            drained.is_empty(),
            "no debe drenar resultados con MAX_RETRIES alcanzado"
        );
    }

    #[tokio::test]
    async fn reset_sending_to_pending_recovers_stuck() {
        let pool = connect_test().await;

        enqueue(&pool, &[make_result("chk-stuck", true)])
            .await
            .unwrap();

        // Simulamos que el proceso muere durante un drain (queda en 'sending')
        drain_pending(&pool, 10).await.unwrap();

        let sending_count: i64 =
            sqlx::query_scalar!("SELECT COUNT(*) FROM result_queue WHERE status = 'sending'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(sending_count, 1);

        // Al arrancar de nuevo, se recuperan
        let recovered = reset_sending_to_pending(&pool).await.unwrap();
        assert_eq!(recovered, 1);

        let pending = pending_count(&pool).await.unwrap();
        assert_eq!(pending, 1);
    }

    #[tokio::test]
    async fn enqueue_empty_slice_is_noop() {
        let pool = connect_test().await;
        enqueue(&pool, &[]).await.unwrap();
        assert_eq!(pending_count(&pool).await.unwrap(), 0);
    }
}
