-- Los resultados de checks se insertan aquí inmediatamente después de ejecutarse.
-- El result_flush los drena y los envía al servidor cuando hay conectividad.
-- Esto garantiza que ningún resultado se pierda aunque el servidor no sea alcanzable.

CREATE TABLE IF NOT EXISTS result_queue (
  id TEXT NOT NULL PRIMARY KEY, -- UUID del resultado en la cola (distinto del check_id).
  check_id TEXT NOT NULL, -- UUID del PolicyCheck cuyo resultado es este.
  data_json TEXT NOT NULL, -- Resultado completo serializado como JSON (proto::CheckResult). Incluye passed, detail, actual_value, expected_value y executed_at.

  -- Estado del resultado en la cola:
  --   pending  → pendiente de enviar al servidor
  --   sending  → siendo enviado en este momento (evita doble envío)
  --   sent     → enviado y confirmado por el servidor
  --   failed   → el servidor lo rechazó (ver error_detail)
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'sending', 'sent', 'failed')),

  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')), -- Cuándo se insertó en la cola (executed_at del check).

  sent_at TEXT, -- Cuándo se envió con éxito al servidor. NULL hasta que status = 'sent'.

  -- Número de veces que se ha intentado enviar al servidor.
  -- El flush no reintenta indefinidamente resultados con retries > MAX_RETRIES.
  retries         INTEGER NOT NULL DEFAULT 0,

  error_detail TEXT -- Mensaje de error del servidor si status = 'failed'.
);

-- Índice para el drain_pending: solo nos interesan los 'pending' ordenados por antigüedad.
CREATE INDEX IF NOT EXISTS idx_result_queue_pending ON result_queue (status, created_at) WHERE status = 'pending';

-- Índice para el mark_sent / mark_failed: búsqueda por ID de lote.
CREATE INDEX IF NOT EXISTS idx_result_queue_check_id ON result_queue (check_id);
