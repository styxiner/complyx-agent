#!/usr/bin/env bash
# build-packages.sh
#
# Compila el binario de complyx-agent y genera paquetes .deb, .rpm y PKGBUILD
# para Arch Linux.
#
# Uso:
#   ./packaging/build-packages.sh [--version X.Y.Z] [--target <rust-target>]
#
# Dependencias en el sistema de build:
#   - cargo + rustup
#   - dpkg-deb          (para .deb)
#   - rpmbuild          (para .rpm)
#   - makepkg           (para Arch, solo en sistemas Arch)
#   - musl-tools        (para compilación estática: apt install musl-tools)

set -euo pipefail

# ---------------------------------------------------------------------------
# Parámetros
# ---------------------------------------------------------------------------
VERSION="0.1.0"
TARGET="x86_64-unknown-linux-musl" # Binario estático: no depende de glibc
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)" # raíz del workspace Cargo
BUILD_DIR="$SCRIPT_DIR/build"
BINARY_NAME="complyx-agent"

while [[ $# -gt 0 ]]; do
  case $1 in
  --version)
    VERSION="$2"
    shift 2
    ;;
  --target)
    TARGET="$2"
    shift 2
    ;;
  *)
    echo "Argumento desconocido: $1"
    exit 1
    ;;
  esac
done

echo "=== Build de complyx-agent v${VERSION} (target: ${TARGET}) ==="

# ---------------------------------------------------------------------------
# 1. Compilar el binario con musl para enlace estático
# ---------------------------------------------------------------------------
echo "--- [1/4] Compilando binario..."

rustup target add "$TARGET" 2>/dev/null || true

DATABASE_URL="sqlite:$WORKSPACE_ROOT/complyx-agent.db" \
  cargo build \
  --profile dist \
  --bin complyx-agent \
  --target "$TARGET" \
  --manifest-path "$WORKSPACE_ROOT/Cargo.toml"

BINARY_PATH="$WORKSPACE_ROOT/target/$TARGET/dist/$BINARY_NAME"

if [[ ! -f "$BINARY_PATH" ]]; then
  echo "ERROR: binario no encontrado en $BINARY_PATH"
  exit 1
fi

BINARY_SIZE=$(du -sh "$BINARY_PATH" | cut -f1)
echo "Binario compilado: $BINARY_PATH ($BINARY_SIZE)"

# ---------------------------------------------------------------------------
# 2. Preparar artefactos comunes
# ---------------------------------------------------------------------------
echo "--- [2/4] Preparando artefactos..."

mkdir -p "$BUILD_DIR"
cp "$BINARY_PATH" "$BUILD_DIR/complyx-agent"
cp "$SCRIPT_DIR/config/agent.toml" "$BUILD_DIR/agent.toml"
cp "$SCRIPT_DIR/systemd/complyx-agent.service" "$BUILD_DIR/complyx-agent.service"

# Sustituir %{version} en el spec de RPM
sed "s/%{version}/$VERSION/g" "$SCRIPT_DIR/rpm/complyx-agent.spec" \
  >"$BUILD_DIR/complyx-agent.spec"

# ---------------------------------------------------------------------------
# 3. Paquete .deb (Debian / Ubuntu)
# ---------------------------------------------------------------------------
if command -v dpkg-deb &>/dev/null; then
  echo "--- [3a/4] Generando paquete .deb..."

  DEB_ROOT="$BUILD_DIR/deb"
  DEB_PKG="$DEB_ROOT/$BINARY_NAME-$VERSION"

  # Estructura del paquete deb
  mkdir -p "$DEB_PKG/DEBIAN"
  mkdir -p "$DEB_PKG/usr/bin"
  mkdir -p "$DEB_PKG/etc/complyx"
  mkdir -p "$DEB_PKG/usr/lib/systemd/system"

  # Ficheros del paquete
  cp "$BUILD_DIR/complyx-agent" "$DEB_PKG/usr/bin/complyx-agent"
  cp "$BUILD_DIR/agent.toml" "$DEB_PKG/etc/complyx/agent.toml"
  cp "$BUILD_DIR/complyx-agent.service" "$DEB_PKG/usr/lib/systemd/system/complyx-agent.service"

  # Permisos del binario
  chmod 0755 "$DEB_PKG/usr/bin/complyx-agent"

  # Scripts de mantenimiento
  cp "$SCRIPT_DIR/debian/postinst" "$DEB_PKG/DEBIAN/postinst"
  cp "$SCRIPT_DIR/debian/prerm" "$DEB_PKG/DEBIAN/prerm"
  cp "$SCRIPT_DIR/debian/postrm" "$DEB_PKG/DEBIAN/postrm"
  chmod 0755 "$DEB_PKG/DEBIAN/postinst" "$DEB_PKG/DEBIAN/prerm" "$DEB_PKG/DEBIAN/postrm"

  # Fichero de control
  INSTALLED_SIZE=$(du -sk "$DEB_PKG/usr" | cut -f1)
  cat >"$DEB_PKG/DEBIAN/control" <<EOF
