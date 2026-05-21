#!/bin/bash
# Ejecutado antes de desinstalar (%preun, =0 = desinstalación, =1 = actualización)

SERVICE=complyx-agent

if [  -eq 0 ]; then
    # Desinstalación real
    if systemctl is-active --quiet "ERVICE" 2>/dev/null; then
        systemctl stop "ERVICE" || true
    fi
    systemctl disable "ERVICE" 2>/dev/null || true
fi
