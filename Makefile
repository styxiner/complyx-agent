# =============================================================================
# complyx-agent — Makefile de distribución
# =============================================================================
#
# Flujo de instalación en producción:
#
#   1. El administrador copia ca.crt al endpoint antes de instalar el paquete:
#        scp ca.crt root@endpoint:/var/lib/complyx/certs/ca.crt
#
#   2. Se instala el paquete:
#        dpkg -i complyx-agent_*.deb
#        rpm -i complyx-agent-*.rpm
#        makepkg -si   (Arch)
#
#   3. Se arranca el agente pasando el token de enrolamiento:
#        COMPLYX_ENROLL_TOKEN=<token> systemctl start complyx-agent
#        (o se persiste en /etc/complyx/agent.toml antes del primer start)
#
# Targets principales:
#   make build              Compilar en modo release (glibc, desarrollo)
#   make build-static       Compilar estático con musl (producción)
#   make deb                Generar paquete .deb
#   make rpm                Generar paquete .rpm
#   make arch               Preparar PKGBUILD para Arch Linux
#   make all-packages       Generar todos los paquetes disponibles
#   make scripts            Generar scripts de mantenedor (postinst, prerm…)
#   make clean              Limpiar artefactos de build
#   make install-tools      Instalar herramientas de empaquetado necesarias
#   make prepare-db         Preparar BD SQLite para compilación offline de sqlx
#   make check / test       Verificar y testear el workspace

# 1. Instalar herramientas (solo la primera vez)
#	make install-tools

# 2. Preparar la BD para sqlx (compile-time checks)
#	make prepare-db

# 3. Verificar que el workspace compila
#	make check

# 4. Compilar el binario estático (producción)
#	make build-static
# 	→ target/x86_64-unknown-linux-musl/dist/complyx-agent

# 5. Generar scripts de mantenedor + servicio systemd + config
#	make scripts
# 	→ packaging/scripts/{postinst,prerm,postrm,rpm-*.sh}
# 	→ packaging/systemd/complyx-agent.service
# 	→ packaging/config/agent.toml.example

# 6a. Generar paquete .deb
#	make deb

# 6b. Generar paquete .rpm
#	make rpm

# 6c. Generar todos los paquetes de golpe
#	make all-packages


