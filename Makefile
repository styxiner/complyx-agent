# =============================================================================
# complyx-agent — Makefile
# =============================================================================
#
# Targets principales:
#   make build          Compilar en modo release (glibc, desarrollo)
#   make build-static   Compilar estático con musl (producción)
#   make deb            Generar paquete .deb
#   make rpm            Generar paquete .rpm
#   make arch           Preparar PKGBUILD para Arch Linux
#   make all-packages   Generar todos los paquetes disponibles
#   make clean          Limpiar artefactos de build
#   make install-tools  Instalar herramientas de empaquetado necesarias

VERSION  := $(shell grep '^version' crates/agent-core/Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
BINARY   := complyx-agent
TARGET_MUSL := x86_64-unknown-linux-musl
TARGET_GLIBC := x86_64-unknown-linux-gnu

# Directorio de salida de paquetes
DIST_DIR := dist

DATABASE_URL ?= sqlite:./complyx-agent.db

.PHONY: all build build-static deb rpm arch all-packages clean install-tools \
        prepare-db check test

# ---------------------------------------------------------------------------
# Targets por defecto
# ---------------------------------------------------------------------------

all: build

# ---------------------------------------------------------------------------
# Compilación
# ---------------------------------------------------------------------------

## Compilar en modo release con glibc (desarrollo, tests)
build:
	DATABASE_URL="$(DATABASE_URL)" cargo build --release --bin $(BINARY)
	@echo "Binario: target/release/$(BINARY)"

## Compilar estático con musl (producción, empaquetado)
build-static:
	@rustup target add $(TARGET_MUSL) 2>/dev/null || true
	DATABASE_URL="$(DATABASE_URL)" \
	cargo build --profile dist --target $(TARGET_MUSL) --bin $(BINARY)
	@echo "Binario estático: target/$(TARGET_MUSL)/dist/$(BINARY)"

## Preparar la BD SQLite para que sqlx pueda compilar
prepare-db:
	@command -v sqlx >/dev/null 2>&1 || cargo install sqlx-cli --no-default-features --features sqlite
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
# Empaquetado .deb (Debian / Ubuntu)
# ---------------------------------------------------------------------------

deb: build-static
	@command -v cargo-deb >/dev/null 2>&1 || \
		{ echo "Instalando cargo-deb..."; cargo install cargo-deb; }
	@mkdir -p $(DIST_DIR)
	# cargo-deb usa el binario release estándar — copiamos el musl al lugar esperado
	cp target/$(TARGET_MUSL)/dist/$(BINARY) target/release/$(BINARY)
	cargo deb --no-build --manifest-path crates/agent-core/Cargo.toml \
		--output $(DIST_DIR)/
	@echo "Paquete .deb generado en $(DIST_DIR)/"
	@ls -lh $(DIST_DIR)/*.deb

# ---------------------------------------------------------------------------
# Empaquetado .rpm (RHEL / Fedora / Rocky)
# ---------------------------------------------------------------------------

rpm: build-static
	@command -v cargo-generate-rpm >/dev/null 2>&1 || \
		{ echo "Instalando cargo-generate-rpm..."; cargo install cargo-generate-rpm; }
	@mkdir -p $(DIST_DIR)
	# cargo-generate-rpm lee el binario del perfil dist del target musl
	cargo generate-rpm \
		--manifest-path crates/agent-core/Cargo.toml \
		--target $(TARGET_MUSL) \
		--profile dist \
		--output $(DIST_DIR)/$(BINARY)-$(VERSION)-1.x86_64.rpm
	@echo "Paquete .rpm generado en $(DIST_DIR)/"
	@ls -lh $(DIST_DIR)/*.rpm

# ---------------------------------------------------------------------------
# Empaquetado Arch Linux (PKGBUILD + makepkg)
# ---------------------------------------------------------------------------

arch: build-static
	@mkdir -p $(DIST_DIR)/arch
	# Copiar los artefactos al directorio de build de Arch
	cp target/$(TARGET_MUSL)/dist/$(BINARY)          $(DIST_DIR)/arch/$(BINARY)
	cp packaging/config/agent.toml                    $(DIST_DIR)/arch/agent.toml
	cp packaging/systemd/complyx-agent.service        $(DIST_DIR)/arch/complyx-agent.service
	# Actualizar la versión en el PKGBUILD
	sed "s/^pkgver=.*/pkgver=$(VERSION)/" packaging/arch/PKGBUILD > $(DIST_DIR)/arch/PKGBUILD
	@echo "PKGBUILD preparado en $(DIST_DIR)/arch/"
	@echo "Para instalar: cd $(DIST_DIR)/arch && makepkg -si"

# ---------------------------------------------------------------------------
# Generar todos los paquetes disponibles
# ---------------------------------------------------------------------------

all-packages: build-static
	@mkdir -p $(DIST_DIR)
	@echo "=== Generando paquetes para complyx-agent v$(VERSION) ==="

	@# .deb
	@if command -v cargo-deb >/dev/null 2>&1; then \
		$(MAKE) deb; \
	else \
		echo "cargo-deb no disponible, omitiendo .deb (instala con: cargo install cargo-deb)"; \
	fi

	@# .rpm
	@if command -v cargo-generate-rpm >/dev/null 2>&1; then \
		$(MAKE) rpm; \
	else \
		echo "cargo-generate-rpm no disponible, omitiendo .rpm (instala con: cargo install cargo-generate-rpm)"; \
	fi

	@# Arch (siempre disponible, makepkg es opcional)
	$(MAKE) arch

	@echo ""
	@echo "=== Paquetes generados ==="
	@ls -lh $(DIST_DIR)/ 2>/dev/null || echo "(ninguno)"

# ---------------------------------------------------------------------------
# Instalar herramientas de empaquetado
# ---------------------------------------------------------------------------

install-tools:
	rustup target add $(TARGET_MUSL)
	@# musl linker (Debian/Ubuntu)
	@if command -v apt-get >/dev/null 2>&1; then \
		sudo apt-get install -y musl-tools; \
	fi
	@# musl linker (Fedora/RHEL)
	@if command -v dnf >/dev/null 2>&1; then \
		sudo dnf install -y musl-gcc; \
	fi
	cargo install cargo-deb
	cargo install cargo-generate-rpm
	cargo install sqlx-cli --no-default-features --features sqlite
	@echo "Herramientas instaladas correctamente"

# ---------------------------------------------------------------------------
# Limpieza
# ---------------------------------------------------------------------------

clean:
	cargo clean
	rm -rf $(DIST_DIR)/
