#!/bin/bash
# Ejecutado después de instalar el paquete RPM (%post)

CERT_DIR=/var/lib/complyx/certs
DATA_DIR=/var/lib/complyx
CONFIG_FILE=/etc/complyx/agent.toml
SERVICE=complyx-agent

install -d -m 0750 -o complyx -g complyx "ATA_DIR"
install -d -m 0700 -o complyx -g complyx "ERT_DIR"
install -d -m 0755            /etc/complyx

if [ -n "" ] && [ -f "OMPLYX_CA_PATH" ]; then
    echo "==> Copiando CA desde OMPLYX_CA_PATH a ERT_DIR/ca.crt"
    install -m 0640 -o complyx -g complyx "OMPLYX_CA_PATH" "ERT_DIR/ca.crt"
fi

if [ -f "ERT_DIR/ca.crt" ]; then
    chown complyx:complyx "ERT_DIR/ca.crt"
    chmod 0640 "ERT_DIR/ca.crt"
fi

if [ ! -f "ONFIG_FILE" ]; then
    install -m 0644 /usr/share/complyx-agent/agent.toml.example "ONFIG_FILE"
fi

if [  -eq 1 ]; then
    # Primera instalación
    systemctl daemon-reload || true
    systemctl enable "ERVICE" || true
    echo ""
    echo "========================================================"
    echo "  complyx-agent instalado. Para enrolar el agente:"
    echo "    COMPLYX_ENROLL_TOKEN=<token> systemctl start ERVICE"
    echo "========================================================"
fi

if [  -gt 1 ]; then
    # Actualización
    systemctl daemon-reload || true
    systemctl try-restart "ERVICE" || true
fi