VERSION     := $(shell grep '^version' crates/agent-core/Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
BINARY      := complyx-agent
TARGET_MUSL := x86_64-unknown-linux-musl

DIST_DIR        := dist
SCRIPTS_DIR     := packaging/scripts
SYSTEMD_DIR     := packaging/systemd
CONFIG_DIR      := packaging/config
ARCH_PKG_DIR    := packaging/arch

DATABASE_URL ?= sqlite:./complyx-agent.db

# Directorios que crean los paquetes en el sistema destino
CERT_DIR   := /var/lib/complyx/certs
DATA_DIR   := /var/lib/complyx
CONFIG_PATH := /etc/complyx
BINARY_PATH := /usr/bin

.PHONY: all help build build-static deb rpm arch all-packages \
        scripts clean install-tools prepare-db check test \
        _check-version \
        _script-postinst-debian _script-prerm-debian _script-postrm-debian \
        _script-rpm-pre _script-rpm-post _script-rpm-preun _script-rpm-postun \
        _script-arch-install _systemd-service _default-config

# ---------------------------------------------------------------------------
# Default → ayuda
# ---------------------------------------------------------------------------

all: help

## Mostrar esta ayuda
help:
	@echo ""
	@echo "complyx-agent v$(VERSION) — Makefile de distribución"
	@echo ""
	@echo "USO: make <target> [DATABASE_URL=sqlite:./mi.db]"
	@echo ""
	@echo "COMPILACIÓN"
	@echo "  build              Compilar en modo release con glibc (desarrollo/tests)"
	@echo "  build-static       Compilar binario estático con musl (producción)"
	@echo "  prepare-db         Preparar BD SQLite para sqlx compile-time checks"
	@echo ""
	@echo "TESTS"
	@echo "  check              cargo check en todo el workspace"
	@echo "  test               cargo test en todo el workspace"
	@echo ""
	@echo "SCRIPTS DE MANTENEDOR"
	@echo "  scripts            Generar postinst/prerm/postrm (Debian), rpm-*.sh (RPM)"
	@echo "                     y complyx-agent.install (Arch) en packaging/scripts/"
	@echo ""
	@echo "EMPAQUETADO"
	@echo "  deb                Generar paquete .deb (Debian/Ubuntu)"
	@echo "  rpm                Generar paquete .rpm (RHEL/Fedora/Rocky)"
	@echo "  arch               Preparar PKGBUILD para Arch Linux en dist/arch/"
	@echo "  all-packages       Generar todos los paquetes disponibles"
	@echo ""
	@echo "HERRAMIENTAS"
	@echo "  install-tools      Instalar cargo-deb, cargo-generate-rpm, sqlx-cli"
	@echo "                     y el target musl según la distribución del host"
	@echo "  clean              Eliminar artefactos de build y dist/"
	@echo ""
	@echo "VARIABLES"
	@echo "  DATABASE_URL       URL SQLite para sqlx (por defecto: sqlite:./complyx-agent.db)"
	@echo ""
	@echo "FLUJO DE INSTALACIÓN EN PRODUCCIÓN"
	@echo "  1. Copiar la CA al endpoint antes (o después) de instalar el paquete:"
	@echo "       install -m 640 -o complyx -g complyx ca.crt /var/lib/complyx/certs/ca.crt"
	@echo "  2. Instalar el paquete (.deb / .rpm / makepkg -si)"
	@echo "  3. Editar /etc/complyx/agent.toml (server_url, enroll_url)"
	@echo "  4. Primer arranque con el token de enrolamiento:"
	@echo "       COMPLYX_ENROLL_TOKEN=<token> systemctl start complyx-agent"
	@echo ""

# ---------------------------------------------------------------------------
# Internos
# ---------------------------------------------------------------------------

_check-version:
	@test -n "$(VERSION)" || (echo "ERROR: no se pudo leer la versión de Cargo.toml" && exit 1)
	@echo "==> complyx-agent v$(VERSION)"

# ---------------------------------------------------------------------------
# Compilación
# ---------------------------------------------------------------------------

## Compilar en modo release con glibc (desarrollo/tests)
build:
	DATABASE_URL="$(DATABASE_URL)" cargo build --release --bin $(BINARY)
	@echo "Binario: target/release/$(BINARY)"

## Compilar binario estático con musl (producción y empaquetado)
build-static: _check-version
	@rustup target add $(TARGET_MUSL) 2>/dev/null || true
	DATABASE_URL="$(DATABASE_URL)" \
	cargo build --profile dist --target $(TARGET_MUSL) --bin $(BINARY)
	@echo "Binario estático: target/$(TARGET_MUSL)/dist/$(BINARY)"

## Preparar la BD SQLite local (necesario para sqlx compile-time checks)
prepare-db:
	@command -v sqlx >/dev/null 2>&1 || \
		cargo install sqlx-cli --no-default-features --features sqlite
	sqlx database create --database-url "$(DATABASE_URL)"
	sqlx migrate run \
		--database-url "$(DATABASE_URL)" \
		--source crates/local-db/src/migrations
	DATABASE_URL="$(DATABASE_URL)" cargo sqlx prepare --workspace

# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

check:
	DATABASE_URL="$(DATABASE_URL)" cargo check --workspace

test:
	DATABASE_URL="$(DATABASE_URL)" cargo test --workspace

# ---------------------------------------------------------------------------
# Contenido de los scripts de mantenedor (define blocks)
#
# Se usa $(file >,path,$(VAR)) en lugar de heredocs porque make ejecuta
# cada línea de receta en un shell separado, por lo que los heredocs
# multilínea nunca funcionan correctamente en recetas make.
# ---------------------------------------------------------------------------

define POSTINST_CONTENT
#!/bin/bash
set -e

CERT_DIR=/var/lib/complyx/certs
DATA_DIR=/var/lib/complyx
CONFIG_FILE=/etc/complyx/agent.toml
SERVICE=complyx-agent

# ---- 1. Usuario y grupo del sistema ----------------------------------------
if ! getent group complyx >/dev/null 2>&1; then
    groupadd --system complyx
fi
if ! getent passwd complyx >/dev/null 2>&1; then
    useradd \
        --system \
        --gid complyx \
        --home-dir "$DATA_DIR" \
        --no-create-home \
        --shell /usr/sbin/nologin \
        --comment "Complyx Security Agent" \
        complyx
fi

# ---- 2. Directorios --------------------------------------------------------
install -d -m 0750 -o complyx -g complyx "$DATA_DIR"
install -d -m 0700 -o complyx -g complyx "$CERT_DIR"
install -d -m 0755            /etc/complyx

# Asegurar propietario correcto si los directorios ya existían
chown complyx:complyx "$DATA_DIR"
chown complyx:complyx "$CERT_DIR"

# ---- 3. CA pre-provisionada ------------------------------------------------
# Si el administrador copió ca.crt al endpoint antes de instalar el paquete,
# ajustar sus permisos. Si se pasa COMPLYX_CA_PATH, copiarla ahora.
if [ -n "${COMPLYX_CA_PATH:-}" ] && [ -f "$COMPLYX_CA_PATH" ]; then
    echo "==> Copiando CA desde $COMPLYX_CA_PATH a $CERT_DIR/ca.crt"
    install -m 0640 -o complyx -g complyx "$COMPLYX_CA_PATH" "$CERT_DIR/ca.crt"
fi

if [ -f "$CERT_DIR/ca.crt" ]; then
    chown complyx:complyx "$CERT_DIR/ca.crt"
    chmod 0640 "$CERT_DIR/ca.crt"
    echo "==> CA encontrada en $CERT_DIR/ca.crt"
else
    echo ""
    echo "AVISO: No se encontró $CERT_DIR/ca.crt"
    echo "       Cópiala antes de arrancar el agente:"
    echo "         install -m 640 -o complyx -g complyx <ruta/ca.crt> $CERT_DIR/ca.crt"
    echo ""
fi

# ---- 4. Configuración por defecto ------------------------------------------
if [ ! -f "$CONFIG_FILE" ]; then
    install -m 0644 /usr/share/complyx-agent/agent.toml.example "$CONFIG_FILE"
    echo "==> Configuración de ejemplo instalada en $CONFIG_FILE"
    echo "    Edítala para configurar server_url y enroll_url antes de arrancar."
fi

# ---- 5. Servicio systemd ---------------------------------------------------
if command -v systemctl >/dev/null 2>&1 && systemctl is-system-running --quiet 2>/dev/null; then
    systemctl daemon-reload || true
    systemctl enable --now "$SERVICE" 2>/dev/null || systemctl enable "$SERVICE" || true
fi

# ---- 6. Instrucciones de enrolamiento --------------------------------------
echo ""
echo "========================================================"
echo "  complyx-agent instalado correctamente"
echo "========================================================"
echo ""
echo "PRÓXIMOS PASOS:"
echo ""
echo "  1. Edita /etc/complyx/agent.toml y configura:"
echo "       server_url = \"https://<tu-servidor>:9000\""
echo "       enroll_url = \"https://<tu-servidor>:9001\""
echo ""
echo "  2. Asegúrate de que la CA del servidor está en:"
echo "       $CERT_DIR/ca.crt"
echo "     Si no, cópiala con:"
echo "       install -m 640 -o complyx -g complyx ca.crt $CERT_DIR/ca.crt"
echo ""
echo "  3. Obtén un token de enrolamiento en el servidor:"
echo "       complyx-server enroll-token --hostname $(hostname)"
echo ""
echo "  4. Arranca el agente con el token:"
echo "       COMPLYX_ENROLL_TOKEN=<token> systemctl start complyx-agent"
echo ""
echo "     O persiste el token en el fichero de configuración:"
echo "       echo 'enroll_token = \"<token>\"' >> /etc/complyx/agent.toml"
echo "       systemctl start complyx-agent"
echo "     (El token se usará una sola vez; puedes eliminarlo después)"
echo ""
#DEBHELPER#
endef

define PRERM_CONTENT
#!/bin/bash
set -e

SERVICE=complyx-agent

if command -v systemctl >/dev/null 2>&1; then
    if systemctl is-active --quiet "$SERVICE" 2>/dev/null; then
        systemctl stop "$SERVICE" || true
    fi
    systemctl disable "$SERVICE" 2>/dev/null || true
fi
#DEBHELPER#
endef

define POSTRM_CONTENT
#!/bin/bash
set -e

CERT_DIR=/var/lib/complyx/certs
DATA_DIR=/var/lib/complyx

case "$1" in
    purge)
        echo "==> Eliminando datos del agente (purge)..."
        rm -rf "$CERT_DIR" "$DATA_DIR" /etc/complyx /etc/sysctl.d/99-complyx.conf
        if getent passwd complyx >/dev/null 2>&1; then
            userdel complyx || true
        fi
        if getent group complyx >/dev/null 2>&1; then
            groupdel complyx 2>/dev/null || true
        fi
        if command -v systemctl >/dev/null 2>&1; then
            systemctl daemon-reload || true
        fi
        ;;
    remove)
        if command -v systemctl >/dev/null 2>&1; then
            systemctl daemon-reload || true
        fi
        ;;
