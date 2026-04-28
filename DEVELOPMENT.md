# complyx-agent
> Guía técnica para desarrolladores que se incorporan al proyecto. Explicación de la arquitectura del agente, crates que lo componen, conceptos de Rust y Tokio usados y cómo está organizado el código

## 1. Stack tecnologico
| Tecnología | Versión | Para qué se usa|
|------------|---------|-----------------|
| Rust | 1.93.1+ | Lenguaje principal del agente |
| Tokio | 1.x | Runtime asíncrono: Ejecuta el poll loop, schedulers y operaciones E/S |
| Tonic | 0.11+ | Cliente gRPC generado a partir del esquema .proto |
| Prost | 0.12+ | Serialización/deserialización de protobuff |
| Rustls | 0.23+ | Implementación TLS sin OpenSSL. Base para mTLS |
| sqlx | 0.7+ | Acceso asíncrono a SQLite con queries comprobadas en compilación |
| serde / serde_json | 1.x | Serialización de la configuración y de los parametros de checks | 
| rcgen | 0.12+ | Generación del par de claves y CSR durante el proceso de registro de agente |
| regex | 1.x | Parseo de lineas en los executors de contenido de ficheros |
| figment | 0.10+ | Carga de configuración desde TOML y variables de entorno |
| tracing | 0.1+ | Logs estructurados y spans de diagnóstico |
| uuid | 1.x | Identificadores únicos para agente, checks y resultados |

---

## 2. Estructura del proyecto
 
El repositorio es un **Cargo workspace**: varios crates con responsabilidades distintas que compilan juntos. El binario final es `agent-core`; el resto son librerías que consume.
 
```
complyx-agent/
├── Cargo.toml                          # workspace: lista todos los crates miembro
├── Cargo.lock
├── proto/
│   └── complyx.proto                   # fuente de verdad del contrato gRPC
│
└── crates/
    ├── agent-core/                     # binario principal
    ├── grpc-client/                    # cliente gRPC con mTLS
    ├── policy-engine/                  # librería: interpreta y ejecuta checks
    ├── remediation-engine/             # librería: aplica remediaciones
    ├── local-db/                       # SQLite: caché de políticas y cola de resultados
    └── proto/                          # crate generado por build.rs a partir del .proto
```
 
> Cada crate es autónomo: tiene su propio `Cargo.toml`, sus dependencias declaradas explícitamente y no comparte código de forma implícita con otros crates. Un cambio en `policy-engine` no recompila `grpc-client` salvo que cambie su interfaz pública.
 
---
 
## 3. Crates en detalle
 
### 3.1 `agent-core` — el binario
 
Punto de entrada del proceso. Responsabilidades:
 
- Cargar y validar la configuración (`AgentConfig`).
- Verificar si el agente está enrolado (cert presente). Si no, ejecutar el flujo de enrolamiento antes de arrancar.
- Construir todas las dependencias (`PolicyEngine`, `RemediationEngine`, `LocalDb`, `GrpcClient`) e inyectarlas donde se necesiten.
- Arrancar los tres loops concurrentes mediante `tokio::join!`:
  - **poll_loop**: obtiene políticas del servidor y ejecuta checks.
  - **result_flush**: drena la cola SQLite y envía resultados pendientes.
  - **cert_renew**: comprueba la caducidad del certificado y lanza renovación si queda menos de 30 días.
```
agent-core/src/
├── main.rs          # tokio::main, carga config, comprueba enrolamiento, lanza loops
├── config.rs        # AgentConfig { server_url, enroll_url, poll_interval_secs, cert_dir, db_path, log_level }
├── poll_loop.rs     # loop principal: PollPolicies → PolicyEngine → LocalDb → SubmitResults
└── scheduler.rs     # gestiona los tres intervals de Tokio con sus respectivos handles
```
 
**`config.rs`** usa `figment` para fusionar `config.toml` con variables de entorno. Las variables de entorno tienen precedencia y siguen el esquema `COMPLYX_<CAMPO>` (ej. `COMPLYX_SERVER_URL`).
 
**`poll_loop.rs`** es el núcleo del comportamiento del agente. En cada tick:
 
1. Llama a `grpc_client.poll_policies(current_bundle_hash)`.
2. Si el servidor responde `policies_changed = false`, no hay nada que hacer en este tick.
3. Si hay bundle nuevo, lo persiste en `local_db.save_bundle()` y ejecuta todos los checks con `policy_engine.run_all(bundle)`.
4. Los resultados se encolan en `local_db.enqueue_results(results)`.
5. Inmediatamente después intenta un flush: `grpc_client.submit_results(pending)`. Si falla (red caída), los resultados permanecen en la cola para el siguiente tick del `result_flush`.
### 3.2 `grpc-client` — cliente gRPC con mTLS
 