Package: complyx-agent
Version: $VERSION
Section: admin
Priority: optional
Architecture: amd64
Installed-Size: $INSTALLED_SIZE
Maintainer: Complyx Team <dev@complyx.io>
Description: Complyx Security and Compliance Agent
 Agente de gestión de cumplimiento normativo.
 Ejecuta checks de seguridad, envía resultados al servidor central
 y aplica remediaciones automáticas.
EOF

  # Fichero conffiles: los config files no se sobreescriben en upgrade
  cat >"$DEB_PKG/DEBIAN/conffiles" <<EOF
/etc/complyx/agent.toml
EOF

  dpkg-deb --build --root-owner-group "$DEB_PKG" \
    "$DEB_ROOT/${BINARY_NAME}_${VERSION}_amd64.deb"

  echo "Paquete .deb generado: $DEB_ROOT/${BINARY_NAME}_${VERSION}_amd64.deb"
else
  echo "dpkg-deb no disponible, omitiendo .deb"
fi

# ---------------------------------------------------------------------------
# 4. Paquete .rpm (RHEL / Fedora / SUSE)
# ---------------------------------------------------------------------------
if command -v rpmbuild &>/dev/null; then
  echo "--- [3b/4] Generando paquete .rpm..."

  RPM_ROOT="$BUILD_DIR/rpm"
  mkdir -p "$RPM_ROOT"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}

  # Copiar sources que usa el spec
  cp "$BUILD_DIR/complyx-agent" "$RPM_ROOT/SOURCES/complyx-agent"
  cp "$BUILD_DIR/agent.toml" "$RPM_ROOT/SOURCES/agent.toml"
  cp "$BUILD_DIR/complyx-agent.service" "$RPM_ROOT/SOURCES/complyx-agent.service"
  cp "$BUILD_DIR/complyx-agent.spec" "$RPM_ROOT/SPECS/complyx-agent.spec"

  rpmbuild \
    --define "_topdir $RPM_ROOT" \
    --define "version $VERSION" \
    -bb "$RPM_ROOT/SPECS/complyx-agent.spec"

  RPM_FILE=$(find "$RPM_ROOT/RPMS" -name "*.rpm" | head -1)
  echo "Paquete .rpm generado: $RPM_FILE"
else
  echo "rpmbuild no disponible, omitiendo .rpm"
fi

# ---------------------------------------------------------------------------
# 5. PKGBUILD para Arch Linux
# ---------------------------------------------------------------------------
echo "--- [4/4] Preparando PKGBUILD para Arch..."

ARCH_DIR="$BUILD_DIR/arch"
mkdir -p "$ARCH_DIR"
cp "$BUILD_DIR/complyx-agent" "$ARCH_DIR/complyx-agent"
cp "$BUILD_DIR/agent.toml" "$ARCH_DIR/agent.toml"
cp "$BUILD_DIR/complyx-agent.service" "$ARCH_DIR/complyx-agent.service"

sed "s/pkgver=.*/pkgver=$VERSION/" "$SCRIPT_DIR/arch/PKGBUILD" \
  >"$ARCH_DIR/PKGBUILD"

# En sistemas Arch se puede ejecutar directamente:
# cd "$ARCH_DIR" && makepkg -si
echo "PKGBUILD preparado en: $ARCH_DIR"
echo "Para instalar en Arch: cd $ARCH_DIR && makepkg -si"

# ---------------------------------------------------------------------------
# Resumen
# ---------------------------------------------------------------------------
echo ""
echo "=== Build completado ==="
echo ""
ls -lh "$BUILD_DIR"/*.deb "$BUILD_DIR"/rpm/RPMS/**/*.rpm 2>/dev/null || true
echo ""
echo "Artefactos en: $BUILD_DIR"
