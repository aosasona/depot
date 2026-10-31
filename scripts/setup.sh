#!/bin/sh
set -eu

REPOSITORY=${DEPOT_GITHUB_REPOSITORY:-aosasona/depot}
VERSION=${DEPOT_VERSION:-latest}
ASSUME_YES=0
ROLE=
COMMAND=
TTY=/dev/tty

say() { printf '%s\n' "$*"; }
warn() { printf 'Warning: %s\n' "$*" >&2; }
die() { printf 'Error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
Depot guided setup

Usage: depot-setup [install|update|check|start|stop|restart|status] [options]

Options:
  --role vm|caddy-agent  Select the Depot VM or Caddy-host agent
  --version TAG          Install a release tag instead of the latest release
  --yes                  Accept package installation and service prompts
  --help                 Show this help

Fresh install example:
  curl -fsSL https://github.com/aosasona/depot/releases/latest/download/depot-setup.sh | sudo sh

After installation:
  sudo depot-setup check
  sudo depot-setup update
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        install|update|check|start|stop|restart|status)
            [ -z "$COMMAND" ] || die "Specify only one command."
            COMMAND=$1
            ;;
        --role)
            shift
            [ "$#" -gt 0 ] || die "--role requires vm or caddy-agent."
            ROLE=$1
            ;;
        --version)
            shift
            [ "$#" -gt 0 ] || die "--version requires a release tag."
            VERSION=$1
            ;;
        --yes) ASSUME_YES=1 ;;
        --help|-h) usage; exit 0 ;;
        *) die "Unknown argument: $1" ;;
    esac
    shift
done

prompt() {
    label=$1
    default=${2:-}
    if [ "$ASSUME_YES" -eq 1 ] && [ -n "$default" ]; then
        REPLY=$default
        return
    fi
    if [ -n "$default" ]; then
        printf '%s [%s]: ' "$label" "$default" >"$TTY"
    else
        printf '%s: ' "$label" >"$TTY"
    fi
    IFS= read -r REPLY <"$TTY" || die "Input was interrupted."
    [ -n "$REPLY" ] || REPLY=$default
}

confirm() {
    question=$1
    if [ "$ASSUME_YES" -eq 1 ]; then
        return 0
    fi
    printf '%s [y/N]: ' "$question" >"$TTY"
    IFS= read -r answer <"$TTY" || return 1
    case "$answer" in y|Y|yes|YES) return 0 ;; *) return 1 ;; esac
}

require_root() {
    [ "$(id -u)" -eq 0 ] || die "Run this command as root (for example: curl ... | sudo sh)."
}

infer_role() {
    if [ -n "$ROLE" ]; then
        case "$ROLE" in vm|caddy-agent) return ;; *) die "Role must be vm or caddy-agent." ;; esac
    fi
    if [ -f /etc/depot/config.toml ]; then
        ROLE=vm
        return
    fi
    if [ -f /etc/depot/caddy-agent.toml ]; then
        ROLE=caddy-agent
        return
    fi
    say "Select the host role:"
    say "  1) Depot VM (API, dashboard, gateway, worker)"
    say "  2) Caddy agent host"
    prompt "Role" "1"
    case "$REPLY" in 1|vm) ROLE=vm ;; 2|caddy-agent|agent) ROLE=caddy-agent ;; *) die "Invalid role." ;; esac
}

detect_package_manager() {
    if command -v apt-get >/dev/null 2>&1; then
        PACKAGE_MANAGER=apt
    elif command -v dnf >/dev/null 2>&1; then
        PACKAGE_MANAGER=dnf
    elif command -v yum >/dev/null 2>&1; then
        PACKAGE_MANAGER=yum
    else
        PACKAGE_MANAGER=none
    fi
}

missing_commands() {
    MISSING=
    for command_name in "$@"; do
        command -v "$command_name" >/dev/null 2>&1 || MISSING="$MISSING $command_name"
    done
}