Encapsula toda la comunicación de red. El resto del código no conoce Tonic ni Protobuf directamente: trabaja con tipos propios del dominio que este crate traduce.
 
```
grpc-client/src/
├── lib.rs       # re-exporta GrpcClient y los tipos públicos
├── client.rs    # GrpcClient: wrappea el stub de Tonic generado, gestiona reconexión
├── mtls.rs      # carga cert + key + CA raíz y construye ClientTlsConfig de Rustls
├── enroll.rs    # flujo de enrolamiento: genera keypair Ed25519, construye CSR con rcgen,
│               #   llama al endpoint Enroll (sin mTLS), guarda cert y clave en disco
└── retry.rs    # RetryPolicy: backoff exponencial con jitter para errores de red
```
 
**mTLS en la práctica**: `mtls.rs` lee tres ficheros del `cert_dir` configurado:
 
- `agent.crt` — certificado del agente firmado por la CA interna.
- `agent.key` — clave privada del agente (generada en el enrolamiento).
- `ca.crt` — certificado raíz de la CA interna del servidor.
Con estos tres ficheros construye un `ClientTlsConfig` que Tonic usa en el canal gRPC. El servidor verifica el `agent.crt` y extrae el CN para identificar al agente sin necesidad de tokens adicionales en cada llamada.
 
**Reconexión**: `client.rs` no expone el canal directamente. Mantiene un `Arc<Mutex<Option<Channel>>>` y en cada llamada comprueba si el canal está activo. Si no, intenta reconectar aplicando la `RetryPolicy`.
 
### 3.3 `policy-engine` — motor de checks
 
Librería pura: no hace I/O de red, no escribe en disco (excepto lecturas del sistema para ejecutar los checks). Es la parte más testeable del agente.
 
```
policy-engine/src/
├── lib.rs                  # pub use PolicyEngine, CheckSpec, CheckResult
├── engine.rs               # PolicyEngine { executors: HashMap<&'static str, Arc<dyn CheckExecutor>> }
│                           #   run_all(bundle) → Vec<CheckResult>
│                           #   run_check(id, spec) → CheckResult
├── executor.rs             # trait CheckExecutor + enum CompareOperator (=, !=, >=, <=, >, <, contains, not_contains, regex)
├── result.rs               # CheckResult { check_id, passed, detail, actual_value, expected, executed_at }
└── executors/
    ├── mod.rs              # register_all_executors() → PolicyEngine con todos los executors
    ├── filesystem/
    │   ├── mod.rs
    │   ├── file_exists.rs  # verifica existencia, owner, group y mode de un fichero/directorio
    │   ├── file_absent.rs  # verifica que un path NO existe
    │   ├── dir_contains.rs # verifica que un directorio contiene ficheros que cumplen un glob, con owner y mode opcionales
    │   └── symlink.rs      # verifica que un symlink apunta al target correcto
    ├── content/
    │   ├── mod.rs
    │   ├── file_line.rs    # busca una línea con formato "key <sep> value" y la compara con un operador. Ej: PASS_MIN_LEN >= 15 en /etc/login.defs
    │   ├── file_block.rs   # verifica que el fichero contiene/no contiene patrones regex. Útil para pam.d, sudoers, sshd_config con bloques complejos
    │   └── ini_value.rs    # verifica valores en ficheros con formato [section] key = value
    ├── package/
    │   ├── mod.rs
    │   ├── pkg_installed.rs # verifica que un paquete está instalado con restricción de versión. Detecta automáticamente dpkg / rpm / pacman
    │   └── pkg_absent.rs   # verifica que un paquete no está instalado
    └── system/
        ├── mod.rs
        ├── sysctl.rs       # lee /proc/sys o llama a sysctl(8) y compara el valor
        ├── service.rs      # consulta el estado de un servicio (systemd via D-Bus, sin shell)
        └── user_attr.rs    # lee /etc/passwd, /etc/shadow y /etc/group y verifica atributos (shell, grupos, password_max_age, etc.)
```
 
**Cómo añadir un nuevo tipo de check**:
 
1. Crear el fichero en el subdirectorio correspondiente de `executors/`.
2. Implementar el trait `CheckExecutor`:
   ```rust
   #[async_trait]
   impl CheckExecutor for MiNuevoExecutor {
       fn check_type(&self) -> &'static str { "mi_tipo" }
       async fn execute(&self, params: &serde_json::Value) -> Result<CheckResult, CheckError> {
           let p: MiParams = serde_json::from_value(params.clone())?;
           // ...
       }
   }
   ```
