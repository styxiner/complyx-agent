//! Check `user_attr`: verifica atributos de una cuenta de usuario del sistema.
//!
//! Lee directamente `/etc/passwd`, `/etc/shadow` y `/etc/group` sin ejecutar comandos
//! externos. Esto lo hace seguro (no hay inyección de comandos posible) y funciona
//! aunque `getent` o `id` no estén disponibles.
//!
//! ## Atributos disponibles
//!
//! | Atributo | Fuente | Descripción |
//! |---|---|---|
//! | `shell` | `/etc/passwd` | Shell de login del usuario |
//! | `home` | `/etc/passwd` | Directorio home |
//! | `uid` | `/etc/passwd` | UID numérico |
//! | `gid` | `/etc/passwd` | GID principal numérico |
//! | `groups` | `/etc/group` | Grupos a los que pertenece (separados por coma) |
//! | `password_max_age` | `/etc/shadow` | Días máximos de validez de la contraseña |
//! | `password_min_age` | `/etc/shadow` | Días mínimos entre cambios |
//! | `password_status` | `/etc/shadow` | Estado: `locked`, `no_password`, `set` |
//!
//! ## Ejemplo de uso
//!
//! ```json
//! {
//!   "type": "user_attr",
//!   "username": "root",
//!   "checks": [
//!     { "attr": "shell", "operator": "=", "value": "/bin/bash" },
//!     { "attr": "password_max_age", "operator": "<=", "value": "90" }
//!   ]
//! }
//! ```

use async_trait::async_trait;
use serde::Deserialize;

use crate::executor::{CheckExecutor, CompareOperator};
use crate::result::{CheckError, EngineCheckResult};

pub struct UserAttrExecutor;

/// Un check individual sobre un atributo del usuario.
#[derive(Deserialize)]
struct AttrCheck {
    attr: String,
    operator: CompareOperator,
    value: String,
}

#[derive(Deserialize)]
struct Params {
    /// Nombre del usuario a verificar.
    username: String,

    /// Lista de atributos a verificar. Todos deben cumplirse.
    checks: Vec<AttrCheck>,
}

/// Datos de un usuario extraídos de `/etc/passwd`.
struct PasswdEntry {
    uid: String,
    gid: String,
    home: String,
    shell: String,
}

/// Datos de un usuario extraídos de `/etc/shadow`.
struct ShadowEntry {
    /// Campo de contraseña (! = locked, * = no password, hash = set)
    password_field: String,
    /// Días mínimos entre cambios (campo 4)
    min_age: String,
    /// Días máximos de validez (campo 5)
    max_age: String,
}

#[async_trait]
impl CheckExecutor for UserAttrExecutor {
    fn check_type(&self) -> &'static str { "user_attr" }

    async fn execute(
        &self,
        check_id: &str,
        params: &serde_json::Value,
    ) -> Result<EngineCheckResult, CheckError> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| CheckError::invalid_params("user_attr", e))?;

        if p.checks.is_empty() {
            return Err(CheckError::Internal {
                check_type: "user_attr".into(),
                reason: "debe especificar al menos un check de atributo".into(),
            });
        }

        // Leer /etc/passwd
        let passwd_content = tokio::fs::read_to_string("/etc/passwd")
            .await
            .map_err(|e| CheckError::io("/etc/passwd", e))?;

        let passwd = parse_passwd(&passwd_content, &p.username).ok_or_else(|| {
            CheckError::Internal {
                check_type: "user_attr".into(),
                reason: format!("usuario '{}' no encontrado en /etc/passwd", p.username),
            }
        })?;

        // Leer /etc/shadow (puede fallar si el agente no tiene permisos — no es error crítico)
        let shadow = read_shadow(&p.username).await;

        // Leer /etc/group para los grupos del usuario
        let groups = read_user_groups(&passwd.gid, &p.username).await;

        // Ejecutar cada check de atributo
        for attr_check in &p.checks {
            let actual = resolve_attr(
                &attr_check.attr,
                &passwd,
                shadow.as_ref(),
                groups.as_deref(),
            ).ok_or_else(|| CheckError::Internal {
                check_type: "user_attr".into(),
                reason: format!(
                    "atributo '{}' no disponible para usuario '{}'",
                    attr_check.attr, p.username
                ),
            })?;

            let passed = attr_check.operator.compare(&actual, &attr_check.value);
            if !passed {
                let expected_str = format!("{} {}", attr_check.operator, attr_check.value);
                let detail = EngineCheckResult::value_detail(
                    &format!("{}:{}", p.username, attr_check.attr),
                    &actual,
                    &attr_check.operator,
                    &attr_check.value,
                    false,
                );
                return Ok(EngineCheckResult::fail(check_id, &actual, &expected_str, detail));
            }
        }

        Ok(EngineCheckResult::pass(
            check_id,
            "conforme",
            "conforme",
            format!(
                "usuario '{}': {} atributo(s) verificado(s) correctamente",
                p.username,
                p.checks.len()
            ),
        ))
    }
}

