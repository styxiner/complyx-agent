Name:           complyx-agent
Version:        %{version}
Release:        1%{?dist}
Summary:        Complyx Security and Compliance Agent
License:        Proprietary
URL:            https://github.com/styxiner/complyx-agent

# El binario se compila fuera del spec y se pasa como Source0
Source0:        complyx-agent
Source1:        agent.toml
Source2:        complyx-agent.service

# No autoprovisnig de Rust (binario estático)
AutoReqProv:    no

%description
Agente de gestión de cumplimiento normativo de Complyx.
Ejecuta checks de seguridad en el endpoint, envía resultados al servidor
central y aplica remediaciones automáticas cuando está configurado.

# ---------------------------------------------------------------------------
# Preparación: no compilamos aquí, usamos el binario ya compilado
# ---------------------------------------------------------------------------
%prep
# nada

%build
# nada — el binario se compila con: cargo build --release --bin complyx-agent

%install
install -D -m 0755 %{SOURCE0}   %{buildroot}/usr/bin/complyx-agent
install -D -m 0644 %{SOURCE1}   %{buildroot}/etc/complyx/agent.toml
install -D -m 0644 %{SOURCE2}   %{buildroot}/usr/lib/systemd/system/complyx-agent.service

%pre
# Crear grupo y usuario antes de instalar los ficheros
getent group complyx > /dev/null || groupadd --system complyx
getent passwd complyx > /dev/null || \
    useradd --system --gid complyx \
            --home-dir /var/lib/complyx \
            --no-create-home \
            --shell /sbin/nologin \
            --comment "Complyx Security Agent" \
            complyx

%post
# Crear directorios de datos con permisos correctos
install -d -m 0755 -o complyx -g complyx /var/lib/complyx
install -d -m 0700 -o complyx -g complyx /var/lib/complyx/certs

%systemd_post complyx-agent.service

echo ""
echo "=== Complyx Agent instalado ==="
echo "Configura el servidor en /etc/complyx/agent.toml"
echo "Luego: COMPLYX_ENROLL_TOKEN=<token> systemctl start complyx-agent"
echo ""

%preun
%systemd_preun complyx-agent.service

%postun
%systemd_postun_with_restart complyx-agent.service

# En desinstalación completa (no upgrade): limpiar datos
if [ $1 -eq 0 ]; then
    rm -rf /var/lib/complyx
    rm -rf /etc/complyx
    getent passwd complyx > /dev/null && userdel complyx || true
    getent group complyx > /dev/null && groupdel complyx || true
fi

%files
%attr(0755, root, root) /usr/bin/complyx-agent
%config(noreplace) %attr(0644, root, root) /etc/complyx/agent.toml
%attr(0644, root, root) /usr/lib/systemd/system/complyx-agent.service

%changelog
* %(date "+%a %b %d %Y") Complyx Team <dev@complyx.io> - %{version}-1
- Versión inicial del paquete RPM