3. Registrarlo en `executors/mod.rs` dentro de `register_all_executors()`.
4. Añadir tests unitarios en el mismo fichero bajo `#[cfg(test)]`.
**El `check_command` no es un comando de shell.** Es un JSON con `type` y `params` que el engine deserializa a una estructura tipada. Añadir un nuevo tipo de check requiere un cambio de código en el agente — esto es intencional por seguridad.
 
### 3.4 `remediation-engine` — motor de remediaciones
 
Misma arquitectura que `policy-engine` pero para aplicar correcciones. Se ejecuta solo si la política y el operador lo autorizan.
 
```
remediation-engine/src/
├── lib.rs
├── engine.rs       # RemediationEngine: run_remediation(spec) → RemediationResult
├── executor.rs     # trait RemediationExecutor
├── audit.rs        # antes de aplicar cualquier cambio escribe en local-db:
│                   #   { check_id, action, params, timestamp, status: pending }
│                   #   después actualiza a: { status: applied | failed, detail }
└── remediators/
    ├── mod.rs
    ├── file_line_set.rs    # escribe o reemplaza "key value" en un fichero de configuración.
    │                       #   Siempre hace backup del fichero original antes de modificarlo
    ├── file_block_set.rs   # asegura que un bloque de texto está presente en un fichero
    ├── pkg_install.rs      # instala un paquete via el package manager detectado
    ├── pkg_remove.rs       # elimina un paquete
    ├── sysctl_set.rs       # escribe el valor en /etc/sysctl.d/99-complyx.conf y llama sysctl --system para aplicarlo sin reinicio
    └── service_set.rs      # activa/desactiva o arranca/para un servicio via systemd D-Bus
```
 
**Principio de mínimo privilegio**: el agente debe correr con los permisos mínimos necesarios. Los remediators que requieren root (editar `/etc`, gestionar paquetes) comprueban que tienen los permisos necesarios antes de intentar la acción y devuelven `RemediationResult::PermissionDenied` en caso contrario, en lugar de fallar de forma inesperada (Como está en una etapa inicial, es posible que deje el binario con capabilities y apañado).
 
### 3.5 `local-db` — SQLite local
 
Dos responsabilidades: caché de políticas para operación offline, y cola de resultados para resiliencia ante pérdida de conectividad.
 
```
local-db/src/
├── lib.rs
├── db.rs               # connect(path) → SqlitePool, run_migrations()
├── policy_cache.rs     # save_bundle(bundle) → reemplaza el bundle almacenado
│                       # load_bundle() → Option<PolicyBundle>
│                       # get_hash() → Option<String> (para el campo policy_bundle_hash del poll)
├── result_queue.rs     # enqueue(results) → inserta con status = 'pending'
│                       # drain_pending(limit) → Vec<CheckResult> pendientes de enviar
│                       # mark_sent(ids) → actualiza status = 'sent'
│                       # mark_failed(ids, reason) → actualiza status = 'failed', incrementa retries
└── migrate.rs          # sqlx::migrate!("./migrations") — embebe las migrations en el binario
    migrations/
    ├── 001_policy_cache.sql   # tabla policy_bundle { hash, data_json, cached_at }
    └── 002_result_queue.sql   # tabla result_queue { id, check_id, data_json, status, created_at, sent_at, retries }
```
 
Las migrations están **embebidas en el binario** mediante `sqlx::migrate!`. El agente crea y migra `agent.db` en el primer arranque sin herramientas externas — fundamental para el despliegue en endpoints sin acceso a infraestructura adicional.
 
### 3.6 `proto` — tipos gRPC generados
 
Crate auxiliar que invoca `tonic_build` en tiempo de compilación y expone los tipos Protobuf generados.
 
```
proto/
├── Cargo.toml      # deps: tonic, prost
├── build.rs        # tonic_build::configure()
│                   #   .build_server(false)   ← el agente solo necesita el cliente
│                   #   .compile(&["../../proto/complyx.proto"], &["../../proto/"])
└── src/
    └── lib.rs      # include!(concat!(env!("OUT_DIR"), "/complyx.rs"))
```
 
Cualquier cambio en `complyx.proto` requiere recompilar este crate. Los tipos generados (structs Protobuf) solo se usan dentro de `grpc-client`; el resto del agente trabaja con tipos propios del dominio para no acoplarse al contrato de red.
 
---
 
## 4. Conceptos clave de Rust y Tokio
 
Esta sección explica los patrones que aparecen con más frecuencia en el código.
 