esac
#DEBHELPER#
endef

define RPM_PRE_CONTENT
#!/bin/bash
# Ejecutado antes de instalar el paquete RPM (%pre)

if ! getent group complyx >/dev/null 2>&1; then
    groupadd --system complyx
fi
if ! getent passwd complyx >/dev/null 2>&1; then
    useradd \
        --system \
        --gid complyx \
        --home-dir /var/lib/complyx \
        --no-create-home \
        --shell /sbin/nologin \
        --comment "Complyx Security Agent" \
        complyx
fi
endef

define RPM_POST_CONTENT
#!/bin/bash
# Ejecutado después de instalar el paquete RPM (%post)

CERT_DIR=/var/lib/complyx/certs
DATA_DIR=/var/lib/complyx
CONFIG_FILE=/etc/complyx/agent.toml
SERVICE=complyx-agent

install -d -m 0750 -o complyx -g complyx "$DATA_DIR"
install -d -m 0700 -o complyx -g complyx "$CERT_DIR"
install -d -m 0755            /etc/complyx

if [ -n "${COMPLYX_CA_PATH:-}" ] && [ -f "$COMPLYX_CA_PATH" ]; then
    echo "==> Copiando CA desde $COMPLYX_CA_PATH a $CERT_DIR/ca.crt"
    install -m 0640 -o complyx -g complyx "$COMPLYX_CA_PATH" "$CERT_DIR/ca.crt"