install_dependencies() {
    require_root
    [ "$(uname -s)" = Linux ] || die "Depot guided setup currently supports Linux only."
    case "$(uname -m)" in x86_64|amd64) ;; *) die "This release supports x86-64 hosts only." ;; esac
    command -v systemctl >/dev/null 2>&1 || die "systemd is required but systemctl is unavailable."
    detect_package_manager
    common_commands="curl tar sha256sum awk grep install mktemp runuser getent id chown chmod cp date od tr rm mkdir useradd"
    if [ "$ROLE" = vm ]; then
        required_commands="$common_commands groupadd usermod sqlite3 git docker"
    else
        required_commands="$common_commands caddy"
    fi
    missing_commands $required_commands
    if [ -z "$MISSING" ]; then
        say "[ok] Required commands are installed."
    else
        warn "Missing commands:$MISSING"
        [ "$PACKAGE_MANAGER" != none ] || die "No supported package manager was found; install the missing commands manually."
        confirm "Install the missing prerequisites with $PACKAGE_MANAGER?" || die "Install the missing prerequisites, then run setup again."
        packages="ca-certificates"
        case " $MISSING " in *" curl "*) packages="$packages curl" ;; esac
        case " $MISSING " in *" tar "*) packages="$packages tar" ;; esac
        case " $MISSING " in
            *" sha256sum "*|*" install "*|*" id "*|*" chown "*|*" chmod "*|*" cp "*|*" date "*|*" od "*|*" tr "*|*" rm "*|*" mkdir "*)
                packages="$packages coreutils"
            ;;
        esac
        case " $MISSING " in *" awk "*) packages="$packages gawk" ;; esac
        case " $MISSING " in *" grep "*) packages="$packages grep" ;; esac
        case " $MISSING " in *" mktemp "*|*" runuser "*) packages="$packages util-linux" ;; esac
        case " $MISSING " in *" getent "*)
            if [ "$PACKAGE_MANAGER" = apt ]; then packages="$packages libc-bin"; else packages="$packages glibc-common"; fi
        ;; esac
        case " $MISSING " in *" useradd "*|*" groupadd "*|*" usermod "*)
            if [ "$PACKAGE_MANAGER" = apt ]; then packages="$packages passwd"; else packages="$packages shadow-utils"; fi
        ;; esac
        case " $MISSING " in *" git "*) packages="$packages git" ;; esac
        case " $MISSING " in *" sqlite3 "*)
            if [ "$PACKAGE_MANAGER" = apt ]; then packages="$packages sqlite3"; else packages="$packages sqlite"; fi
        ;; esac
        case " $MISSING " in *" docker "*)
            if [ "$PACKAGE_MANAGER" = apt ]; then packages="$packages docker.io"; else packages="$packages docker"; fi
        ;; esac
        case " $MISSING " in *" caddy "*) packages="$packages caddy" ;; esac
        if [ "$PACKAGE_MANAGER" = apt ]; then
            apt-get update
            case " $MISSING " in *" caddy "*)
                apt-cache show caddy >/dev/null 2>&1 ||
                    die "This apt repository does not provide Caddy. Install Caddy from its official repository, then rerun setup."
            ;; esac
            DEBIAN_FRONTEND=noninteractive apt-get install -y $packages
        else
            "$PACKAGE_MANAGER" install -y $packages ||
                die "The configured repositories could not install every dependency. Install the missing commands manually, then rerun setup."
        fi
        missing_commands $required_commands
        [ -z "$MISSING" ] || die "Dependencies are still missing:$MISSING"
    fi

    if [ "$ROLE" = vm ]; then
        if ! systemctl is-active --quiet docker.service; then
            confirm "Enable and start Docker now?" || die "Docker must be running before Depot starts."
            systemctl enable --now docker.service
        fi
        docker version >/dev/null 2>&1 || die "Docker is installed but the daemon is unavailable."
        say "[ok] Docker daemon is reachable."
        git --version
    else
        caddy version
        systemctl is-active --quiet caddy.service || warn "Caddy is installed but caddy.service is not currently active."
    fi
}

validate_safe_value() {
    value=$1
    label=$2
    [ -n "$value" ] || die "$label cannot be empty."
    case "$value" in *'"'*|*'\'*|*"
"*|*""*) die "$label contains unsupported characters." ;; esac
}

