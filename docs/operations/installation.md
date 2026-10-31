# VM installation and deployment smoke test

This guide installs the single Depot binary as separate API/gateway and worker
services on a Debian- or Ubuntu-like VM. Run the Caddy-agent section on the
Proxmox host, not inside the Depot VM.

## 1. Prepare the VM

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
root-equivalent; use a dedicated VM.

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

On the Proxmox/Caddy host, run:

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