fi

if [ -f "$CERT_DIR/ca.crt" ]; then
    chown complyx:complyx "$CERT_DIR/ca.crt"
    chmod 0640 "$CERT_DIR/ca.crt"
fi

if [ ! -f "$CONFIG_FILE" ]; then
    install -m 0644 /usr/share/complyx-agent/agent.toml.example "$CONFIG_FILE"
fi

if [ $1 -eq 1 ]; then
    # Primera instalación
    systemctl daemon-reload || true
    systemctl enable "$SERVICE" || true
    echo ""
    echo "========================================================"
    echo "  complyx-agent instalado. Para enrolar el agente:"
    echo "    COMPLYX_ENROLL_TOKEN=<token> systemctl start $SERVICE"
    echo "========================================================"
fi

if [ $1 -gt 1 ]; then
    # Actualización
    systemctl daemon-reload || true
    systemctl try-restart "$SERVICE" || true
fi
endef

define RPM_PREUN_CONTENT
#!/bin/bash
# Ejecutado antes de desinstalar (%preun, $1=0 = desinstalación, $1=1 = actualización)

SERVICE=complyx-agent

if [ $1 -eq 0 ]; then
    # Desinstalación real
    if systemctl is-active --quiet "$SERVICE" 2>/dev/null; then
        systemctl stop "$SERVICE" || true
    fi
    systemctl disable "$SERVICE" 2>/dev/null || true
fi
endef

define RPM_POSTUN_CONTENT
#!/bin/bash
# Ejecutado después de desinstalar (%postun)

if [ $1 -eq 0 ]; then
    # Desinstalación real — limpiar usuario y datos
    systemctl daemon-reload || true
    if getent passwd complyx >/dev/null 2>&1; then
        userdel complyx || true
    fi
    if getent group complyx >/dev/null 2>&1; then
        groupdel complyx 2>/dev/null || true
    fi
    rm -rf /var/lib/complyx /etc/complyx /etc/sysctl.d/99-complyx.conf || true
fi
endef

define ARCH_INSTALL_CONTENT
# Arch Linux .install hook para complyx-agent

