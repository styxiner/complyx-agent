//! Pool de conexiones SQLite y ejecucion de las migraciones
//!
//! SQLite en el agente actua de forma single-writer por naturalez: el motor no permite escrituras
//! concurrentes desde multiples conexiones. Sin embargo, usa un pool de tamaño 1 para las
//! escrituras y permite lecturas concurrentes con WAL mode activado.
//! Referencia: [Write-Ahead Logging](https://sqlite.org/wal.html)

use std::path::Path;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{SqlitePool};

use crate::LocalDbError;

// Conecta a la bbdd SQLite en `db_path`, la crea si no existe, activa el modo WAL y ejecuta las
// migraciones pendientes.
//
// Argumentos
// * `db_path`: ruta al fichero SQLite. Ej: `/var/lib/complyx/agent.db`. Se crea el fichero y los
// directorios intermedios si no existen
//
// modo WAL
//
// Journal mode WAL (Write-Ahead Logging) permite lecturas concurrentes mientras hay una escritura
// en curso. Se configura mas que nada porque el agente tiene varias tareas asincronas (poll_loop +
// result_flush + cert_renew).
pub async fn connect(db_path: impl AsRef<Path>) -> Result<SqlitePool, LocalDbError> {
    let path = db_path.as_ref();

    // Crea directorios intermedios si no existen
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| LocalDbError::Io {
                path: parent.display().to_string(),
                source: e,
            })?;
    }

    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true) // Crea el fichero si no existe en el primer arranque del agente
        .journal_mode(SqliteJournalMode::Wal) // Modo WAL: lecturas concurrentes con escrituras
        .synchronous(SqliteSynchronous::Normal) // NORMAL: fsync solo en checkpoints WAL. Para
                                                // balancear rendimiento/durabilidad
        .busy_timeout(std::time::Duration::from_secs(5)) // Timeout si otra conexion tiene el lock
                                                         // de escritura
        .pragma("cache_size", "-4000") // Cache de paginas en memoria (4MB). Mejora lecturas
                                       // repetidas del bundle
        .foreign_keys(true);

    let pool = SqlitePool::connect_with(options)
        .await
        .map_err(LocalDbError::Database)?;

    run_migrations(&pool).await?;

    tracing::info!(
        db_path = %path.display(),
        "base de datos local inicializada"
        );

    Ok(pool)
}

// Ejecuta las migraciones embebidas en el binario
//
// sqlx registra en la tabla `_sqlx_migrations` que migrations se han aplicado y solo ejecuta las
// nuevas. Es idempotente: ejecutarlo varias veces no causa efectos secundarios

async fn run_migrations(pool: &SqlitePool) -> Result<(), LocalDbError> {
    sqlx::migrate!("src/migrations")
        .run(pool)
        .await
        .map_err(|e| LocalDbError::Migration(e.to_string()))?;

    tracing::debug!("migraciones SQLite aplicadas");

    Ok(())
}

#[cfg(test)]
pub(crate) async fn connect_test() -> SqlitePool {
    // Base de datos en memoria para tests. Cada llamada crea una BD nueva e
    // independiente: los tests no se interfieren entre sí.
    let pool = SqlitePool::connect(":memory:")
        .await
        .expect("no se pudo crear BD de test en memoria");
 
    run_migrations(&pool)
        .await
        .expect("migrations fallaron en BD de test");
 
    pool
}
