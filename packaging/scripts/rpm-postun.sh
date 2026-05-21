#!/bin/bash
# Ejecutado después de desinstalar (%postun)

if [  -eq 0 ]; then
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