post_install() {
    local CERT_DIR=/var/lib/complyx/certs
    local DATA_DIR=/var/lib/complyx
    local SERVICE=complyx-agent

    # Crear usuario/grupo del sistema
    if ! getent group complyx >/dev/null 2>&1; then
        groupadd --system complyx
    fi
    if ! getent passwd complyx >/dev/null 2>&1; then
        useradd --system --gid complyx \
            --home-dir "$DATA_DIR" --no-create-home \
            --shell /usr/sbin/nologin \
            --comment "Complyx Security Agent" complyx
    fi

    install -d -m 0750 -o complyx -g complyx "$DATA_DIR"
    install -d -m 0700 -o complyx -g complyx "$CERT_DIR"
    install -d -m 0755 /etc/complyx

    # CA pre-provisionada
    if [ -n "${COMPLYX_CA_PATH:-}" ] && [ -f "$COMPLYX_CA_PATH" ]; then
        install -m 0640 -o complyx -g complyx "$COMPLYX_CA_PATH" "$CERT_DIR/ca.crt"
        echo "==> CA instalada en $CERT_DIR/ca.crt"
    fi
    if [ -f "$CERT_DIR/ca.crt" ]; then
        chown complyx:complyx "$CERT_DIR/ca.crt"
        chmod 0640 "$CERT_DIR/ca.crt"
    fi

    # Configuración por defecto
    if [ ! -f /etc/complyx/agent.toml ]; then
        install -m 0644 /usr/share/complyx-agent/agent.toml.example /etc/complyx/agent.toml
    fi

    systemctl daemon-reload

    echo ""
    echo "========================================================"
    echo "  complyx-agent instalado. Para enrolar el agente:"
    echo ""
    echo "  1. Edita /etc/complyx/agent.toml (server_url, enroll_url)"
    echo "  2. Copia la CA del servidor a $CERT_DIR/ca.crt"
    echo "  3. COMPLYX_ENROLL_TOKEN=<token> systemctl start $SERVICE"
    echo "========================================================"
    echo ""
}

post_upgrade() {
    systemctl daemon-reload
    systemctl try-restart complyx-agent || true
}

pre_remove() {
    systemctl stop complyx-agent 2>/dev/null || true
    systemctl disable complyx-agent 2>/dev/null || true
}

post_remove() {
    systemctl daemon-reload
    if getent passwd complyx >/dev/null 2>&1; then
        userdel complyx || true
    fi
    if getent group complyx >/dev/null 2>&1; then
        groupdel complyx 2>/dev/null || true
    fi
    rm -rf /var/lib/complyx /etc/complyx /etc/sysctl.d/99-complyx.conf || true
}
endef

define SYSTEMD_SERVICE_CONTENT
[Unit]
Description=Complyx Security Agent
Documentation=https://github.com/styxiner/complyx-agent
After=network-online.target
Wants=network-online.target
StartLimitIntervalSec=300
StartLimitBurst=5

[Service]
Type=simple
User=complyx
Group=complyx

ExecStart=/usr/bin/complyx-agent
EnvironmentFile=-/etc/complyx/agent.env
Environment=COMPLYX_CONFIG_PATH=/etc/complyx/agent.toml

Restart=on-failure
RestartSec=30s

WorkingDirectory=/var/lib/complyx

StandardOutput=journal
StandardError=journal
SyslogIdentifier=complyx-agent

NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=/var/lib/complyx /etc/sysctl.d
ReadOnlyPaths=/etc /proc/sys /run/
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes

SystemCallFilter=@system-service @file-system @network-io
SystemCallFilter=~@privileged @obsolete @debug @reboot @swap @raw-io

AmbientCapabilities=CAP_DAC_READ_SEARCH CAP_FOWNER CAP_NET_ADMIN
CapabilityBoundingSet=CAP_DAC_READ_SEARCH CAP_FOWNER CAP_NET_ADMIN

ProtectKernelTunables=no
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectClock=yes
ProtectHostname=yes
ProtectControlGroups=yes
MemoryDenyWriteExecute=yes

LimitNOFILE=4096
LimitNPROC=64

[Install]
WantedBy=multi-user.target
endef

# Export all define blocks so $(file) can write them
export POSTINST_CONTENT
export PRERM_CONTENT
export POSTRM_CONTENT
export RPM_PRE_CONTENT
export RPM_POST_CONTENT
export RPM_PREUN_CONTENT
export RPM_POSTUN_CONTENT
export ARCH_INSTALL_CONTENT
export SYSTEMD_SERVICE_CONTENT

# ---------------------------------------------------------------------------
# Scripts de mantenedor (postinst, prerm, postrm, etc.)
# Se generan en packaging/scripts/ y se incluyen en los paquetes.
# ---------------------------------------------------------------------------

## Generar todos los scripts de mantenedor en packaging/scripts/
scripts:
	@mkdir -p $(SCRIPTS_DIR) $(SYSTEMD_DIR) $(CONFIG_DIR)
	@$(MAKE) _script-postinst-debian
	@$(MAKE) _script-prerm-debian
	@$(MAKE) _script-postrm-debian
	@$(MAKE) _script-rpm-pre
	@$(MAKE) _script-rpm-post
	@$(MAKE) _script-rpm-preun
	@$(MAKE) _script-rpm-postun
	@$(MAKE) _script-arch-install
	@$(MAKE) _systemd-service
	@$(MAKE) _default-config
	@echo "Scripts generados en $(SCRIPTS_DIR)/"

