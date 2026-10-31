# VM installation and deployment smoke test

This guide installs the single Depot binary as separate API/gateway and worker
services on a Debian- or Ubuntu-like VM. Run the Caddy-agent section on the
Proxmox host, not inside the Depot VM.

## 1. Prepare the VM

### Guided installation (recommended)

On a fresh server, use the interactive setup:

```sh
curl -fsSL https://github.com/aosasona/depot/releases/latest/download/depot-setup.sh | sudo sh
```

Choose **Depot VM**. The setup verifies Linux x86-64, systemd, download and
checksum tools, SQLite, Git, and the Docker CLI/daemon. When a supported
`apt`, `dnf`, or `yum` package manager is available, it lists missing
dependencies and asks before installing them. It never changes firewall or DNS
configuration. It then downloads the release package and `SHA256SUMS` from the
same GitHub release, verifies the archive, installs the binary and units,
creates the encryption key, gathers the minimum configuration, migrates and
checks SQLite, and starts both services. It refuses wildcard listeners, shows a
final plan, and validates a temporary configuration before replacing anything.
After startup it makes an authenticated request to the configured Caddy-agent
IP address or DNS name. A first-host installation may explicitly defer that
peer check; `check` and `update` do not report success until it passes.

The installed administration command provides the normal lifecycle:

```sh
sudo depot-setup check
sudo depot-setup status
sudo depot-setup restart
sudo depot-setup update
```

An update preserves configuration, stops the API and worker, creates a dated
pre-update backup under `/var/backups/depot`, verifies the new package, applies
migrations, runs `doctor`, and starts only after those checks succeed. On a
failed check it leaves the services stopped and prints the recovery location.
The update also requires successful end-to-end peer reachability.

### Manual installation

Install Docker Engine and Git using the operating system or Docker's official
packages. Verify the daemon before granting Depot access:

```sh
sudo docker version
git --version
```

Download a release asset named `depot-linux-x86_64`, verify its provenance,
and make it executable. From a source checkout, install it with:

```sh
chmod +x depot-linux-x86_64
sudo ./scripts/install.sh vm ./depot-linux-x86_64
```

The installer creates separate unprivileged `depot` and `depot-worker`
accounts, shared persistent directories, two systemd units, and an example
configuration when one does not already exist. If the Docker group exists, it
adds only `depot-worker` to that group. Docker daemon access is effectively
root-equivalent; use a dedicated VM. It also creates `/etc/depot/master.key`
once with restricted permissions. Back up that key separately from SQLite;
stored runtime values cannot be recovered without it.

Never store `master.key` beside an unencrypted SQLite backup. A minimal backup
set contains `/var/lib/depot/depot.db` (captured with SQLite's backup command)
and `/etc/depot/master.key` in separate protected locations. Restore the key
with owner `root`, group `depot`, and mode `0640` before starting either service.

Edit `/etc/depot/config.toml`, then initialize and check storage:

```sh
sudo -u depot sh -c 'umask 0007; /usr/local/bin/depot migrate --config /etc/depot/config.toml'
sudo -u depot /usr/local/bin/depot doctor --config /etc/depot/config.toml
sudo -u depot /usr/local/bin/depot setup \
  --config /etc/depot/config.toml \
  --expires-in-minutes 30
```

Start the services only after reviewing the private bind address, public origin,
data paths, and Caddy-agent token:

```sh
sudo systemctl enable --now depot.service depot-worker.service
sudo systemctl status depot.service depot-worker.service
curl --fail http://127.0.0.1:8080/api/v1/system/health
curl --fail http://127.0.0.1:8080/api/v1/system/readiness
```

Replace `127.0.0.1:8080` with the configured private bind address when testing
from the Caddy host. Do not expose the listener directly to the public internet.

## 2. Install the host Caddy agent

The same guided command can configure the Caddy host. Choose **Caddy agent**:

```sh
curl -fsSL https://github.com/aosasona/depot/releases/latest/download/depot-setup.sh | sudo sh
```

It verifies Caddy and systemd, offers supported package installation when
needed, prompts for the private listener, fixed Depot gateway, allowed domain
suffixes, and shared token, and asks before appending the isolated
`import depot/*.caddy` line. It backs up the operator-owned Caddyfile first and
runs a complete Caddy validation without changing the current Depot route set.
Manual Caddy records outside `/etc/caddy/depot/routes.caddy` remain untouched.

For a manual agent installation on the Proxmox/Caddy host, run:

```sh
chmod +x depot-linux-x86_64
sudo ./scripts/install.sh caddy-agent ./depot-linux-x86_64
sudo editor /etc/depot/caddy-agent.toml
```

Add `import depot/*.caddy` to the operator-owned Caddyfile and validate it as
described in [the Caddy-agent guide](caddy-agent.md). The installer deliberately
does not edit the Caddyfile, firewall, or authentication token.

```sh
sudo systemctl enable --now depot-caddy-agent.service
sudo systemctl status depot-caddy-agent.service
```

## 3. Run a real deployment smoke test

This repository contains a small non-root HTTP fixture under
`examples/smoke-service`. In Depot, create a project with these settings:

- repository: `https://github.com/aosasona/depot.git`
- branch: `main`
- project kind: `service`
- builder: `dockerfile`
- context path: `examples/smoke-service`
- Dockerfile path: `Dockerfile`
- exposed port: `8080`
- health path: `/`

Inspect, then deploy the project from the dashboard. Alternatively, issue a
deploy token and send the current full commit SHA to `/api/v1/deploy` using the
README curl example. Watch both services while it runs:

```sh
sudo journalctl -fu depot.service
sudo journalctl -fu depot-worker.service
sudo docker ps --filter label=depot.deployment
```

Attach a test hostname and point its DNS record at the Caddy host. Verify every
hop independently:

```sh
curl --fail -H 'Host: smoke.example.com' http://DEPOT_VM_PRIVATE_IP:8080/
curl --fail --resolve smoke.example.com:443:CADDY_PUBLIC_IP \
  https://smoke.example.com/
```

Success means the response contains `Depot smoke service is ready.` and the
deployment is marked successful. The second command also validates Caddy's TLS
route; it requires working public DNS/ACME reachability or an already issued
certificate.

## 4. Update and rollback the Depot binary

The recommended update is:

```sh
sudo depot-setup update
```

The manual equivalent follows.

Keep the previous binary until the new version passes `doctor`. Stop the worker
before replacing the executable so it cannot begin new deployment work midway
through an update:

```sh
sudo systemctl stop depot-worker.service depot.service
sudo install -m 0755 /usr/local/bin/depot /usr/local/bin/depot.previous
sudo install -m 0755 depot-linux-x86_64 /usr/local/bin/depot
sudo -u depot /usr/local/bin/depot migrate --config /etc/depot/config.toml
sudo -u depot /usr/local/bin/depot doctor --config /etc/depot/config.toml
sudo systemctl start depot.service depot-worker.service
```

Database migrations are forward-only. Restoring an older binary may also
require restoring the matching SQLite backup; take a backup before upgrading.

## Environment limitations of the repository test run

The development sandbox does not provide a Docker daemon, systemd as PID 1, a
Caddy installation, public DNS, or ACME access. Staged installation, unit-file
verification where available, fake Docker/Caddy boundary tests, API tests, and
the release build run in CI. Complete the live commands above on the target VM
and host before treating the installation as production-ready.