download_package() {
    temporary_root=$(mktemp -d /tmp/depot-setup.XXXXXX)
    trap 'rm -rf "$temporary_root"' EXIT HUP INT TERM
    if [ "$VERSION" = latest ]; then
        base_url="https://github.com/$REPOSITORY/releases/latest/download"
    else
        case "$VERSION" in *[!A-Za-z0-9._-]*) die "Release tag contains unsafe characters." ;; esac
        base_url="https://github.com/$REPOSITORY/releases/download/$VERSION"
    fi
    say "Downloading Depot release package ($VERSION)…"
    curl --fail --location --silent --show-error \
        "$base_url/depot-linux-x86_64.tar.gz" \
        --output "$temporary_root/depot-linux-x86_64.tar.gz"
    curl --fail --location --silent --show-error \
        "$base_url/SHA256SUMS" \
        --output "$temporary_root/SHA256SUMS"
    expected=$(awk '$2 == "depot-linux-x86_64.tar.gz" { print $1; exit }' "$temporary_root/SHA256SUMS")
    [ -n "$expected" ] || die "Release checksum manifest does not contain the Depot package."
    actual=$(sha256sum "$temporary_root/depot-linux-x86_64.tar.gz" | awk '{ print $1 }')
    [ "$actual" = "$expected" ] || die "Depot release checksum verification failed."
    say "[ok] Release checksum verified."
    mkdir "$temporary_root/package"
    tar -xzf "$temporary_root/depot-linux-x86_64.tar.gz" -C "$temporary_root/package"
    PACKAGE_ROOT=$temporary_root/package
    [ -x "$PACKAGE_ROOT/depot-linux-x86_64" ] || die "Release package does not contain an executable Depot binary."
    [ -x "$PACKAGE_ROOT/scripts/install.sh" ] || die "Release package does not contain the low-level installer."
}

generate_token() {
    if command -v openssl >/dev/null 2>&1; then
        openssl rand -hex 32
    else
        od -An -N32 -tx1 /dev/urandom | tr -d ' \n'
    fi
}

configure_vm() {
    prompt "Private listen address" "127.0.0.1:8080"
    bind=$REPLY
    prompt "Public dashboard origin" "https://depot.example.com"
    public_origin=$REPLY
    validate_safe_value "$bind" "Listen address"
    validate_safe_value "$public_origin" "Public origin"
    prompt "Caddy agent address (leave blank to configure later)" ""
    agent_address=$REPLY
    agent_section=
    if [ -n "$agent_address" ]; then
        validate_safe_value "$agent_address" "Caddy agent address"
        prompt "Shared Caddy agent token" ""
        agent_token=$REPLY
        [ "${#agent_token}" -ge 32 ] || die "The Caddy agent token must contain at least 32 characters."
        validate_safe_value "$agent_token" "Caddy agent token"
        agent_section="
[worker.caddy_agent]
address = \"$agent_address\"
auth_token = \"$agent_token\"
timeout_seconds = 30"
    fi
    umask 0027
    {
        printf '[server]\nbind = "%s"\n\n' "$bind"
        printf '[database]\nurl = "sqlite:///var/lib/depot/depot.db"\n\n'
        printf '[security]\npublic_origin = "%s"\nmaster_key_file = "/etc/depot/master.key"\n\n' "$public_origin"
        printf '[worker]\nworkspace_root = "/var/lib/depot/workspaces"\ngit_timeout_seconds = 120\ndocker_binary = "docker"\ndocker_timeout_seconds = 1800\n'
        printf '%s\n' "$agent_section"
    } > /etc/depot/config.toml
    chown depot:depot /etc/depot/config.toml
    chmod 0640 /etc/depot/config.toml
}

format_suffixes() {
    raw=$1
    SUFFIX_ARRAY=
    old_ifs=$IFS
    IFS=,
    set -- $raw
    IFS=$old_ifs
    for suffix in "$@"; do
        suffix=$(printf '%s' "$suffix" | tr -d ' ')
        [ -n "$suffix" ] || continue
        case "$suffix" in *[!A-Za-z0-9.-]*) die "Domain suffix contains unsafe characters: $suffix" ;; esac
        if [ -n "$SUFFIX_ARRAY" ]; then SUFFIX_ARRAY="$SUFFIX_ARRAY, "; fi
        SUFFIX_ARRAY="$SUFFIX_ARRAY\"$suffix\""
    done
    [ -n "$SUFFIX_ARRAY" ] || die "At least one allowed domain suffix is required."
}

configure_caddy_agent() {
    prompt "Private agent listen address" "127.0.0.1:9080"
    bind=$REPLY
    prompt "Depot VM gateway upstream" "127.0.0.1:8080"
    gateway=$REPLY
    prompt "Allowed domain suffixes (comma-separated)" "example.com"
    suffixes=$REPLY
    generated_token=$(generate_token)
    prompt "Shared agent token" "$generated_token"
    token=$REPLY
    validate_safe_value "$bind" "Agent listen address"
    validate_safe_value "$gateway" "Gateway upstream"
    validate_safe_value "$token" "Agent token"
    [ "${#token}" -ge 32 ] || die "The agent token must contain at least 32 characters."
    format_suffixes "$suffixes"
    umask 0027
    {
        printf 'bind = "%s"\n' "$bind"
        printf 'auth_token = "%s"\n' "$token"
        printf 'gateway_upstream = "%s"\n' "$gateway"
        printf 'allowed_domain_suffixes = [%s]\n' "$SUFFIX_ARRAY"
        printf 'managed_directory = "/etc/caddy/depot"\n'
        printf 'caddyfile = "/etc/caddy/Caddyfile"\n'
        printf 'caddy_binary = "caddy"\ncommand_timeout_seconds = 30\n'
    } > /etc/depot/caddy-agent.toml
    chown depot-caddy:caddy /etc/depot/caddy-agent.toml
    chmod 0640 /etc/depot/caddy-agent.toml
    say "Shared token for the Depot VM configuration: $token"
}