/// Resuelve el valor de un atributo del usuario.
fn resolve_attr(
    attr: &str,
    passwd: &PasswdEntry,
    shadow: Option<&ShadowEntry>,
    groups: Option<&str>,
) -> Option<String> {
    match attr {
        "shell" => Some(passwd.shell.clone()),
        "home" => Some(passwd.home.clone()),
        "uid" => Some(passwd.uid.clone()),
        "gid" => Some(passwd.gid.clone()),
        "groups" => groups.map(|g| g.to_string()),
        "password_max_age" => shadow.map(|s| s.max_age.clone()),
        "password_min_age" => shadow.map(|s| s.min_age.clone()),
        "password_status" => shadow.map(|s| {
            let field = &s.password_field;
            if field.starts_with('!') || field.starts_with('*') {
                if field == "*" || field == "!!" {
                    "no_password".to_string()
                } else {
                    "locked".to_string()
                }
            } else if field.is_empty() {
                "no_password".to_string()
            } else {
                "set".to_string()
            }
        }),
        _ => None,
    }
}

/// Parsea la línea del usuario en `/etc/passwd`.
/// Formato: `username:password:uid:gid:gecos:home:shell`
fn parse_passwd(content: &str, username: &str) -> Option<PasswdEntry> {
    for line in content.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() >= 7 && fields[0] == username {
            return Some(PasswdEntry {
                uid: fields[2].to_string(),
                gid: fields[3].to_string(),
                home: fields[5].to_string(),
                shell: fields[6].to_string(),
            });
        }
    }
    None
}

/// Lee y parsea la entrada de `/etc/shadow` para el usuario dado.
/// Devuelve `None` si no hay acceso (el agente no tiene permisos suficientes).
/// Formato: `username:password:lastchg:min:max:warn:inactive:expire:reserved`
async fn read_shadow(username: &str) -> Option<ShadowEntry> {
    let content = tokio::fs::read_to_string("/etc/shadow").await.ok()?;
    for line in content.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() >= 9 && fields[0] == username {
            return Some(ShadowEntry {
                password_field: fields[1].to_string(),
                min_age: fields[3].to_string(),
                max_age: fields[4].to_string(),
            });
        }
    }
    None
}

/// Lee `/etc/group` y devuelve los grupos del usuario separados por coma.
/// Incluye el grupo principal (por GID) y los grupos suplementarios.
async fn read_user_groups(primary_gid: &str, username: &str) -> Option<String> {
    let content = tokio::fs::read_to_string("/etc/group").await.ok()?;
    let mut groups = Vec::new();

    for line in content.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() < 4 {
            continue;
        }
        let group_name = fields[0];
        let gid = fields[2];
        let members = fields[3];

        // Grupo principal (coincide por GID)
        if gid == primary_gid && !groups.contains(&group_name.to_string()) {
            groups.push(group_name.to_string());
        }

        // Grupos suplementarios (el username aparece en la lista de miembros)
        if members.split(',').any(|m| m.trim() == username) {
            if !groups.contains(&group_name.to_string()) {
                groups.push(group_name.to_string());
            }
        }
    }

    if groups.is_empty() {
        None
    } else {
        Some(groups.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_passwd_root() {
        let content = "root:x:0:0:root:/root:/bin/bash\ndaemon:x:1:1:daemon:/usr/sbin:/usr/sbin/nologin\n";
        let entry = parse_passwd(content, "root").unwrap();
        assert_eq!(entry.uid, "0");
        assert_eq!(entry.shell, "/bin/bash");
        assert_eq!(entry.home, "/root");
    }

    #[test]
    fn parse_passwd_missing_user() {
        let content = "nobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin\n";
        assert!(parse_passwd(content, "ghost").is_none());
    }

    #[test]
    fn resolve_attr_shell() {
        let passwd = PasswdEntry {
            uid: "0".into(),
            gid: "0".into(),
            home: "/root".into(),
            shell: "/bin/bash".into(),
        };
        assert_eq!(resolve_attr("shell", &passwd, None, None), Some("/bin/bash".into()));
        assert_eq!(resolve_attr("uid", &passwd, None, None), Some("0".into()));
    }

    #[test]
    fn resolve_password_status_locked() {
        let passwd = PasswdEntry { uid: "0".into(), gid: "0".into(), home: "/".into(), shell: "/bin/bash".into() };
        let shadow = ShadowEntry {
            password_field: "!$6$hash".into(),
            min_age: "0".into(),
            max_age: "90".into(),
        };
        assert_eq!(
            resolve_attr("password_status", &passwd, Some(&shadow), None),
            Some("locked".into())
        );
    }

    #[test]
    fn resolve_password_status_set() {
        let passwd = PasswdEntry { uid: "0".into(), gid: "0".into(), home: "/".into(), shell: "/bin/bash".into() };
        let shadow = ShadowEntry {
            password_field: "$6$somehash".into(),
            min_age: "0".into(),
            max_age: "90".into(),
        };
        assert_eq!(
            resolve_attr("password_status", &passwd, Some(&shadow), None),
            Some("set".into())
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn checks_root_user_exists() {
        use serde_json::json;
        let result = UserAttrExecutor
            .execute("chk-1", &json!({
                "username": "root",
                "checks": [
                    { "attr": "uid", "operator": "=", "value": "0" }
                ]
            }))
            .await
            .unwrap();

        assert!(result.passed, "root debe tener uid=0: {}", result.detail);
    }
}
