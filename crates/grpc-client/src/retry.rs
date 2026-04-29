//! política de reintentos backoff exponencial y jitter para llamadas gRPC
//!
//! La política disingue entre errores transitorios (temas de red) y errores permanentes (auth
//! fallida, recurso no encontrado), aplicando reintentos solo en los primeros.
//!
//! El tiempo de espera entre reintentos crece exponencialmente para evitar saturación.
//!
//! El jitter (ruido aleatorio) evita la sincronización de múltiples agentes que reconectan a la vez
//! tras una caída del servidor

use std::time::Duration;

use tonic::Code;

// Configuracion de la politica de reintentos
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
    pub jitter_max: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            multiplier: 2.0,
            jitter_max: Duration::from_millis(500),
        }
    }
}

impl RetryPolicy {
    // Crea una politica para el arranque del agente: reintentos más rapidos y más intentos, para
    // que el agente no quede en el limbo si el servidor tarda un poco en estar disponible
    pub fn for_startup() -> Self {
        Self {
            max_attempts: 10,
            base_delay: Duration::from_secs(500),
            max_delay: Duration::from_secs(30),
            multiplier: 1.5,
            jitter_max: Duration::from_millis(200),
        }
    }

    // Crea una politica para el loop de poll: menos reintentos porque habra otro tick en breve de
    // todas formas jajajajaja
    pub fn for_poll() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(15),
            multiplier: 2.0,
            jitter_max: Duration::from_millis(500),
        }
    }

    // Calcula el tiempo de espera para el intento x 
    // Aplica backoff exponencial con cap y jitter pseudoaleatorio basado en el numero de intento
    // (deterministico pero suficiente disperso para evitar sincronizacion, por recomendación de
    // flop).
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        // backoff exponencial = base * multiplicador^intento
        let exp = self.multiplier.powi(attempt as i32);
        let base_ms = self.base_delay.as_millis() as f64;
        let backoff_ms = (base_ms * exp) as u64;

        // Jitter: Meto un XOR simple del intento con un primo para dispersión sin rand
        let jitter_range_ms = self.jitter_max.as_millis() as u64;
        let jitter_ms = if jitter_range_ms > 0 {
            // Para esto uso una combinacion de bits del intento para simular aleatoriedad sin
            // añadir la dependencia de rand. Imprescindible porque los agentes tienen distintos
            // tiempos de arranque (evidentemente)
            let pseudo = attempt.wrapping_mul(2654435761) ^ attempt.wrapping_add(1) as u64;

            pseudo % jitter_range_ms
        } else {
            0
        };

        let total_ms = backoff_ms.saturating_add(jitter_ms);
        let capped_ms = total_ms.min(self.max_delay.as_millis() as u64);

        Duration::from_millis(capped_ms)
    }

    // Devuelve true si el codigo de estado gRPC indica un error transitorio sobre el que tiene
    // sentido reintentar. Los errores permanentes se propagan inmediatamente sin reintentar.
    pub fn is_retryable(code: Code) -> bool {
        matches!(
            code,
            Code::Unaviable // no hay conexion
            | Code::DeadlineExceeded // timeout de la llamada
            | Code::ResourceExhausted // rate limit
            | Code::Aborted // conflicto transitorio, reintentar
            | Code::Internal // error interno del servidor (puede ser transitorio)
            )
    }

    // Ejecuta f con reintentos segun la politica configurada
    // Reintenta solo si el error es un `tonic::Status` con codigo retryable.
    // Cualquier otro ejemplo de error se propaga inmediatamente
    pub async fn execute<F, Fut, T>(&self, mut f: F) -> Result<T, tonic::Status> where
        F: FnMut() -> Fut, Fut: std::future::Future<Output = Result<T, tonic::Status>>, {
            let mut last_error = None;

            for attempt in 0..self.max_attempts {
                match f().await {
                    Ok(value) => {
                        if attempt > 0 {
                            tracing::info!(attempt, "Llamada gRPC exitosa tras reintento");
                        }
                        return Ok(value);
                    }

                    Err(status) if !Self::is_retryable(status.code()) => {
                        // No reintentar porque es error permantnet
                        tracing::debug!(
                            code = ?status.code(),
                            message = status.message(),
                            "Error gRPC no retryable, propagando.."
                            );

                        return Err(status);
                    }
                    Err(status) => {
                        let remaining = self.max_attempts - attempt - 1;

                        if remaining == 0 {
                            // ultimo intento fallido
                            last_error = Some(status);
                            break;
                        }

                        let delay = self.delay_for_attempt(attempt);
                        tracing::warn!(
                            attempt = attempt + 1,
                            max_attempts = self.max_attempts,
                            delay_ms = delay.as_millis(),
                            code = ?status.code(),
                            message = status.message(),
                            "Error gRPC transitorio, reintentando..."
                            );

                        tokio::time::sleep(delay).await;
                        last_error = Some(status);
                    }
                }
            }

            Err(last_error.unwrap_or_else(|| {
                tonic::Status::internal("Politica de reintentos agotada sin error registrado")
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
 
    #[test]
    fn delay_grows_exponentially() {
        let policy = RetryPolicy {
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(300),
            multiplier: 2.0,
            jitter_max: Duration::ZERO, // sin jitter para que el test sea determinístico
            max_attempts: 5,
        };
 
        // Sin jitter, los delays deben ser exactamente base * 2^n
        assert_eq!(policy.delay_for_attempt(0).as_secs(), 1);
        assert_eq!(policy.delay_for_attempt(1).as_secs(), 2);
        assert_eq!(policy.delay_for_attempt(2).as_secs(), 4);
        assert_eq!(policy.delay_for_attempt(3).as_secs(), 8);
    }
 
    #[test]
    fn delay_is_capped_at_max() {
        let policy = RetryPolicy {
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(10),
            multiplier: 2.0,
            jitter_max: Duration::ZERO,
            max_attempts: 10,
        };
 
        // A partir del intento 4 (1 * 2^4 = 16s) el cap de 10s debe aplicarse
        assert!(policy.delay_for_attempt(4).as_secs() <= 10);
        assert!(policy.delay_for_attempt(8).as_secs() <= 10);
    }
 
    #[test]
    fn unavailable_is_retryable() {
        assert!(RetryPolicy::is_retryable(Code::Unavailable));
        assert!(RetryPolicy::is_retryable(Code::DeadlineExceeded));
        assert!(RetryPolicy::is_retryable(Code::ResourceExhausted));
    }
 
    #[test]
    fn auth_errors_are_not_retryable() {
        assert!(!RetryPolicy::is_retryable(Code::Unauthenticated));
        assert!(!RetryPolicy::is_retryable(Code::PermissionDenied));
        assert!(!RetryPolicy::is_retryable(Code::NotFound));
        assert!(!RetryPolicy::is_retryable(Code::InvalidArgument));
    }
 
    #[tokio::test]
    async fn execute_returns_ok_on_first_success() {
        let policy = RetryPolicy::default();
        let result = policy.execute(|| async { Ok::<i32, tonic::Status>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }
 
    #[tokio::test]
    async fn execute_propagates_non_retryable_immediately() {
        let policy = RetryPolicy::default();
        let mut calls = 0u32;
 
        let result = policy
            .execute(|| {
                calls += 1;
                async { Err::<(), _>(tonic::Status::unauthenticated("token inválido")) }
            })
            .await;
 
        assert!(result.is_err());
        assert_eq!(calls, 1, "solo debe haberse llamado una vez para error no retryable");
    }
 
    #[tokio::test]
    async fn execute_retries_on_unavailable() {
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(1), // rápido para tests
            max_delay: Duration::from_millis(10),
            multiplier: 2.0,
            jitter_max: Duration::ZERO,
        };
        let mut calls = 0u32;
 
        let result = policy
            .execute(|| {
                calls += 1;
                async move {
                    if calls < 3 {
                        Err::<(), _>(tonic::Status::unavailable("servidor caído"))
                    } else {
                        Ok(())
                    }
                }
            })
            .await;
 
        assert!(result.is_ok());
        assert_eq!(calls, 3);
    }
}
