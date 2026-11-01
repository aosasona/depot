#!/bin/sh
set -eu

usage() {
    echo "Usage: $0 <vm|caddy-agent> <path-to-depot-binary>" >&2
    echo "Set DESTDIR to stage files without changing users or systemd." >&2
    exit 2
}

[ "$#" -eq 2 ] || usage
mode=$1
binary=$2
[ -f "$binary" ] && [ -x "$binary" ] || {
    echo "Depot binary must be an executable regular file: $binary" >&2
    exit 2
}

case "$mode" in
    vm|caddy-agent) ;;
    *) usage ;;
esac

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repository_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
stage=${DESTDIR:-}

if [ -z "$stage" ] && [ "$(id -u)" -ne 0 ]; then
    echo "Run as root, or set DESTDIR for an unprivileged staging install." >&2
    exit 1
fi

install -d -m 0755 "$stage/usr/local/bin" "$stage/etc/depot" "$stage/etc/systemd/system"
install -m 0755 "$binary" "$stage/usr/local/bin/depot"
install -d -m 0755 "$stage/usr/local/sbin"
install -m 0755 "$repository_root/scripts/setup.sh" "$stage/usr/local/sbin/depot-setup"

if [ "$mode" = "vm" ]; then
    install -d -m 0770 "$stage/var/lib/depot" "$stage/var/lib/depot/workspaces" "$stage/var/lib/depot/static"
    if [ ! -e "$stage/etc/depot/config.toml" ]; then
        install -m 0640 "$repository_root/deploy/config.example.toml" "$stage/etc/depot/config.toml"
    fi
    install -m 0644 "$repository_root/deploy/systemd/depot.service" "$stage/etc/systemd/system/depot.service"
    install -m 0644 "$repository_root/deploy/systemd/depot-worker.service" "$stage/etc/systemd/system/depot-worker.service"
    if [ ! -e "$stage/etc/depot/master.key" ]; then
        "$stage/usr/local/bin/depot" generate-key --output "$stage/etc/depot/master.key"
    fi
else
    install -d -m 0750 "$stage/etc/caddy/depot"
    if [ ! -e "$stage/etc/depot/caddy-agent.toml" ]; then
        install -m 0640 "$repository_root/deploy/caddy-agent.example.toml" "$stage/etc/depot/caddy-agent.toml"
    fi
    if [ ! -e "$stage/etc/caddy/depot/routes.caddy" ]; then
        install -m 0640 /dev/null "$stage/etc/caddy/depot/routes.caddy"
    fi
    install -m 0644 "$repository_root/deploy/systemd/depot-caddy-agent.service" "$stage/etc/systemd/system/depot-caddy-agent.service"
fi

if [ -n "$stage" ]; then
    echo "Staged Depot $mode installation under $stage."
    exit 0
fi

if [ "$mode" = "vm" ]; then
    getent group depot >/dev/null 2>&1 || groupadd --system depot
    id depot >/dev/null 2>&1 || useradd --system --gid depot --home-dir /var/lib/depot --shell /usr/sbin/nologin depot
    id depot-worker >/dev/null 2>&1 || useradd --system --gid depot --home-dir /var/lib/depot --shell /usr/sbin/nologin depot-worker
    chown -R depot:depot /var/lib/depot
    chmod -R g+rwX /var/lib/depot
    chown depot:depot /etc/depot/config.toml
    chown root:depot /etc/depot/master.key
    chmod 0640 /etc/depot/master.key
    if getent group docker >/dev/null 2>&1; then
        usermod --append --groups docker depot-worker
    else
        echo "Warning: docker group is absent; grant the depot worker Docker access before starting it." >&2
    fi
else
    getent group caddy >/dev/null 2>&1 || {
        echo "The caddy group must exist before installing the agent." >&2
        exit 1
    }
    id depot-caddy >/dev/null 2>&1 || useradd --system --gid caddy --home-dir /nonexistent --shell /usr/sbin/nologin depot-caddy
    chown depot-caddy:caddy /etc/depot/caddy-agent.toml /etc/caddy/depot /etc/caddy/depot/routes.caddy
fi

systemctl daemon-reload
echo "Depot $mode files installed. Review configuration before enabling services."