ensure_caddy_import() {
    caddyfile=/etc/caddy/Caddyfile
    [ -f "$caddyfile" ] || die "$caddyfile does not exist."
    if grep -Eq '^[[:space:]]*import[[:space:]]+depot/\*\.caddy[[:space:]]*$' "$caddyfile"; then
        say "[ok] Depot managed routes are already imported by Caddy."
        return
    fi
    if confirm "Append 'import depot/*.caddy' to $caddyfile (a backup will be made)?"; then
        cp -p "$caddyfile" "$caddyfile.depot-before-import"
        printf '\n# Routes managed by Depot; keep manual routes outside this directory.\nimport depot/*.caddy\n' >> "$caddyfile"
    else
        die "Add 'import depot/*.caddy' to $caddyfile, then rerun setup."
    fi
}

install_release_files() {
    "$PACKAGE_ROOT/scripts/install.sh" "$ROLE" "$PACKAGE_ROOT/depot-linux-x86_64"
}

run_vm_checks() {
    runuser -u depot -- /usr/local/bin/depot migrate --config /etc/depot/config.toml
    runuser -u depot -- /usr/local/bin/depot doctor --config /etc/depot/config.toml
}

run_agent_checks() {
    runuser -u depot-caddy -- /usr/local/bin/depot caddy-agent --config /etc/depot/caddy-agent.toml --check
}

fresh_install() {
    require_root
    infer_role
    install_dependencies
    if [ "$ROLE" = vm ]; then config_path=/etc/depot/config.toml; else config_path=/etc/depot/caddy-agent.toml; fi
    had_config=0
    [ -f "$config_path" ] && had_config=1
    download_package
    install_release_files
    if [ "$had_config" -eq 0 ]; then
        if [ "$ROLE" = vm ]; then configure_vm; else configure_caddy_agent; fi
    else
        say "Preserved existing configuration: $config_path"
    fi
    if [ "$ROLE" = vm ]; then
        run_vm_checks
        systemctl enable --now depot.service depot-worker.service
        systemctl is-active --quiet depot.service || die "depot.service did not start. Check its journal."
        systemctl is-active --quiet depot-worker.service || die "depot-worker.service did not start. Check its journal."
        say "[ok] Depot services started."
        runuser -u depot -- /usr/local/bin/depot setup --config /etc/depot/config.toml --expires-in-minutes 30
    else
        ensure_caddy_import
        run_agent_checks
        systemctl enable --now caddy.service depot-caddy-agent.service
        systemctl is-active --quiet caddy.service || die "caddy.service did not start. Check its journal."
        systemctl is-active --quiet depot-caddy-agent.service || die "depot-caddy-agent.service did not start. Check its journal."
        say "[ok] Caddy and the Depot agent started."
    fi
    system_status
}

backup_vm_state() {
    stamp=$(date -u +%Y%m%dT%H%M%SZ)
    BACKUP_DIRECTORY=/var/backups/depot/pre-update-$stamp
    install -d -m 0700 "$BACKUP_DIRECTORY"
    [ -f /usr/local/bin/depot ] && cp -p /usr/local/bin/depot "$BACKUP_DIRECTORY/depot.previous"
    [ -f /etc/depot/config.toml ] && cp -p /etc/depot/config.toml "$BACKUP_DIRECTORY/config.toml"
    [ -f /etc/depot/master.key ] && cp -p /etc/depot/master.key "$BACKUP_DIRECTORY/master.key"
    for database_file in /var/lib/depot/depot.db /var/lib/depot/depot.db-wal /var/lib/depot/depot.db-shm; do
        [ -f "$database_file" ] && cp -p "$database_file" "$BACKUP_DIRECTORY/"
    done
    say "Backup created at $BACKUP_DIRECTORY"
}