# --- Debian: postinst -------------------------------------------------------
# Se ejecuta tras instalar el paquete.
# Responsabilidades:
#   - Crear usuario/grupo 'complyx' si no existen.
#   - Crear directorios con permisos correctos.
#   - Si COMPLYX_CA_PATH está definida, copiar la CA al cert_dir.
#   - Habilitar el servicio (pero NO arrancarlo — el token va aparte).
#   - Mostrar instrucciones de enrolamiento.
_script-postinst-debian:
	$(file >$(SCRIPTS_DIR)/postinst,$(POSTINST_CONTENT))
	@chmod 0755 $(SCRIPTS_DIR)/postinst
	@echo "==> $(SCRIPTS_DIR)/postinst"

# --- Debian: prerm ----------------------------------------------------------
# Se ejecuta antes de eliminar el paquete: para el servicio.
_script-prerm-debian:
	$(file >$(SCRIPTS_DIR)/prerm,$(PRERM_CONTENT))
	@chmod 0755 $(SCRIPTS_DIR)/prerm
	@echo "==> $(SCRIPTS_DIR)/prerm"

# --- Debian: postrm ---------------------------------------------------------
# Se ejecuta tras eliminar o purgar el paquete.
# Con 'purge' elimina datos, certificados y usuario.
_script-postrm-debian:
	$(file >$(SCRIPTS_DIR)/postrm,$(POSTRM_CONTENT))
	@chmod 0755 $(SCRIPTS_DIR)/postrm
	@echo "==> $(SCRIPTS_DIR)/postrm"

# --- RPM: %pre --------------------------------------------------------------
_script-rpm-pre:
	$(file >$(SCRIPTS_DIR)/rpm-pre.sh,$(RPM_PRE_CONTENT))
	@chmod 0755 $(SCRIPTS_DIR)/rpm-pre.sh
	@echo "==> $(SCRIPTS_DIR)/rpm-pre.sh"

# --- RPM: %post -------------------------------------------------------------
_script-rpm-post:
	$(file >$(SCRIPTS_DIR)/rpm-post.sh,$(RPM_POST_CONTENT))
	@chmod 0755 $(SCRIPTS_DIR)/rpm-post.sh
	@echo "==> $(SCRIPTS_DIR)/rpm-post.sh"

# --- RPM: %preun ------------------------------------------------------------
_script-rpm-preun:
	$(file >$(SCRIPTS_DIR)/rpm-preun.sh,$(RPM_PREUN_CONTENT))
	@chmod 0755 $(SCRIPTS_DIR)/rpm-preun.sh
	@echo "==> $(SCRIPTS_DIR)/rpm-preun.sh"

# --- RPM: %postun -----------------------------------------------------------
_script-rpm-postun:
	$(file >$(SCRIPTS_DIR)/rpm-postun.sh,$(RPM_POSTUN_CONTENT))
	@chmod 0755 $(SCRIPTS_DIR)/rpm-postun.sh
	@echo "==> $(SCRIPTS_DIR)/rpm-postun.sh"

# --- Arch: .install ---------------------------------------------------------
_script-arch-install:
	@mkdir -p $(ARCH_PKG_DIR)
	$(file >$(ARCH_PKG_DIR)/complyx-agent.install,$(ARCH_INSTALL_CONTENT))
	@echo "==> $(ARCH_PKG_DIR)/complyx-agent.install"

# --- Servicio systemd -------------------------------------------------------
_systemd-service:
	@mkdir -p $(SYSTEMD_DIR)
	$(file >$(SYSTEMD_DIR)/complyx-agent.service,$(SYSTEMD_SERVICE_CONTENT))
	@echo "==> Servicio systemd generado en $(SYSTEMD_DIR)/complyx-agent.service"
	@echo "    Nota: El fichero EnvironmentFile=/etc/complyx/agent.env permite"
	@echo "    persistir COMPLYX_ENROLL_TOKEN para el primer arranque sin pasarlo"
	@echo "    en línea de comandos (recuerda eliminarlo tras el enrolamiento)."

# --- Configuración por defecto ----------------------------------------------
_default-config:
	@mkdir -p $(CONFIG_DIR)
	@cp crates/agent-core/config/agent.toml $(CONFIG_DIR)/agent.toml.example
	@echo "==> Configuración de ejemplo en $(CONFIG_DIR)/agent.toml.example"

