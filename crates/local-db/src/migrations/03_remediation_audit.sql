-- El remediation-engine escribe aquí antes y después de aplicar cada remediación.
-- Permite saber qué cambios hizo el agente en el sistema, cuándo y con qué resultado.
-- Esta tabla es solo append: nunca se modifican ni eliminan filas.

CREATE TABLE IF NOT EXISTS remediation_audit (
    id TEXT NOT NULL PRIMARY KEY, -- UUID de la entrada de auditoría
    check_id TEXT NOT NULL, -- UUID del PolicyCheck que originó la remediación.
    remediation_id TEXT NOT NULL, -- UUID de la PolicyRemediation aplicada.
    remediation_type TEXT NOT NULL, -- Tipo de remediación (ej. "file_line_set", "pkg_install").
    params_json TEXT NOT NULL, -- Parámetros usados, serializados como JSON. Para auditoría forense.

    -- Estado:
    --   pending   → se va a aplicar (escrito antes de ejecutar, para detectar crashes)
    --   applied   → aplicada con éxito
    --   failed    → falló la aplicación
    --   skipped   → se decidió no aplicar (ej. sin permisos o modo dry-run)
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'applied', 'failed', 'skipped')),

    result_detail TEXT, -- Descripción del resultado: qué cambió exactamente, o el error si falló.
    started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')), -- Cuándo se registró la intención de remediar.
    finished_at TEXT -- Cuándo finalizó (applied, failed o skipped). NULL si status = 'pending'.
);

CREATE INDEX IF NOT EXISTS idx_remediation_audit_check_id ON remediation_audit (check_id);

CREATE INDEX IF NOT EXISTS idx_remediation_audit_status ON remediation_audit (status, started_at);