update_installation() {
    require_root
    infer_role
    install_dependencies
    [ -x /usr/local/bin/depot ] || die "Depot is not installed; run depot-setup install."
    download_package
    if [ "$ROLE" = vm ]; then
        systemctl stop depot-worker.service depot.service
        backup_vm_state
    else
        systemctl stop depot-caddy-agent.service
        cp -p /usr/local/bin/depot /usr/local/bin/depot.previous
    fi
    install_release_files
    if [ "$ROLE" = vm ]; then
        if ! run_vm_checks; then
            warn "Update checks failed. Services remain stopped; restore from $BACKUP_DIRECTORY after diagnosing the failure."
            exit 1
        fi
        systemctl start depot.service depot-worker.service
        systemctl is-active --quiet depot.service || die "Updated depot.service did not start. Check its journal."
        systemctl is-active --quiet depot-worker.service || die "Updated depot-worker.service did not start. Check its journal."
    else
        if ! run_agent_checks; then
            warn "Update checks failed. The agent remains stopped and /usr/local/bin/depot.previous is available."
            exit 1
        fi
        systemctl start depot-caddy-agent.service
        systemctl is-active --quiet depot-caddy-agent.service || die "Updated agent did not start. Check its journal."
    fi
    say "[ok] Depot update completed."
    system_status
}

check_installation() {
    infer_role
    install_dependencies
    [ -x /usr/local/bin/depot ] || die "Depot is not installed."
    /usr/local/bin/depot --version
    if [ "$ROLE" = vm ]; then
        [ -r /etc/depot/config.toml ] || die "Depot VM configuration is missing."
        [ -r /etc/depot/master.key ] || die "Depot master key is missing."
        systemctl is-enabled --quiet depot.service || warn "depot.service is not enabled."
        systemctl is-enabled --quiet depot-worker.service || warn "depot-worker.service is not enabled."
        systemctl is-active --quiet depot.service || die "depot.service is not active."
        systemctl is-active --quiet depot-worker.service || die "depot-worker.service is not active."
        configured_bind=$(awk '
            /^\[server\]$/ { server = 1; next }
            /^\[/ { server = 0 }
            server && $1 == "bind" { value = $3; gsub(/"/, "", value); print value; exit }
        ' /etc/depot/config.toml)
        [ -n "$configured_bind" ] || die "server.bind could not be read from the configuration."
        case "$configured_bind" in 0.0.0.0:*) health_bind="127.0.0.1:${configured_bind##*:}" ;; *) health_bind=$configured_bind ;; esac
        curl --fail --silent --show-error "http://$health_bind/api/v1/system/health" >/dev/null ||
            die "Depot's HTTP health endpoint is unavailable at $health_bind."
        curl --fail --silent --show-error "http://$health_bind/api/v1/system/readiness" >/dev/null ||
            die "Depot's readiness endpoint reports a failure at $health_bind."
        docker version >/dev/null
    else
        [ -r /etc/depot/caddy-agent.toml ] || die "Caddy agent configuration is missing."
        systemctl is-active --quiet caddy.service || die "caddy.service is not active."
        systemctl is-active --quiet depot-caddy-agent.service || die "depot-caddy-agent.service is not active."
        runuser -u depot-caddy -- /usr/local/bin/depot caddy-agent --config /etc/depot/caddy-agent.toml --check
    fi
    say "[ok] Depot $ROLE checks passed."
}

service_names() {
    infer_role
    if [ "$ROLE" = vm ]; then SERVICES="depot.service depot-worker.service"; else SERVICES="depot-caddy-agent.service"; fi
}

system_status() {
    service_names
    systemctl --no-pager --full status $SERVICES || true
}

service_action() {
    action=$1
    require_root
    service_names
    systemctl "$action" $SERVICES
    [ "$action" = stop ] || system_status
}

if [ -z "$COMMAND" ]; then
    if [ -x /usr/local/bin/depot ]; then
        say "Depot is already installed."
        say "  1) Check installation"
        say "  2) Update"
        say "  3) Restart services"
        prompt "Action" "1"
        case "$REPLY" in 1) COMMAND=check ;; 2) COMMAND=update ;; 3) COMMAND=restart ;; *) die "Invalid action." ;; esac
    else
        COMMAND=install
    fi
fi

case "$COMMAND" in
    install) fresh_install ;;
    update) update_installation ;;
    check) check_installation ;;
    start|stop|restart) service_action "$COMMAND" ;;
    status) service_names; system_status ;;
esac