# ---------------------------------------------------------------------------
# Empaquetado .deb (Debian / Ubuntu)
# ---------------------------------------------------------------------------
#
# Prerrequisito de la CA:
#   Antes de arrancar el agente, el administrador debe depositar ca.crt en
#   /var/lib/complyx/certs/ca.crt (perteneciente al usuario complyx, modo 640).
#   El script postinst ajusta permisos si ya existe al instalar, o acepta el
#   path vía la variable COMPLYX_CA_PATH durante la instalación.
#
# Token de enrolamiento:
#   Se pasa al arrancar el servicio:
#     COMPLYX_ENROLL_TOKEN=<token> systemctl start complyx-agent
#   O se persiste en /etc/complyx/agent.env (limpiarlo tras el enrolamiento):
#     echo 'COMPLYX_ENROLL_TOKEN=<token>' > /etc/complyx/agent.env
#     chmod 600 /etc/complyx/agent.env
#     chown complyx:complyx /etc/complyx/agent.env
#     systemctl start complyx-agent

deb: build-static scripts _check-version
	@command -v cargo-deb >/dev/null 2>&1 || \
		{ echo "Instalando cargo-deb..."; cargo install cargo-deb; }
	@mkdir -p $(DIST_DIR) target/release
	@# cargo-deb espera el binario en target/release/
	cp target/$(TARGET_MUSL)/dist/$(BINARY) target/release/$(BINARY)
	@# Preparar assets extra que el paquete debe instalar
	@mkdir -p packaging/share
	cp $(CONFIG_DIR)/agent.toml.example packaging/share/agent.toml.example
	cargo deb --no-build \
		--manifest-path crates/agent-core/Cargo.toml \
		--deb-version "$(VERSION)" \
		--output $(DIST_DIR)/
	@echo ""
	@echo "==> Paquete .deb generado en $(DIST_DIR)/"
	@ls -lh $(DIST_DIR)/*.deb
	@echo ""
	@echo "Instrucciones de instalación:"
	@echo "  # Copiar la CA antes de instalar (opcional, también después):"
	@echo "  scp ca.crt root@endpoint:/var/lib/complyx/certs/ca.crt"
	@echo ""
	@echo "  # Instalar:"
	@echo "  dpkg -i $(DIST_DIR)/$(BINARY)_$(VERSION)_amd64.deb"
	@echo ""
	@echo "  # Enrolar (primera vez):"
	@echo "  COMPLYX_ENROLL_TOKEN=<token> systemctl start complyx-agent"

# ---------------------------------------------------------------------------
# Empaquetado .rpm (RHEL / Fedora / Rocky)
# ---------------------------------------------------------------------------

rpm: build-static scripts _check-version
	@command -v cargo-generate-rpm >/dev/null 2>&1 || \
		{ echo "Instalando cargo-generate-rpm..."; cargo install cargo-generate-rpm; }
	@mkdir -p $(DIST_DIR)
	cargo generate-rpm \
		-p crates/agent-core \
		--target $(TARGET_MUSL) \
		--profile dist \
		--output $(DIST_DIR)/$(BINARY)-$(VERSION)-1.x86_64.rpm
	@echo ""
	@echo "==> Paquete .rpm generado en $(DIST_DIR)/"
	@ls -lh $(DIST_DIR)/*.rpm
	@echo ""
	@echo "Instrucciones de instalación:"
	@echo "  # Copiar la CA antes de instalar (opcional, también después):"
	@echo "  scp ca.crt root@endpoint:/var/lib/complyx/certs/ca.crt"
	@echo ""
	@echo "  # Instalar:"
	@echo "  rpm -i $(DIST_DIR)/$(BINARY)-$(VERSION)-1.x86_64.rpm"
	@echo "  # o con dnf: dnf install $(DIST_DIR)/$(BINARY)-$(VERSION)-1.x86_64.rpm"
	@echo ""
	@echo "  # Enrolar (primera vez):"
	@echo "  COMPLYX_ENROLL_TOKEN=<token> systemctl start complyx-agent"

# ---------------------------------------------------------------------------
# Empaquetado Arch Linux (PKGBUILD + makepkg)
# ---------------------------------------------------------------------------

arch: build-static scripts _check-version
	@mkdir -p $(DIST_DIR)/arch
	cp target/$(TARGET_MUSL)/dist/$(BINARY)    $(DIST_DIR)/arch/$(BINARY)
	cp $(CONFIG_DIR)/agent.toml.example        $(DIST_DIR)/arch/agent.toml.example
	cp $(SYSTEMD_DIR)/complyx-agent.service    $(DIST_DIR)/arch/complyx-agent.service
	cp $(ARCH_PKG_DIR)/complyx-agent.install   $(DIST_DIR)/arch/complyx-agent.install
	@# Generar PKGBUILD con la versión actual
	@sed "s/^pkgver=.*/pkgver=$(VERSION)/" $(ARCH_PKG_DIR)/PKGBUILD \
		> $(DIST_DIR)/arch/PKGBUILD
	@echo ""
	@echo "==> PKGBUILD preparado en $(DIST_DIR)/arch/"
	@echo ""
	@echo "Instrucciones de instalación:"
	@echo "  cd $(DIST_DIR)/arch && makepkg -si"
	@echo ""
	@echo "  # Copiar la CA antes de arrancar:"
	@echo "  install -m 640 -o complyx -g complyx ca.crt /var/lib/complyx/certs/ca.crt"
	@echo ""
	@echo "  # Enrolar (primera vez):"
	@echo "  COMPLYX_ENROLL_TOKEN=<token> systemctl start complyx-agent"

