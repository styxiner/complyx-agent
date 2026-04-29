//! Base de datos SQLite local del agente
//!
//! Responsabilidades principales:
//!
//! * Cache de politicas: Almacena el ultimo `PolicyBundle` recibido del servidor para que el
//! agente pueda seguir ejecutando checks cuando el servidor no esta alcanzable.
//!
//! * Cola de resultados: Los `CheckResult` producidos por el `policy-engine` se encolan aqui antes
//! de enviar al servidor. Si falla la red, los resultados persisten y se envian en el siguiente
//! ciclo de flush
//!
//! * Log de auditorioa: Registro inmutable de todas las remediaciones aplicadas por el
//! `remediation-engine`

mod db;
mod policy_cache;
mod remediation_audit;
mod result_queue;

pub use remediation_audit::{PendingAudit, RemediationOutcome};
pub use result_queue::QueueRow;

use std::path::Path;

use sqlx::SqlitePool;

use proto::{CheckResult, PolicyBundle};

// Errores unificados del crate
#[derive(Debug, thiserror::Error)]
pub enum LocalDbError {
    #[error("Error de base de datos: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Error de serializacion: {0}")]
    Serialization(String),

    #[error("error de migracion: {0}")]
    Migration(String),

    #[error("error de E/S en {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error
    }
}

// Fachada principal del crate. Agrupa las operaciones sobre la bbdd local detrás de una interfaz
// orientada a los casos de uso del agente. Internamente usa un `SqlitePool` compartido. Es barato
// de clonar con un `Arc` interno.
#[derive(Clone, Debug)]
pub struct LocalDb {
    pool: SqlitePool
}

impl LocalDb {
    // Abre o crea la bbdd en `db_path` y aplica las migraciones pendientes
    //
    // Errores
    //
    // * `LocalDbError::Io`: Si no puede crear los directorios
    // * `LocalDbError::Database` si SQLite rechaza la conexion
    // * `LocalDbError::Migration` si alguna migracion falla
    pub async fn connect(db_path: impl AsRef<Path>) -> Result<Self, LocalDbError> {
        let pool = db::connect(db_path).await?;
        Ok(Self {pool})
    }

    // Cache de politicas
    //
    // Persiste el bundle recibido del servidor, reemplazando el anterior
    pub async fn save_bundle(&self, bundle: &PolicyBundle) -> Result<(), LocalDbError> {
        policy_cache::save_bundle(&self.pool, bundle).await
    }

    // Carga el bundle almacenado en cache. `None` si no hay bundle todavia
    pub async fn load_bundle(&self) -> Result<Option<PolicyBundle>, LocalDbError> {
        policy_cache::load_bundle(&self.pool).await
    }

    // Devuelve el hash del bundle en cache para incluirlo en el `PollRequest`. Cadena vacia si no
    // hay bundle cacheado
    pub async fn get_bundle_hash(&self) -> Result<String, LocalDbError> {
        Ok(policy_cache::get_bundle_hash(&self.pool))
        .await?
        .unwrap_or_default()
    }

    // Elimina el bundle de la cache. Se usa al des registrar un agente
    pub async fn clear_bundle(&self) -> Result<(), LocalDbError> {
        policy_cache::clear_bundle(&self.pool).await
    }

    /// Inserta un lote de resultados en la cola con estado `pending`.
    pub async fn enqueue_results(&self, results: &[CheckResult]) -> Result<(), LocalDbError> {
        result_queue::enqueue(&self.pool, results).await
    }
 
    /// Obtiene hasta `limit` resultados `pending` y los marca como `sending`.
    ///
    /// Devuelve las filas crudas. Usa `deserialize_pending` para convertirlas
    /// a `CheckResult` antes de enviarlas al servidor.
    pub async fn drain_pending(&self, limit: i64) -> Result<Vec<QueueRow>, LocalDbError> {
        result_queue::drain_pending(&self.pool, limit).await
    }
 
    /// Deserializa las filas obtenidas de `drain_pending` a tipos proto.
    ///
    /// Las filas corruptas se marcan como `failed` automáticamente y se excluyen
    /// del resultado. Devuelve `(resultados_válidos, ids_fallidos)`.
    pub async fn deserialize_pending(
        &self,
        rows: Vec<QueueRow>,
    ) -> Result<(Vec<CheckResult>, Vec<String>), LocalDbError> {
        result_queue::deserialize_rows(&self.pool, rows).await
    }
 
    /// Marca los resultados identificados por `ids` como enviados con éxito.
    pub async fn mark_sent(&self, ids: &[String]) -> Result<(), LocalDbError> {
        result_queue::mark_sent(&self.pool, ids).await
    }
 
    /// Marca los resultados como fallidos e incrementa su contador de reintentos.
    pub async fn mark_failed(&self, ids: &[String], reason: &str) -> Result<(), LocalDbError> {
        result_queue::mark_failed(&self.pool, ids, reason).await
    }
 
    /// Recupera resultados que quedaron en `sending` tras un reinicio del agente.
    ///
    /// Debe llamarse en el arranque, antes de iniciar el flush loop.
    pub async fn reset_sending_to_pending(&self) -> Result<u64, LocalDbError> {
        result_queue::reset_sending_to_pending(&self.pool).await
    }
 
    /// Número de resultados pendientes de enviar. Para logs y métricas.
    pub async fn pending_count(&self) -> Result<i64, LocalDbError> {
        result_queue::pending_count(&self.pool).await
    }
 
    /// Elimina resultados enviados hace más de `days` días.
    pub async fn purge_old_sent(&self, days: i64) -> Result<u64, LocalDbError> {
        result_queue::purge_old_sent(&self.pool, days).await
    }
 
    /// Abre una entrada de auditoría antes de ejecutar una remediación.
    /// Devuelve el `audit_id` que se pasa a `finish_remediation`.
    pub async fn start_remediation(&self, check_id: &str, remediation_id: &str, remediation_type: &str, params_json: &str) -> Result<String, LocalDbError> {
        remediation_audit::start_remediation(
            &self.pool,
            check_id,
            remediation_id,
            remediation_type,
            params_json,
        )
        .await
    }
 
    /// Cierra la entrada de auditoría con el resultado final.
    pub async fn finish_remediation(&self, audit_id: &str, outcome: RemediationOutcome) -> Result<(), LocalDbError> {
        remediation_audit::finish_remediation(&self.pool, audit_id, outcome).await
    }
 
    /// Devuelve remediaciones que quedaron en `pending` tras un crash del agente.
    pub async fn get_pending_audits(&self) -> Result<Vec<PendingAudit>, LocalDbError> {
        remediation_audit::get_pending_audits(&self.pool).await
    }
}
