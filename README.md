# Complyx Agent

Agente de gestión de cumplimiento normativo para endpoints Linux.
Ejecuta checks de seguridad definidos en políticas,
envía los resultados al servidor central y
aplica remediaciones automáticas cuando está configurado.

## Índice

- [Requisitos](#requisitos)
- [Compilación para desarrollo](#compilación-para-desarrollo)
- [Compilación para producción](#compilación-para-producción)
- [Empaquetado](#empaquetado)
- [Instalación en producción](#instalación-en-producción)
- [Registro](#registro)
- [Configuración](#configuración)
- [Operación](#operación)
- [Tests](#tests)

---

## Requisitos

**Para compilar:**

- Rust 1.78+ con cargo ([instalación oficial](https://rust-lang.org/tools/install/))
- `protoc` — compilador de Protocol Buffers
- `sqlx-cli` — herramienta de migraciones de base de datos

```bash
# protoc
sudo apt install protobuf-compiler      # Debian/Ubuntu
sudo dnf install protobuf-compiler      # RHEL/Fedora

# sqlx-cli
cargo install sqlx-cli --no-default-features --features sqlite
```

**Para empaquetar:**

```bash
sudo apt install dpkg-dev       # .deb
sudo dnf install rpm-build      # .rpm
# Arch: makepkg está incluido en base-devel
```

---

## Compilación para desarrollo

Clona el repositorio:

```bash
git clone https://github.com/styxiner/complyx-agent.git
cd complyx-agent
```

Prepara la base de datos local
(necesario para que `sqlx` verifique las queries en compilación):

```bash
sqlx database create --database-url sqlite:./complyx-agent.db
sqlx migrate run \
    --database-url sqlite:./complyx-agent.db \
    --source crates/local-db/src/migrations
DATABASE_URL="sqlite:./complyx-agent.db" cargo sqlx prepare --workspace
```

Compila en modo debug:

```bash
DATABASE_URL="sqlite:./complyx-agent.db" cargo build
```

El binario queda en `target/debug/complyx-agent`.

---

## Compilación para producción

Para producción se usa el target `musl` que genera un **binario estático**
sin dependencias de glibc, compatible con cualquier distribución Linux:

```bash
# Añadir el target musl si no está instalado
rustup target add x86_64-unknown-linux-musl

# En Debian/Ubuntu, musl-tools proporciona el linker
sudo apt install musl-tools

# En Fedora/RHEL:
sudo dnf install musl-gcc musl-devel musl-libc-static

# Compilar con el perfil optimizado para tamaño
DATABASE_URL="sqlite:./complyx-agent.db" \
cargo build --profile dist --target x86_64-unknown-linux-musl --bin complyx-agent
```

El binario queda en `target/x86_64-unknown-linux-musl/dist/complyx-agent`.

### Reducción opcional del tamaño con UPX

```bash
# Instalar upx
sudo apt install upx
sudo dnf install upx

# Comprimir el binario (~50-60% de reducción)
upx --best --lzma target/x86_64-unknown-linux-musl/dist/complyx-agent
```

---

## Empaquetado

El script `packaging/build-packages.sh` compila el binario
y genera los tres formatos de paquete en un solo paso:

```bash
cd packaging
./build-packages.sh --version 0.1.0
```

Los artefactos se generan en `packaging/build/`:

```
packaging/build/
├── deb/
│   └── complyx-agent_0.1.0_amd64.deb
├── rpm/
│   └── RPMS/x86_64/complyx-agent-0.1.0-1.x86_64.rpm
└── arch/
    └── PKGBUILD
```

Para compilar solo para una distribución concreta:

```bash
./build-packages.sh --version 0.1.0   # genera los tres formatos
```

El script detecta automáticamente qué herramientas están disponibles
(`dpkg-deb`, `rpmbuild`) y genera solo los paquetes que puede construir.

---

## Instalación en producción

### Debian / Ubuntu

```bash
sudo dpkg -i complyx-agent_0.1.0_amd64.deb
```

### RHEL / Fedora / Rocky

```bash
sudo rpm -i complyx-agent-0.1.0-1.x86_64.rpm
# o con dnf para gestión automática de dependencias:
sudo dnf install complyx-agent-0.1.0-1.x86_64.rpm
```

### Arch Linux / Manjaro

```bash
cd packaging/build/arch
makepkg -si
```

Todos los paquetes crean automáticamente el usuario y grupo `complyx`,
los directorios necesarios y registran el servicio systemd.
**El servicio no arranca automáticamente** tras la instalación —
es necesario configurar el servidor y registrar el agente primero.

---

## Registro

El registro es el proceso por el que el agente obtiene su certificado de cliente
para autenticarse con el servidor mediante mTLS. Ocurre una única vez.

**1. Genera un token de registro en el servidor:**

```bash
# En el servidor complyx-server
complyx-server enroll-token --hostname nombre-del-endpoint
# → token: a3f8c2d1e9b04f7a...
```

**2. Configura la URL del servidor en el agente:**

Edita `/etc/complyx/agent.toml` en el endpoint que vas a registrar:

```toml
server_url = "https://tu-servidor-complyx:9000"
enroll_url = "https://tu-servidor-complyx:9001"
```

**3. Arranca el agente pasando el token:**

```bash
COMPLYX_ENROLL_TOKEN=a3f8c2d1e9b04f7a... systemctl start complyx-agent
```

El agente genera un par de claves Ed25519, envía el CSR al servidor,
recibe su certificado firmado por la CA interna y lo guarda en `/var/lib/complyx/certs/`.
A partir de este momento el token queda invalidado y
el agente arranca directamente sin necesitar token.

**4. Verifica el registro:**

```bash
systemctl status complyx-agent
journalctl -u complyx-agent -n 50
```

Deberías ver algo similar a:

```
complyx-agent[1234]: agente ya registrado, usando certificados existentes
complyx-agent[1234]: scheduler arrancando
complyx-agent[1234]: ejecutando poll inicial...
```

---

## Configuración

El fichero de configuración principal es `/etc/complyx/agent.toml`.
Cualquier valor puede sobreescribirse con una variable de entorno
con el prefijo `COMPLYX_`:

```toml
# /etc/complyx/agent.toml

# --- Obligatorio ---
server_url = "https://tu-servidor:9000"   # gRPC con mTLS
enroll_url = "https://tu-servidor:9001"   # Solo para el enrolamiento inicial

# --- Rutas (defecto recomendado) ---
cert_dir = "/var/lib/complyx/certs"       # agent.crt, agent.key, ca.crt
db_path  = "/var/lib/complyx/agent.db"   # SQLite local

# --- Timings ---
poll_interval_secs      = 300    # Cada cuánto obtiene políticas del servidor
flush_interval_secs     = 60     # Cada cuánto envía resultados pendientes
heartbeat_interval_secs = 120    # Cada cuánto hace ping al servidor
result_purge_days       = 7      # Días que conserva resultados ya enviados

# --- Comportamiento ---
auto_remediate = true    # false = solo auditoría, sin aplicar cambios

# --- Logging ---
log_level  = "info"      # error | warn | info | debug | trace
log_format = "json"      # json (producción) | pretty (desarrollo)
```

### Variables de entorno

Todas las claves del TOML son accesibles como variables de entorno. Ejemplos:

|Variable|Equivalente en TOML|
|--------|-------------------|
|`COMPLYX_SERVER_URL`|`server_url`|
|`COMPLYX_ENROLL_URL`|`enroll_url`|
|`COMPLYX_ENROLL_TOKEN`|`enroll_token` (solo para el primer arranque)|
|`COMPLYX_LOG_LEVEL`|`log_level`|
|`COMPLYX_AUTO_REMEDIATE`|`auto_remediate`|
|`COMPLYX_CONFIG_PATH`|Ruta al fichero de configuración (defecto: `/etc/complyx/agent.toml`)|

Las variables de entorno tienen **mayor precedencia** que el fichero TOML.

---

## Operación

### Gestión del servicio

```bash
# Estado
systemctl status complyx-agent

# Logs en tiempo real
journalctl -u complyx-agent -f

# Logs de la última hora
journalctl -u complyx-agent --since "1 hour ago"

# Reiniciar (por ejemplo, tras cambiar la configuración)
systemctl restart complyx-agent

# Deshabilitar remediaciones sin reiniciar
# (editar /etc/complyx/agent.toml y cambiar auto_remediate = false, luego restart)
```

### Directorios relevantes

|Ruta|Contenido|
|------|-------|
|`/etc/complyx/agent.toml`|Configuración del agente|
|`/var/lib/complyx/certs/agent.crt`|Certificado del agente|
|`/var/lib/complyx/certs/agent.key`|Clave privada (permisos 0600)|
|`/var/lib/complyx/certs/ca.crt`|Certificado raíz de la CA del servidor|
|`/var/lib/complyx/agent.db`|Base de datos SQLite local|
|`/etc/sysctl.d/99-complyx.conf`|Parámetros de kernel aplicados por remediaciones|

### Re-registro

Si los certificados se corrompen o el servidor los revoca,
puedes re-registrar el agente:

```bash
# Eliminar los certificados actuales
rm -rf /var/lib/complyx/certs/*

# Generar un nuevo token en el servidor y re-registrar
COMPLYX_ENROLL_TOKEN=nuevo-token systemctl restart complyx-agent
```

### Desinstalación

```bash
# Debian — conserva /var/lib/complyx (datos y certificados)
sudo apt remove complyx-agent

# Debian — elimina todo incluyendo datos y usuario del sistema
sudo apt purge complyx-agent

# RPM
sudo rpm -e complyx-agent
# o: sudo dnf remove complyx-agent

# Arch
sudo pacman -R complyx-agent
```

---

## Tests

Ejecuta todos los tests del workspace:

```bash
DATABASE_URL="sqlite:./complyx-agent.db" cargo test
```

Tests de un crate específico:

```bash
DATABASE_URL="sqlite:./complyx-agent.db" cargo test -p policy-engine
DATABASE_URL="sqlite:./complyx-agent.db" cargo test -p local-db
DATABASE_URL="sqlite:./complyx-agent.db" cargo test -p remediation-engine
```

Tests con output en caso de fallo (útil para debug):

```bash
DATABASE_URL="sqlite:./complyx-agent.db" cargo test -- --nocapture
```
