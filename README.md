# Depot

Depot is a self-hosted deployment platform for one Linux VM. A single Rust
executable supplies the control-plane API, embedded dashboard, private HTTP
gateway, durable worker, and restricted host-side Caddy agent.

Depot is under active development. Public Git inspection, Dockerfile builds,
container readiness, project domains, and Host-based gateway routing are
implemented. Railpack, static artifact releases, provider-signed webhooks,
private Git credentials, secrets, and rollback are not complete yet. GitHub
Actions and other CI systems can trigger exact-commit deployments with a
project-scoped bearer token.

## Quick start: main Depot binary

### 1. Install build and runtime prerequisites

The build machine needs Rust 1.91 or newer, Node.js, pnpm, and `just`. The Depot
VM needs Docker Engine and Git at runtime. The production host does not need a
Node.js process or frontend asset directory.

```sh
pnpm --dir web install --frozen-lockfile
just release
./target/release/depot --version
```

`just release` builds the Vite dashboard before compiling it into
`target/release/depot`.

### 2. Create the VM configuration

```sh
sudo install -d -m 0750 /etc/depot /var/lib/depot/workspaces
sudo install -m 0640 deploy/config.example.toml /etc/depot/config.toml
sudo editor /etc/depot/config.toml
```

Set these values for the installation:

- `server.bind`: a private VM address reachable by host Caddy, not a public
  wildcard listener unless a firewall restricts it.
- `database.url`: the persistent SQLite location.
- `security.public_origin`: the HTTPS dashboard origin.
- `worker.workspace_root`: scratch storage with enough space for Git checkouts.
- `worker.docker_binary`: the Docker CLI executable available to the worker.
- `worker.caddy_agent`: the private agent address and shared reconciliation
  token; omit this table only when domains will not be reconciled.

The user running `depot worker` needs narrowly scoped Docker access. Membership
in the Docker group is effectively root-equivalent on the VM and should be
treated accordingly.

### 3. Initialize storage and the first administrator

```sh
sudo install -d -m 0750 /var/lib/depot
depot migrate --config /etc/depot/config.toml
depot doctor --config /etc/depot/config.toml
depot setup --config /etc/depot/config.toml --expires-in-minutes 30
```

Open `security.public_origin`, enter the setup token printed once by `setup`,
and create the first administrator. There is no default password.

### 4. Run the API/gateway and worker

Use two supervised processes invoking the same executable:

```sh
depot serve --config /etc/depot/config.toml
depot worker --config /etc/depot/config.toml
```

For a worker smoke check that claims at most one job:

```sh
depot worker --config /etc/depot/config.toml --once
```

Configure projects in the dashboard, inspect their repositories, select the
Dockerfile builder, set the container port and readiness path, then deploy.
Successful services are published on ephemeral loopback ports and selected by
the internal gateway through exact project-domain matches.

### 5. Deploy with curl or GitHub Actions

Create a project-scoped token on the Depot VM. Project and user IDs are exposed
by the authenticated API and dashboard. The plaintext token is printed once;
Depot stores only its SHA-256 digest.

```sh
depot deploy-token \
  --config /etc/depot/config.toml \
  --project-id 019a... \
  --created-by-user-id 019a... \
  --label "GitHub Actions"
```

Store the result as `DEPOT_DEPLOY_TOKEN`, then deploy a full 40-character
commit SHA:

```sh
curl --fail-with-body --silent --show-error \
  --request POST "https://depot.example.com/api/v1/deploy" \
  --header "Authorization: Bearer ${DEPOT_DEPLOY_TOKEN}" \
  --header "Content-Type: application/json" \
  --data "{\"commit_sha\":\"$(git rev-parse HEAD)\"}"
```

The API returns `202 Accepted` after durably enqueueing the deployment. The
worker checks out the configured branch and rejects the job if that branch does
not resolve to the requested SHA. Keep the token in a CI secret, never in a
repository variable or workflow file.

The repository includes CI builds, tagged Linux binary releases, and a reusable
curl deployment workflow under `.github/workflows/`. Copy `deploy.yml` into the
application repository (so `github.sha` identifies that application), add
`DEPOT_DEPLOY_TOKEN` as a GitHub Actions secret, and provide the Depot URL when
dispatching it.

### 6. Connect host Caddy

Caddy runs on the Proxmox host and forwards public HTTPS traffic to the single
private `server.bind` gateway. Follow
[the Caddy agent quick start](docs/operations/caddy-agent.md).

## Verification

Run the repository quality gate with:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
pnpm --dir web exec biome check src
pnpm --dir web exec tsc -b --pretty false
just release
```

Docker and Caddy are unavailable in the current development sandbox. Their CLI
boundaries, validation, cleanup, and rollback behavior are covered with fake
executables; live engine checks must be completed on the target hosts.
