#!/bin/bash
# Ejecutado antes de instalar el paquete RPM (%pre)

if ! getent group complyx >/dev/null 2>&1; then
    groupadd --system complyx
fi
if ! getent passwd complyx >/dev/null 2>&1; then
    useradd --system --gid complyx --home-dir /var/lib/complyx --no-create-home --shell /sbin/nologin --comment "Complyx Security Agent" complyx
fi