# ---------------------------------------------------------------------------
# Generar todos los paquetes
# ---------------------------------------------------------------------------

all-packages: build-static scripts _check-version
	@mkdir -p $(DIST_DIR)
	@echo "=== Generando paquetes para complyx-agent v$(VERSION) ==="
	@echo ""

	@# .deb
	@if command -v cargo-deb >/dev/null 2>&1; then \
		$(MAKE) deb; \
	else \
		echo "OMITIDO .deb — instala cargo-deb: cargo install cargo-deb"; \
	fi

	@# .rpm
	@if command -v cargo-generate-rpm >/dev/null 2>&1; then \
		$(MAKE) rpm; \
	else \
		echo "OMITIDO .rpm — instala cargo-generate-rpm: cargo install cargo-generate-rpm"; \
	fi

	@# Arch (makepkg es opcional en la máquina de build)
	$(MAKE) arch

	@echo ""
	@echo "=== Paquetes generados en $(DIST_DIR)/ ==="
	@ls -lh $(DIST_DIR)/ 2>/dev/null || echo "(ninguno)"
	@echo ""
	@echo "=== Resumen del flujo de instalación ==="
	@echo ""
	@echo "  PASO 1 — Copiar la CA al endpoint (antes o después de instalar):"
	@echo "    install -m 640 -o complyx -g complyx ca.crt /var/lib/complyx/certs/ca.crt"
	@echo "    (El usuario 'complyx' se crea durante la instalación del paquete)"
	@echo ""
	@echo "  PASO 2 — Editar /etc/complyx/agent.toml:"
	@echo "    server_url = \"https://<servidor>:9000\""
	@echo "    enroll_url = \"https://<servidor>:9001\""
	@echo ""
	@echo "  PASO 3 — Generar token en el servidor:"
	@echo "    complyx-server enroll-token --hostname <hostname>"
	@echo ""
	@echo "  PASO 4 — Primer arranque con el token:"
	@echo "    COMPLYX_ENROLL_TOKEN=<token> systemctl start complyx-agent"
	@echo ""
	@echo "  El token se usa una sola vez. Tras el enrolamiento el agente"
	@echo "  arranca normalmente con 'systemctl start complyx-agent'."

# ---------------------------------------------------------------------------
# Instalar herramientas de build
# ---------------------------------------------------------------------------

install-tools:
	rustup target add $(TARGET_MUSL)
	@# musl linker (Debian/Ubuntu)
	@if command -v apt-get >/dev/null 2>&1; then \
		sudo apt-get install -y musl-tools protobuf-compiler; \
	fi
	@# musl linker (Fedora/RHEL)
	@if command -v dnf >/dev/null 2>&1; then \
		sudo dnf install -y musl-gcc musl-devel musl-libc-static protobuf-compiler; \
	fi
	@# Arch
	@if command -v pacman >/dev/null 2>&1; then \
		sudo pacman -S --noconfirm musl protobuf; \
	fi
	cargo install cargo-deb
	cargo install cargo-generate-rpm
	cargo install sqlx-cli --no-default-features --features sqlite
	@echo ""
	@echo "Herramientas instaladas correctamente."

# ---------------------------------------------------------------------------
# Limpieza
# ---------------------------------------------------------------------------

clean:
	cargo clean
	rm -rf $(DIST_DIR)/ packaging/share/
	@echo "Limpieza completada."
