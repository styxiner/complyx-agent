-- Almacena el ultimo PolicyBundle recibido del servidor serialziado como JSON.
-- Solo existe una fila a la vez. La tabla usa una pk fija para hacer el upsert atómico y simple.

CREATE TABLE IF NOT EXISTS policy_bundle(
  key TEXT NOT NULL PRIMARY KEY CHECK (key = 'current'), -- Siempre current. Garantiza que solo haya una fila.
  hash TEXT NOT NULL, -- hash SHA-256 del bundle. Se envia al server en cada PollRequest para evitar retransmitir el bundle si no ha cambiado
  data_json TEXT NOT NULL, -- Bundle completo serializado como JSON, se usa cuando el agente trabaja sin conexion
  cached_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ')) -- Cuando se recibe y almacena el bundle
);