### 4.1 `async/await` y Tokio
 
El agente es completamente asíncrono. Tokio es el runtime que ejecuta las tareas. El punto de entrada es:
 
```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ...
}
```
 
Las funciones marcadas como `async` devuelven un `Future` que no se ejecuta hasta que alguien hace `.await` sobre ellas. Tokio multithread ejecuta varias tareas concurrentemente en un pool de threads.
 
### 4.2 `Arc<T>` — compartir estado entre tareas
 
Cuando varias tareas asíncronas necesitan acceder al mismo valor (ej. `LocalDb`, `GrpcClient`), se envuelve en `Arc<T>` (Atomic Reference Counted). `Arc` permite tener múltiples propietarios del mismo valor de forma segura entre threads.
 
```rust
let db = Arc::new(LocalDb::connect(&config.db_path).await?);
let engine = Arc::new(PolicyEngine::new());
 
// se clona el Arc (no el valor) para pasarlo a cada tarea
let db_poll = Arc::clone(&db);
let db_flush = Arc::clone(&db);
 
tokio::join!(
    poll_loop(Arc::clone(&db_poll), Arc::clone(&engine), ...),
    result_flush(Arc::clone(&db_flush), ...),
);
```
 
### 4.3 `trait` — interfaces y polimorfismo
 
`CheckExecutor` y `RemediationExecutor` son traits: definen una interfaz que varios tipos concretos implementan. Esto permite añadir nuevos tipos de checks sin modificar el motor.
 
```rust
#[async_trait]
pub trait CheckExecutor: Send + Sync {
    fn check_type(&self) -> &'static str;
    async fn execute(&self, params: &serde_json::Value) -> Result<CheckResult, CheckError>;
}
```
 
`Send + Sync` son marcadores que Rust exige para que el trait pueda usarse en contextos multithreaded. `async_trait` es un macro externo necesario porque Rust aún no soporta métodos `async` directamente en traits estables.
 
### 4.4 `Result<T, E>` y `?` — gestión de errores
 
Rust no tiene excepciones. Los errores se representan como valores del tipo `Result<T, E>`. El operador `?` propaga el error hacia arriba si la operación falla, equivalente a un `return Err(e)` automático.
 
```rust
async fn execute(&self, params: &Value) -> Result<CheckResult, CheckError> {
    let p: FileLineParams = serde_json::from_value(params.clone())?; // propaga si falla la deserialización
    let content = tokio::fs::read_to_string(&p.path).await?;        // propaga si no se puede leer el fichero
    // ...
}
```
 
El crate usa `anyhow` para errores en el binario (`agent-core`) y tipos de error propios en las librerías (`CheckError`, `RemediationError`) para dar mensajes de error precisos.
 
### 4.5 `serde` — serialización
 
Los params de cada check viajan como JSON desde el servidor. `serde` permite deserializarlos a structs tipados en tiempo de ejecución:
 
```rust
#[derive(Deserialize)]
struct FileLineParams {
    path: PathBuf,
    key: String,
    operator: CompareOperator,
    value: String,
    #[serde(default = "default_separator")]
    separator: String,
}
 
// en el executor:
let p: FileLineParams = serde_json::from_value(params.clone())?;
```
 
Si el JSON recibido no tiene todos los campos requeridos o tiene tipos incorrectos, la deserialización falla con un error descriptivo que se registra en el resultado del check.
 
### 4.6 `sqlx` — queries comprobadas en compilación
 
sqlx verifica en tiempo de compilación que las queries SQL son válidas contra el esquema de la base de datos. Requiere que exista un fichero `.sqlx/` con los metadatos del schema (generado con `cargo sqlx prepare`).
 
```rust
// query! macro: verificada en compilación
let rows = sqlx::query!(
    "SELECT id, data_json FROM result_queue WHERE status = 'pending' LIMIT ?",
    limit
)
.fetch_all(&self.pool)
.await?;
```
 
---
 
## 5. Flujo completo: del poll a la base de datos
 
