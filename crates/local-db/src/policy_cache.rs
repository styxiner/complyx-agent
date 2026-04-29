//! Cache local del bundle de politicas
//!
//! Almacena el ultimo `PolicyBundle` recibido del servidor para que el agente pueda seguir
//! ejecutando checks aunque el servidor no sea alcanzable.
//!
//! Solo existe un bundle almacenado a la vez (el mas reciente). La tabla `policy_bundle` usa una
//! clave primaria fija "current" para que el upsert sea atómico y no deje filas huérfanas.

use sqlx::SqlitePool;

use proto::PolicyBundle;

use crate::LocalDbError;

// Persiste un nuevo `PolicyBundle` en la cache local, reemplazando el anterior.
//
// La serializacion a JSON usa los derives de serde que `build.rs` del crate `proto` añade a todos
// los tipos generados.
//
// Errores
//
// * `LocalDbError::Serialization` si el bundle no se puede serializar a JSON.
// * `LocalDbError::Database` si falla la escritura en SQLite

pub async fn save_bundle(pool: &SqlitePool, bundle: &PolicyBundle) -> Result<(), LocalDbError> {
    let data_json = serde_json::to_string(bundle).map_err(|e| LocalDbError::Serialization(e.to_string()))?;

    let hash = &bundle.bundle_hash;

    sqlx::query!(
        r#"
        INSERT INTO policy_bundle (key, hash, data_json, cached_at)
        VALUES ('current', ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        ON CONFLICT (key) DO UPDATE SET
            hash = excluded.hash,
            data_json = excluded.data_json,
            cached_at = excluded.cached_at
        "#,
        hash,
        data_json
        )
        .execute(pool)
        .await
        .map_err(LocalDbError::Database)?;

    tracing::debug!(
        hash = %hash,
        policy_count = bundle.policies.len(),
        "bundle de politicas guardado en cache"
        );

    Ok(())
}

// Carga el bundle de politicas almacenado en cache
// 
// Devuelve `None` si el agente aun no ha recibido ningun bundle del servidor (primer arranque tras
// registrarse, antes del primer poll exitoso).
//
// Errores
//
// * `LocalDbError::Serialization` si el JSON almacenado no se puede deserializar
// * `LocalDbError::Database` si falla la lectura de SQLite

pub async fn  load_bundle(pool: &SqlitePool) -> Result<Option<PolicyBundle>, LocalDbError> {
    let row = sqlx::query!("SELECT data_json FROM policy_bundle WHERE key = 'current'")
        .fetch_optional(pool)
        .await
        .map_err(LocalDbError::Database)?;

    match row {
        None => {
            tracing::debug!("no hay bundle de politicas en cache");

            Ok(None)
        }

        Some(r) => {
            let bundle: PolicyBundle = serde_json::from_str(&r.data_json)
                .map_err(|e| LocalDbError::Serialization(e.to_string()))?;

            tracing::debug!(
                hash = %bundle.bundle_hash,
                policy_count = %bundle.policies.len(),
                "bundle de politicas cargado en cache"
                );

            Ok(Some(bundle))
        }
    }
}

// Devuelve el hash sha-256 del bundle almacenado, o `None` si no hay caché.
//
// Este hash es el que el agente envia en cada `PollRequest` para que el servidor pueda detectar si
// el bundle ha cambiado sin retransmitirlo completo.
pub async fn get_bundle_hash(pool: &SqlitePool) -> Result<Option<String>, LocalDbError> {
    let row = sqlx::query!("SELECT hash FROM policy_bundle WHERE key = 'current'")
        .fetch_optional(pool)
        .await
        .map_err(LocalDbError::Database)?;

    Ok(row.map(|r| r.hash))
}

// Elimina el bundle de politicas de la cache
// Se usa cuando el agente se des registra o cuando el administrador fuerza un re-registro. El
// agente queda en estado "sin politicas" hasta el proximo poll exitoso.
pub async fn clear_bundle(pool: &SqlitePool) -> Result<(), LocalDbError> {
    sqlx::query!("DELETE FROM policy_bundle WHERE key = 'current'")
        .execute(pool)
        .await
        .map_err(LocalDbError::Database)?;

    tracing::info!("cacje del bundle de politicas eliminada");

    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connect_test;
    use proto::{Policy, PolicyBundle};
 
    fn make_bundle(hash: &str, policy_count: usize) -> PolicyBundle {
        PolicyBundle {
            bundle_hash: hash.to_string(),
            policies: (0..policy_count)
                .map(|i| Policy {
                    id: format!("pol-{i}"),
                    name: format!("Policy {i}"),
                    version: "1.0".into(),
                    severity: "high".into(),
                    elements: vec![],
                })
                .collect(),
        }
    }
 
    #[tokio::test]
    async fn save_and_load_bundle() {
        let pool = connect_test().await;
        let bundle = make_bundle("abc123", 3);
 
        save_bundle(&pool, &bundle).await.unwrap();
 
        let loaded = load_bundle(&pool).await.unwrap().unwrap();
        assert_eq!(loaded.bundle_hash, "abc123");
        assert_eq!(loaded.policies.len(), 3);
    }
 
    #[tokio::test]
    async fn load_bundle_returns_none_when_empty() {
        let pool = connect_test().await;
        let result = load_bundle(&pool).await.unwrap();
        assert!(result.is_none());
    }
 
    #[tokio::test]
    async fn get_bundle_hash_returns_hash() {
        let pool = connect_test().await;
        save_bundle(&pool, &make_bundle("deadbeef", 1)).await.unwrap();
 
        let hash = get_bundle_hash(&pool).await.unwrap();
        assert_eq!(hash, Some("deadbeef".to_string()));
    }
 
    #[tokio::test]
    async fn save_bundle_replaces_previous() {
        let pool = connect_test().await;
 
        save_bundle(&pool, &make_bundle("hash-v1", 1)).await.unwrap();
        save_bundle(&pool, &make_bundle("hash-v2", 2)).await.unwrap();
 
        let loaded = load_bundle(&pool).await.unwrap().unwrap();
        assert_eq!(loaded.bundle_hash, "hash-v2");
        assert_eq!(loaded.policies.len(), 2);
 
        // Solo debe haber una fila
        let count: i64 = sqlx::query_scalar!("SELECT COUNT(*) FROM policy_bundle")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
 
    #[tokio::test]
    async fn clear_bundle_removes_cache() {
        let pool = connect_test().await;
        save_bundle(&pool, &make_bundle("xyz", 1)).await.unwrap();
        clear_bundle(&pool).await.unwrap();
 
        assert!(load_bundle(&pool).await.unwrap().is_none());
        assert!(get_bundle_hash(&pool).await.unwrap().is_none());
    }
}
 

