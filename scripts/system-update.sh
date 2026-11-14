#!/bin/sh
set -eu

STATE_DIR=/var/lib/depot/updater
INBOX=$STATE_DIR/inbox
REQUEST=$INBOX/requested
STATUS=$STATE_DIR/status.json
LOG=$STATE_DIR/update.log
BACKUP_BEFORE=

write_status() {
    state=$1
    message=$2
    updated_at=$(date +%s)
    temporary=$STATE_DIR/status.new
    printf '{"state":"%s","message":"%s","updated_at":%s}\n' \
        "$state" "$message" "$updated_at" > "$temporary"
    chmod 0644 "$temporary"
    mv "$temporary" "$STATUS"
}

failed() {
    code=$?
    [ "$code" -ne 0 ] || code=1
    trap - EXIT HUP INT TERM
    backup_after=$(latest_backup)
    if [ -n "$backup_after" ] && [ "$backup_after" != "$BACKUP_BEFORE" ] && restore_backup "$backup_after"; then
        write_status failed "Update failed and Depot was restored to the previous release. Review the update log before retrying."
    else
        systemctl start depot.service depot-worker.service >/dev/null 2>&1 || true
        write_status failed "Update failed. Review /var/lib/depot/updater/update.log and the latest pre-update backup."
    fi
    exit "$code"
}

latest_backup() {
    latest=
    for candidate in /var/backups/depot/pre-update-*; do
        [ -d "$candidate" ] && latest=$candidate
    done
    printf '%s\n' "$latest"
}

restore_backup() {
    backup=$1
    [ -x "$backup/depot.previous" ] || return 1
    [ -f "$backup/depot.db" ] || return 1
    systemctl stop depot-worker.service depot.service >/dev/null 2>&1 || true
    install -m 0755 "$backup/depot.previous" /usr/local/bin/depot || return 1
    [ ! -f "$backup/config.toml" ] || install -o depot -g depot -m 0640 "$backup/config.toml" /etc/depot/config.toml || return 1
    [ ! -f "$backup/master.key" ] || install -o root -g depot -m 0640 "$backup/master.key" /etc/depot/master.key || return 1
    rm -f /var/lib/depot/depot.db /var/lib/depot/depot.db-wal /var/lib/depot/depot.db-shm
    for database_file in "$backup"/depot.db*; do
        [ -f "$database_file" ] || continue
        install -o depot -g depot -m 0660 "$database_file" /var/lib/depot/"$(basename "$database_file")" || return 1
    done
    systemctl start depot.service depot-worker.service >/dev/null 2>&1 || return 1
    systemctl is-active --quiet depot.service && systemctl is-active --quiet depot-worker.service
}

install -d -o root -g root -m 0755 "$STATE_DIR"
install -d -o depot -g depot -m 0700 "$INBOX"
[ -f "$REQUEST" ] || exit 0
BACKUP_BEFORE=$(latest_backup)
rm -f "$REQUEST"
: > "$LOG"
chmod 0644 "$LOG"
trap failed EXIT HUP INT TERM
write_status running "Downloading, verifying, backing up, and installing the latest official release."

/usr/local/sbin/depot-setup update --role vm --yes >> "$LOG" 2>&1

trap - EXIT HUP INT TERM
write_status succeeded "Update completed. Depot services and the Caddy agent connection passed verification."