```
1. TICK DEL SCHEDULER (cada poll_interval_secs)
   scheduler.rs dispara poll_loop
 
2. POLL AL SERVIDOR
   grpc_client.poll_policies(hash_actual)
   → PollRequest { agent_id, policy_bundle_hash }
   ← PollResponse { policies_changed: true/false, bundle }
 
   Si policies_changed = false → fin del tick (no hay nada nuevo)
 
3. CACHÉ LOCAL
   local_db.save_bundle(bundle)     // persiste el nuevo bundle en SQLite
   // Si el servidor cae después de este punto, el agente puede seguir usando el bundle cacheado
 
4. EJECUCIÓN DE CHECKS
   policy_engine.run_all(&bundle)
   → para cada PolicyCheck en el bundle:
       a. Busca el executor por check_type
       b. Deserializa check_params_json a la struct del executor
       c. Ejecuta el check (lee fichero, consulta paquete, etc.)
       d. Devuelve CheckResult { check_id, passed, actual_value, expected, detail }
 
5. COLA DE RESULTADOS
   local_db.enqueue_results(results)   // status = 'pending'
 
6. FLUSH AL SERVIDOR
   local_db.drain_pending(batch_size)  // obtiene los pending
   grpc_client.submit_results(batch)
   → SubmitResultsRequest { agent_id, results }
   ← SubmitResultsResponse { accepted: true }
 
   Si accepted: local_db.mark_sent(ids)
   Si error de red: los resultados permanecen en 'pending' para el siguiente flush
```
 
---
 
## 6. Flujo de registro
 
El registro ocurre una sola vez: la primera vez que el agente arranca sin certificado en `cert_dir`. A partir de ahí, usa siempre su certificado para autenticarse.
 
```
1. El administrador genera un token de un solo uso en el servidor:
   complyx-server enroll-token --hostname server-01.complyx.local
   → token: "a3f8c2d1..."
 
2. El token se pasa al agente en el primer arranque:
   COMPLYX_ENROLL_TOKEN=a3f8c2d1... complyx-agent
 
3. agent-core detecta que no hay cert → llama a grpc_client.enroll(token)
 
4. enroll.rs:
   a. Genera keypair Ed25519 con rcgen
   b. Construye un CSR (Certificate Signing Request) con CN = hostname del sistema
   c. Llama al endpoint ComplyxEnroll.Enroll (puerto separado, sin mTLS, TLS one-way)
      → EnrollRequest { token, csr_pem, hostname, os_name, os_version }
      ← EnrollResponse { cert_pem, ca_cert_pem }
   d. Guarda agent.key, agent.crt y ca.crt en cert_dir
 
5. El token queda invalidado en el servidor (solo se puede usar una vez)
 
6. A partir de ahora, todos los polls usan mTLS con el certificado emitido
```
 
---
 
## 7. Convenciones de desarrollo
 
### Nomenclatura
 
| Tipo | Convención | Ejemplo |
|------|------------|---------|
| Struct de datos | `PascalCase` | `CheckResult`, `AgentConfig` |
| Trait | `PascalCase` | `CheckExecutor`, `RemediationExecutor` |
| Implementación concreta | `PascalCase` + nombre descriptivo | `FileLineExecutor`, `SysctlExecutor` |
| Parámetros de un executor | `<Tipo>Params` | `FileLineParams`, `DirContainsParams` |
| Errores de dominio | `<Módulo>Error` | `CheckError`, `RemediationError` |
| Ficheros de módulo | `snake_case` | `file_line.rs`, `poll_loop.rs` |
 
### Errores y logging
 
- Las librerías (`policy-engine`, `remediation-engine`, `local-db`) usan tipos de error propios derivados con `thiserror`. Nunca usan `unwrap()` ni `panic!()` en código de producción.
- El binario (`agent-core`) usa `anyhow` para errores en el setup inicial.
- Los logs usan `tracing`: `tracing::info!`, `tracing::warn!`, `tracing::error!`. Los spans de `tracing` se añaden en `poll_loop` con el `agent_id` como campo de contexto para que todos los logs de un tick estén correlacionados.
### Tests
Por falta de tiempo, es posible que no de tiempo a hacer todas las pruebas necesarias.
 
Cada executor debería tener tests unitarios en el mismo fichero:
 
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
 
    #[tokio::test]
    async fn test_file_line_pass() {
        // crea un fichero temporal con el contenido esperado y verifica que el check pasa
    }
 
    #[tokio::test]
    async fn test_file_line_fail_value_too_low() {
        // crea un fichero con valor insuficiente y verifica que el check falla con el detail correcto
    }
}
```
 
Los tests de integración de `grpc-client` usan un servidor gRPC de prueba levantado con `tonic` en el propio test.
 
### `cargo` y herramientas de desarrollo
 
```bash
cargo build                     # compilar el workspace
cargo test                      # ejecutar todos los tests
cargo test -p policy-engine     # tests de un crate concreto
cargo run -p agent-core         # ejecutar el agente en desarrollo
cargo sqlx prepare              # regenerar los metadatos de sqlx para compilación offline
cargo clippy -- -D warnings     # linter (los warnings son errores en CI)
cargo fmt                       # formatear el código
```

