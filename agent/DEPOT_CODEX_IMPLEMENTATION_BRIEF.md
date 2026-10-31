# DEPOT — Codex Implementation Brief

> **Mission:** Build Depot v0.1, a real, self-hosted deployment platform for applications and static sites running on one dedicated Linux VM. Deliver a polished product that can authenticate users, build from Git, deploy safely, expose HTTPS domains through an existing host-level Caddy installation, and redeploy automatically on pushes.
>
> **Non-negotiable delivery constraint:** Ship Depot as **one standalone Rust executable** with its React UI embedded. Do not require a Node.js server, separate API binary, or frontend asset directory at runtime. External operational dependencies (Docker Engine, Git, build tools invoked in isolated builders, and an existing Caddy host) are permitted and must be documented. A single binary does **not** mean a single process: the same executable may be launched multiple times with different subcommands and systemd units.

## 0. Operating instructions for Codex

You are implementing a product, not creating a design-only prototype, slideshow, or disconnected scaffold. First inspect the existing repository (if any) and adapt without deleting working code. If the repository is empty, scaffold it. Plan the work, then execute it in independently testable milestones. Implement a complete vertical slice before expanding breadth. Do not silently substitute mocked business logic for real Docker, Git, webhooks, or Caddy integration.

- Before coding, write a short implementation plan and record decisions in `docs/architecture/decisions/` as ADRs.
- Prefer stable, maintained dependencies; pin versions and commit lockfiles. Do not assume a package exists simply because it is listed here: verify package availability and APIs first.
- If the environment cannot exercise Docker, real webhooks, or Caddy, implement the integration and cover it with mocks/integration tests; explicitly document which live checks remain unverified.
- Build the smallest secure working implementation, not premature abstractions or a distributed control plane.
- Keep the repository approachable for a developer who may need to investigate or extend it manually.
- Never quietly drop a requirement. Record any blocked item, its reason, and the exact steps to complete it.

## 1. Product context and scope

The user runs Proxmox. Caddy runs **on the Proxmox host**, not in the Depot VM. `depot` is **one VM for personal projects**, not a Proxmox orchestrator. Depot's application deployment and API run in this VM. It must not require root-level API access to Proxmox.

**v0.1 success scenario:** On a new installation, an admin completes initial setup, creates a member account, and that member imports a Git repository, configures and deploys a Rust/Node/web project, obtains a working domain through host Caddy, views build/runtime logs, pushes a new commit, and sees a new release go live automatically. A static frontend must remain hosted without a dedicated application container or Node process after the build completes.

### In scope for v0.1

1. First-run bootstrap; user authentication; local admin/member accounts; user administration.
2. Project CRUD; selected repository, branch, build configuration, and run configuration.
3. Public Git repository clone; private Git via SSH deploy credentials; GitHub-compatible webhook auto-deploy with signature validation and branch filtering.
4. Dockerfile builds and Railpack builds. Explicit Dockerfile and builder overrides; project detection with inspectable build plan. Respect mise tool/version declarations where the chosen builder supports them, without claiming full support where it doesn't.
5. Runtime services as OCI containers managed through a local Docker Engine. Container creation, health/readiness, restart, stop, resource limits and logs.
6. Static projects: build in an ephemeral isolated builder; extract verified output; serve from one shared static HTTP server embedded in Depot. **No permanently running container for each static website.**
7. One internal HTTP gateway in Depot routes incoming requests by validated Host header to the appropriate service or static release.
8. Host-level Caddy integration via a deliberately narrow `depot caddy-agent` mode of the **same binary**, installed and run separately on the Proxmox host. Host Caddy obtains and renews certificates.
9. Runtime environment variables and encrypted secrets. Build-time secrets only through protected builder mechanisms; never embed secrets in images, logs, or public responses.
10. Durable deployments and job queue, live and historical build/runtime logs, deployment history, health checks, rollback to retained successful releases, basic restart recovery.
11. Functional, responsive, polished dashboard; project details; deployment details; settings; users; forms; errors and empty states.
12. Install/build documentation, unit and integration tests, configuration examples, formatting/linting/typechecking, CI, and a verified single-file release build.

### Explicitly out of scope for v0.1

- Managed PostgreSQL/Redis/Valkey/S3 provisioning and credential issuance.
- PR/branch preview environments (but avoid data models that make future environments impossible).
- Firecracker, alternative execution engines, Kubernetes, Docker Swarm, autoscaling, multi-VM scheduling.
- Spawning Proxmox VMs/LXCs or connecting extra Depot nodes.
- Multi-tenant organisations, elaborate permission matrix, third-party OAuth/SSO.
- Full Docker Compose support or app marketplaces.
- Automatic DNS record creation (document prerequisite DNS setup).
- Promising zero-downtime deployments when unavailable; implement readiness-first switching and safe rollback, and clearly communicate brief interruption risks for stateful or single-instance services.

## 2. Technical stack and distribution

**Backend:** Rust stable; Axum; Tokio; Serde; SQLx/SQLite; tracing; `thiserror` for typed errors; `anyhow` only near app/process boundaries; mature crates for password hashing, encryption and secure cookies; a maintained Docker API client or well-contained Docker CLI adapter (prefer API for lifecycle). Document dependency decisions.

**Frontend:** React + TypeScript + Vite + Tailwind CSS. TanStack tools wherever suited:

- **TanStack Router:** typed nested routes and route-level loading patterns.
- **TanStack Query:** server state, caching, mutation lifecycle, polling, invalidation, optimistic UI only when safe.
- **TanStack Form:** typed forms and validation integration.
- **TanStack Table:** data tables for projects, deployments, users, with accessible sorting/filtering/pagination.
- **TanStack Virtual:** long logs and large lists when performance warrants it.
- Other TanStack packages **only if stable and genuinely useful**; do not invent a dependency named “TanStack UI”. Build a small, explicit accessible UI component kit rather than pulling in a giant visual framework.

Use Zod (or equivalent well-maintained runtime schema validation) for API-facing form values where appropriate. Client types must stay in sync with the Rust API through generated OpenAPI/types or a documented type-generation solution. Avoid silently duplicating and drifting data contracts.

**Formatting/linting:** Biome for frontend formatting + linting (if current plugins satisfy actual needs); `tsc --noEmit` with strict mode; `cargo fmt --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo test`; check Rust documentation where practical. Optionally add an accessibility lint rule set if Biome's coverage is insufficient, without introducing redundant conflicting formatters.

**Packaging:** Build Vite assets, then embed them into the Rust binary (e.g. using an established embed crate or generated include artifacts). Route `/api/*`, event streams and webhooks before SPA fallback; serve hashed assets with immutable caching while ensuring `index.html` isn't stale. No Node runtime in production. Provide a production build command and release CI for Linux x86_64; evaluate musl compatibility honestly, especially SQLite/OpenSSL, and don't claim static linking if unverified. Verify `file` output and an executable smoke test. Support systemd units invoking the same binary with appropriate subcommands.

Suggested CLI, refined if necessary:

```sh
depot setup                      # secure initial configuration and first-admin bootstrap
depot serve --config /etc/depot/config.toml
depot worker --config /etc/depot/config.toml
depot caddy-agent --config /etc/depot/caddy-agent.toml
depot migrate --config /etc/depot/config.toml
depot doctor --config /etc/depot/config.toml
```

`depot serve` may embed the worker for simple installs **only if isolation and graceful lifecycle remain explicit**. Preferred initial deployment: API and worker as separate systemd units invoking the same executable; routing agent is a third installation of the same executable on the Proxmox host. Do not copy the frontend source to the host.

## 3. Repository organisation: readability is a product requirement

Organise by cohesive capability and feature, not one enormous `main.rs`, `App.tsx`, `Dashboard.tsx`, or `api.ts` file. An illustrative structure (adjust to idiomatic implementation):

```text
depot/
├── Cargo.toml
├── Cargo.lock
├── crates/
│   ├── depot/                # single distributable binary / CLI
│   ├── depot-core/           # domain models and interfaces
│   ├── depot-api/            # Axum router, handlers, middleware, DTOs
│   ├── depot-store/          # SQLite repos, migrations, durable jobs
│   ├── depot-auth/           # users, passwords, sessions, roles
│   ├── depot-git/            # repository checkout, webhooks, credentials
│   ├── depot-build/          # Dockerfile/Railpack build orchestration
│   ├── depot-runtime/        # Docker runtime + release transitions
│   ├── depot-routing/        # internal gateway and static hosting
│   ├── depot-secrets/        # encryption and injection
│   └── depot-caddy/          # restricted agent and client
├── web/
│   ├── src/
│   │   ├── app/             # root providers, router, app shell
│   │   ├── routes/          # thin route components
│   │   ├── features/
│   │   │   ├── auth/
│   │   │   ├── dashboard/
│   │   │   ├── projects/
│   │   │   ├── deployments/
│   │   │   ├── domains/
│   │   │   ├── secrets/
│   │   │   ├── users/
│   │   │   └── settings/
│   │   ├── components/ui/   # small accessible UI primitives
│   │   ├── components/layout/
│   │   ├── lib/api/         # HTTP client, schemas, API types
│   │   ├── lib/utils/
│   │   ├── styles/          # tokens, CSS layers, global styles
│   │   └── test/
│   ├── package.json
│   ├── biome.json
│   └── tsconfig.json
├── migrations/
├── deploy/                   # systemd and install templates
├── docs/
│   ├── architecture/
│   │   └── decisions/
│   ├── operations/
│   └── development/
├── scripts/
└── .github/workflows/
```

**Frontend file discipline:** Routes compose feature modules, not implement all API calls and UI inline. Separate page layout, project cards, deployment tables, reusable status indicators, log viewer, forms, and hooks into logical modules. Co-locate feature hooks and schemas with the corresponding feature. Avoid single-file component libraries. Split whenever a file has multiple unrelated responsibilities, not via an arbitrary hard line limit. Don't introduce excessive wrapper layers or one-function files without good reason.

**Function documentation:** Every named Rust function/method, and every named TypeScript function, component and hook, should have a concise **top-level documentation comment above its definition**. Rust public interfaces use `///`; private functions also get concise `///` or appropriate doc comments; TS uses JSDoc `/** ... */`. Explain purpose and non-obvious invariants/side effects, not tautologies. Document security-sensitive functions with trust assumptions and failure modes. Prefer descriptive names, small functions, explicit return/error types and early returns to deeply nested flow. Document modules and complex data structures as needed. Tests should be legible and use descriptive names.

Example:

```rust
/// Atomically promotes a verified release after its health check passes.
/// Leaves the previously active release intact if activation fails.
async fn activate_release(/* ... */) -> Result<(), DeploymentError> {
    // ...
}
```

```ts
/** Renders a project summary and fetches status through the project query hook. */
export function ProjectOverviewPage() {
  // ...
}
```

Follow the language's native style. Prefer straightforward, idiomatic code to clever abstraction; avoid sprawling macros, magic runtime reflection, implicit global state, unchecked `unwrap()`, and unnecessary generic frameworks. Use structured error categories and helpful user-facing messages. Keep comments correct when code changes.

## 4. Approved visual direction — MUST IMPLEMENT

**Use the approved generated Depot UI reference supplied with the task as the aesthetic target.** The design has bold, almost display-type typography, a black/lime palette, zero rounded corners, crisp outlined rectangular panels, compact mono technical text, a clear sidebar, striking metric blocks, clean logs, and dense but readable operational information. It is playful and bold, not a generic corporate dashboard. The illustration is inspiration, not a pixel-perfect specification or a source of factual app data.

Design tokens (adjust only for measured contrast/accessibility):

```css
:root {
  --bg: #0d0d0d;
  --surface: #171717;
  --surface-raised: #1d1d1d;
  --surface-hover: #252525;
  --border: #343434;
  --text: #f2f2f2;
  --muted: #a4a4a4;
  --accent: #c6f547;
  --accent-foreground: #101010;
  --success: #8edb91;
  --warning: #f0bd65;
  --danger: #f36c6c;
  --radius: 0px;
}
```

- **Border radius MUST be `0` across the app**, including inputs, menus, dialogs, avatars, badges, tooltips, charts, scroll containers, and all nested components. No rounded pills or circles masquerading as status chips. Use square status dots if needed.
- All backgrounds should be **neutral black/grey**, not green-tinted charcoal. Lime is the main visual accent, not the base background tint.
- Big bold playful display face for major headings and branding (evaluate Bricolage Grotesque ExtraBold / Space Grotesk Heavy / another suitable licensed web font). Use clear sans-serif for UI and a monospace family for hashes, commands, metadata, logs and numbers. Choose and properly bundle licensed web-font files; do not depend on external font CDNs for local/offline hosting.
- Strong typography: oversized headline, high-contrast numeric KPIs, compact upper labels; use consistent scale rather than random font sizing.
- Flat blocks with precise 1px borders; no blurry drop shadows; understated grid/line geometric decorations where they improve composition.
- Hover and focus states must remain highly visible; keyboard navigation, contrast and responsive layouts must be functional.
- Use lime prominently for selected nav, primary actions, certain statistic panels and progress, but retain semantic red/amber/green for failures/warnings/health rather than treating all statuses as lime.
- No placeholder/demo deployment rows in production views. Populate views from real API state; create thoughtful, intentional empty states.
- Design desktop-first but fully usable on mobile/small screens (sidebar becomes an accessible navigation drawer; tables scroll or adapt).

Required screens: bootstrap admin, login, dashboard, projects listing, new project wizard, project overview, deployment history, deployment details/build logs/runtime logs, domains, environment/secrets, users (admin), instance settings, not-found/error states. Use a real reusable layout and UI primitives. Keep loading/skeleton/error states visually integrated.

## 5. Domain model and persistence

Use migrations and clear schema conventions; SQLite in WAL mode with appropriate busy timeout and transactional data access. Suggested entities:

- `users`: id, email/username, password hash, role, active flag, created/updated timestamps.
- `sessions`: opaque hashed session token / identifier, user id, expiry, revocation, creation metadata.
- `projects`: id, slug, name, owner, repository URL, provider, branch, creation metadata.
- `project_members` **only if needed for v0.1**; otherwise members can manage all projects, with explicit documentation of the policy.
- `service_configs`: project, static/service type, build type, Dockerfile/build paths, repo subdirectory, build/start options, exposed port, healthcheck, resource limits.
- `deployments`: project/service, commit SHA, trigger, stage/status, immutable image digest or static artifact release ID, timestamps, failure reason, active release indication.
- `deployment_events` / `job_events`: ordered event/log records and progress state.
- `jobs`: durable type, payload reference, queue state, retry state, lease owner/time, deduplication key.
- `domains`: normalized hostname, owning service, desired/actual routing state, routing agent acknowledgment/errors.
- `secrets`: encrypted values, name, scope, rotation metadata (never plaintext in SQL rows).
- `git_webhooks`: provider, project, secret reference, last delivery/result metadata.
- `audit_log`: user/admin actions, secret changes, routing changes, key security events (never secret values).

Plan ahead for `environment_id` and `target_id` concepts only insofar as their future addition won't require an entire rewrite. Don't build an unused generalized distributed scheduler.

Database write operations changing an active release should be transactional where possible. For filesystem/network side effects, implement a durable reconciliation process with idempotent steps and compensation, not fictional ACID semantics across SQLite/Docker/Caddy.

## 6. Authentication, RBAC, and bootstrap

- No default admin password. First-run setup only available when the instance is uninitialized and must use an explicit secure bootstrap flow/token (or CLI setup). Lock out unauthenticated setup immediately after initial admin creation.
- Password hashes use Argon2id with defensible parameters; throttle login and reset/credential attempts; never log passwords or session secrets.
- HttpOnly, Secure (behind HTTPS), SameSite cookies with server-side revocation and expiry. Handle reverse-proxy trusted headers deliberately, not blindly. CSRF protection for cookie-authenticated state changes (use suitable CSRF tokens or strict origin rules); robust logout.
- API authorization middleware protects every sensitive endpoint. Admin may manage users and global settings. Member may manage projects/deployments (document v0.1 workspace-sharing policy). No role-based UI-only security.
- Admin can invite/create users securely, deactivate/reactivate accounts and assign supported roles. Don't email credentials or display recoverable passwords.
- Prevent removal/deactivation of the last active admin. Define safe password reset via admin-triggered one-time reset tokens or an explicit CLI recovery path.
- Include audit events for user changes, auth failures of interest and sensitive admin operations.

## 7. Git integration and redeploying

- Support cloning public Git via HTTPS and private Git through controlled SSH deploy keys. Protect private key material; use `known_hosts` verification (no blanket `StrictHostKeyChecking=no`).
- Validate repository scheme and host/address carefully to reduce SSRF and access to cloud metadata/internal endpoints. Use safe non-shell command invocation with arguments, timeouts and concurrency limits.
- Allow branch selection. Resolve to an immutable commit SHA before building and show it in UI. Capture commit subject, author (if available), and build trigger.
- Register GitHub-compatible webhooks or guide the operator through provider setup; verify HMAC signatures, exact event types and selected branch before enqueuing. Reject unauthenticated or replayed/malformed payloads as appropriate. Use webhook delivery IDs for idempotency.
- Push events trigger builds automatically when enabled. Support manual rebuild and manual deploy of known commits. Handle force-pushes, concurrent pushes and stale builds: serialize activations per project, and ensure older build completion cannot accidentally replace newer desired commits.
- Build context may be a monorepo subfolder; do not require complex graph-based affected-project detection in v0.1.

## 8. Builders and `mise`

Expose `builder: auto | railpack | dockerfile` initially, and a custom build command/override where supported and safe. The UI must clearly show the detected builder and reason for detection.

- Dockerfile: respect configurable context, Dockerfile path, build args that aren't secrets, target, and build limits. Avoid running arbitrary command strings through a host shell.
- Railpack: invoke a supported, pinned Railpack execution pathway in a disposable isolated builder, using its current documented CLI/integration. Verify actual command syntax; do not fabricate flags. Railpack's role is build detection and image creation when no Dockerfile is given.
- `mise`: preserve/use version information where Railpack supports it; if a project requires tasks/tool versions not expressible via Railpack, provide a documented advanced build strategy or an explicit error. In this first release, **do not assert universal mise support solely because `mise.toml` exists**. Optionally support a separate `mise` builder only if you can reproducibly package its artifacts and test it thoroughly.
- Build source from pinned SHA. Enforce timeouts, CPU/memory/disk controls where available, and bounded concurrency. Treat repository code as untrusted relative to the control plane; a Docker socket can grant powerful host access, so builder containers must never get privileged access to the daemon socket or the control plane's secret store.
- Record build logs incrementally; stream to UI (SSE preferred for one-way feeds). Redact sensitive values and cap log retention/size. Store release artifacts and image identifiers for rollback.
- Show failure reasons at the correct stage: clone, dependency installation, build, artifact extraction, container startup, health, routing.

### Static build support — first-class and resource efficient

- User chooses or auto-detects `static`. Static output directory is explicit or detected with user confirmation (`dist`, `build`, `out`, etc.). Ensure the directory stays under the allowed build output path; prevent symlink traversal and path escaping.
- Run build in a disposable container/image builder. Extract the built static assets to a staging folder, validate file types/permissions/path boundaries, and atomically promote immutable release directories.
- Store under e.g. `/var/lib/depot/static/<project-id>/releases/<release-id>/` with a stable active pointer or atomic mapping in the gateway. Keep a configured number of releases for rollback.
- **No one-container-per-static-site at runtime.** The `depot serve` process hosts a shared static file server integrated with the gateway; give static files appropriate MIME types, caching, and optional SPA fallback. Do not expose arbitrary filesystem paths.
- Site changes become active through a release pointer switch, not through process restarts. A failed build preserves the old release.

## 9. OCI runtime and safe promotion

- Run service images with explicit port, runtime environment, CPU/memory limits, restart policy, log configuration, and non-privileged defaults. No host networking, unrestricted host mounts, or privileged containers by default.
- Bind app container traffic to a private bridge/network reachable only by Depot gateway; do not publish each app publicly on random host ports if avoidable.
- Build and start a candidate release; wait for a configurable health/readiness check (HTTP or TCP, with a sensible fallback policy). Only after verification switch the gateway's active upstream and persist the active release.
- Gracefully drain and stop the old container where practical, retain rollback metadata/artifacts, and garbage-collect old releases only according to explicit retention rules.
- Support start/stop/restart, manual redeploy and rollback; clearly distinguish administrative stop from crash/failure.
- Worker restart must reconcile Docker containers with database state and safely recover or fail jobs left mid-flight. Serialize conflicting operations per project. Never mark deployment successful before the gateway points at a ready release and the required route has been acknowledged.

## 10. Domains, gateway and Caddy host agent

**Mandatory topology:** public request → **Caddy on Proxmox host** (TLS) → **single private Depot gateway listener** in VM → internal service container or static release. The main host Caddy continues to own all ACME issuance/renewal and existing manually maintained sites.

### Depot gateway

- Listen on a configurable private address/port, preferably reachable only from the host Caddy over a trusted private network.
- Dispatch by canonical validated hostname, with exact host lookup; do not trust hostile Host/X-Forwarded-* values or allow open proxying.
- Route static projects to shared static assets and services to currently active internal upstream. Stream HTTP/websocket requests as supported; preserve appropriate forward headers from **trusted** Caddy only.
- Avoid bouncing the gateway for every release or domain switch.

### Restricted host-side Caddy agent

The **same Depot executable** exposes `depot caddy-agent` and is installed separately on the Proxmox host. It runs as a minimally privileged service and is the only Depot component allowed to edit managed Caddy configuration. It must **not** expose Caddy's unrestricted admin API to the VM.

- Agent receives authenticated domain requests over a protected channel (prefer mTLS or a tightly scoped signed/token protocol over host-only VPN/private network; describe threat model, bootstrap, rotation).
- Allowlist permitted domain suffixes and precisely one configured upstream (the Depot gateway); do **not** accept arbitrary upstream addresses from untrusted app/user input.
- Manage only `/etc/caddy/depot/*.caddy` or another dedicated generated directory, alongside a one-time manually configured `import depot/*.caddy` in the main Caddyfile.
- For each desired hostname, emit a minimal `reverse_proxy <depot-gateway-private-address>` stanza with safe generated names. Canonicalize IDNs/hostnames; reject malformed entries, duplicates/conflicts and injection attempts.
- Write temp file, validate full Caddy configuration (`caddy validate`), perform reload, acknowledge success/failure, and revert generated file atomically if failed. Avoid clobbering manually owned Caddy routes.
- Document DNS A/AAAA records and ACME validation prerequisites. A route being accepted does not guarantee public DNS resolves or certificate issuance has completed: expose accurate states and check externally where possible.
- On domain removal, explicitly remove only the corresponding Depot-owned generated entry and reload.
- Implement reconciliation: after downtime, Depot can re-send desired domains without duplicate/conflicting entries.

The agent's OS privileges must be narrow: writable generated config directory and controlled ability to validate/reload Caddy, not full Proxmox administrator access. Document use of group permissions or a minimal privileged helper for reload if required.

## 11. Secrets and environment variables

- Define per-project variables; mark as plain config or secret. Provide create/update/delete, concealed values and controlled one-time reveal only if policy permits. Default UI/API never returns ciphertext or plaintext values after creation.
- Encrypt secret values at rest using an authenticated encryption scheme with fresh nonces; keep the master key outside SQLite and backups of SQLite. Support key rotation design and initial backup/recovery guidance. Do not invent your own crypto.
- Only decrypt secrets in the authorised worker close to launching the intended app/build; avoid passing secret values on command lines, logs or Docker image build args. Protect worker-to-runtime secret injection and manage exposure in container introspection. Document the limits of Docker secrets under the chosen runtime.
- Prevent secrets from leaking into log ingestion, debug responses, frontend query caches or exceptions. Mask matching literals conservatively but recognise masking is defence-in-depth, not a proof of safety.
- Separate build-time secret scope from runtime secrets. Builds without explicit need should receive none.
- `DATABASE_URL`/S3 fields can be ordinary secret entries initially; automatic resource provisioning is deferred.

## 12. API and frontend implementation

Use consistent JSON envelopes/errors and REST endpoints with sensible resource IDs; OpenAPI spec should match runtime behaviour. Suggested groups:

`/api/v1/auth/*`, `/api/v1/users/*`, `/api/v1/projects/*`, `/api/v1/projects/{id}/deployments/*`, `/api/v1/projects/{id}/domains/*`, `/api/v1/projects/{id}/variables/*`, `/api/v1/deployments/{id}/events`, `/api/v1/settings/*`, `/api/v1/system/health`, `/api/v1/webhooks/github/{token-or-project}`.

The UI must use TanStack Query for server state, Form for form lifecycle, Router for routing, Table for tabular lists, Virtual for long log output. Avoid raw `fetch` repeated in components. Put request handling in a typed API module, and TanStack Query hooks in feature folders. Use appropriately scoped loading, empty and error states. Prefer SSE for deployment events and log tail; reconnect with last event cursor. Document errors in UI instead of swallowing them.

Create reusable accessible primitives: Button, TextInput, Select, Checkbox, Modal/Dialog, Alert, Tabs, Table, StatusIndicator, Code/LogViewer, AppShell, Sidebar, SidebarItem, Field, PageHeading, StatPanel. They must all use square corners and consistent tokens.

New project wizard: repository → ref/build detection → static/service configuration → variables/domains → deploy. Allow backing out/editing. Prevent invalid combinations such as selecting a static deploy without a plausible output directory.

User management: Admin-only user list, create user, role change, disable user, reset flow. Ensure members never see admin-only actions or gain access via direct API calls.

## 13. Logging, observability and operations

- Structured Rust logs with request and deployment correlation IDs; redact credentials.
- Health/readiness endpoints distinguish API liveness from worker, Docker and gateway readiness. Dashboard should show actual status, not a fake “everything operational” indicator.
- Live build logs and runtime logs with timestamps, stage markers, status colours and sensible truncation/retention. Support downloading logs if feasible without exposing secrets.
- Backups: document SQLite backup procedure, `/var/lib/depot` artifact persistence, master key backup (separately secured), and restore order.
- Configuration: TOML file and/or environment-based config with an example; no real secrets committed. File permissions and startup checks for key material. Distinct config for host agent.
- Installation docs for Ubuntu/Debian-like VM, Docker privileges, firewall rules, systemd units, first-run initialization, deploy keys, GitHub webhook setup, agent installation, Caddy import, DNS prerequisites, and update/rollback.
- Provide `depot doctor` for environment checks (Docker socket access, data directories, ports, agent connectivity, config correctness and install prerequisites).

## 14. Security requirements and trust boundaries

Make these review blockers:

1. **Build isolation:** untrusted Git repository code never runs as the privileged control-plane user; never mounts the Docker socket into an untrusted build container. Docker daemon access is effectively host-level privilege; use a restricted worker and document the residual risk.
2. **Secrets:** don't expose env secrets to builds by default; never log or return hidden values.
3. **Network:** protect internal API and host agent; ensure public domains cannot route to arbitrary private targets; validate Git clone destinations, URL schemes and webhook inputs.
4. **Filesystem:** safe slug/path handling, symlink traversal defence, atomic static release promotion, no arbitrary file read/write through the gateway.
5. **Authentication:** hashed passwords, expiration/revocation, secure cookies, CSRF protection, enforce roles server-side.
6. **Deployment concurrency:** idempotent jobs, deduplication, no stale-release promotion, consistent recovery after restart.
7. **Caddy:** only managed config area can be changed and existing routes are never overwritten by a Depot project.
8. **Denial of service:** bounded queues, job concurrency and logs; reasonable request-size/time limits; limits for build and app resources.

Threat-model these boundaries in `docs/architecture/security.md`, acknowledging which risks are deliberately deferred to operator controls in v0.1.

## 15. Testing and quality gates

Tests must be meaningful and executable:

- Rust unit tests for auth/role decisions, domain validation, route generation, deployment state transitions, build selection, release promotion rules, secret encryption and config parsing.
- SQLite integration tests for migrations, transactions, job lease/recovery, deduplication, user creation and permissions.
- API integration tests for login/logout, CSRF, admin-only APIs, project CRUD, malformed inputs, credentials masking and webhook signatures.
- Builder/runtime tests with test doubles; gated live Docker integration tests for an example Rust/Node HTTP service and static Vite/HTML site.
- Caddy agent config generation, validation, rollback and duplicate-domain tests; live gated Caddy test if available.
- Frontend component tests and React Testing Library; key user-flow end-to-end tests using Playwright where feasible. Verify responsive dashboard, auth, project creation, deployment logs and admin user management.
- Security regression tests for traversal, injection, forged webhooks, forbidden API access, stale deployment ordering and unauthorised route changes.
- Accessibility checks: keyboard navigation, labels, contrasts, focus states and dialogs.

Add CI commands, for example:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cd web && pnpm install --frozen-lockfile
cd web && pnpm lint
cd web && pnpm format:check
cd web && pnpm typecheck
cd web && pnpm test
cd web && pnpm build
cargo build --release -p depot
```

Align these example commands with actual manifest scripts. Provide a root `justfile` or Makefile so development setup, format, lint, tests, release packaging and smoke tests are one-command tasks. **Do not declare success if checks fail.**

## 16. Implementation milestones — prioritise working software

### Milestone A — Foundation and visual shell
- Workspace, formatters, linters, type checks, CI.
- Rust CLI/server, migrations, generated API spec/types, embedded Vite assets.
- Fully functional first-admin setup and local authentication, roles, user admin.
- Properly structured React dashboard and shared square-edged components matching reference.
- **Demo:** Release binary starts, serves UI, an admin logs in and creates a member.

### Milestone B — First deployment (critical vertical slice)
- Project create/import with public repo, branch and Dockerfile/Railpack detection.
- Worker and durable jobs; isolated Docker build; OCI container run; internal gateway; logs, health and status.
- **Demo:** Member deploys a simple Git-hosted HTTP service and sees it through the private Depot gateway.

### Milestone C — Static sites and host HTTPS
- Isolated static builds and immutable file releases.
- Single shared static server; service and static gateway routing.
- Caddy host agent, generated imports, validation/reload/reconciliation; DNS setup documentation.
- **Demo:** Static site deployed without persistent app container; both static and API projects reachable over public host Caddy TLS.

### Milestone D — Automatic deploy and reliability
- Private SSH repository credentials, authenticated GitHub webhooks, auto-deploy.
- Secrets/variables, redeploy history, rollback, job recovery, runtime logs, safe cleanup.
- **Demo:** Push to configured branch triggers a healthy new release; failure preserves the previous release; admin can roll back.

### Milestone E — Release hardening
- End-to-end tests, threat model, install/upgrade/backup docs, observability, robust empty/error states and accessibility.
- Verify single binary: embedded UI, clean setup, systemd process modes, no production Node runtime.
- **Demo:** Repeat the acceptance scenario on a clean VM or containerised integration test environment, with honest notes for unavailable external infrastructure.

## 17. Acceptance checklist

Do not consider Depot v0.1 complete until the following are true:

- [ ] One built Rust executable supplies UI/API, worker and Caddy agent subcommands; frontend assets embedded.
- [ ] Install runs without Node/Vite or copying a separate frontend dist directory to production.
- [ ] First-run admin registration is secure; admin can manage users; members cannot call admin APIs.
- [ ] Public Git repository can be deployed from commit SHA via Dockerfile and via Railpack where appropriate.
- [ ] Static site deploys and is served without its own persistent container.
- [ ] Private Git can authenticate securely through SSH.
- [ ] Project can set domain, Caddy agent validates and registers it, and host Caddy handles HTTPS subject to real DNS/ACME prerequisites.
- [ ] GitHub push webhook is validated and triggers deployment on configured branch.
- [ ] Secrets are encrypted at rest and not revealed in ordinary API/log responses.
- [ ] Users can view deployment statuses, build/runtime logs, history and perform rollback.
- [ ] Failed and stale builds cannot replace the last healthy active release.
- [ ] API/worker restart can recover or reconcile deployment state.
- [ ] UI resembles the approved near-black/lime mockup and **all border radii are 0**.
- [ ] React code is split into clear feature modules and properly typed TanStack integrations.
- [ ] Rust and TS functions/components have concise top-level docs; structure and naming favour human readability.
- [ ] Format, lint, tests, typecheck and release binary smoke test are green.
- [ ] README gives an operator a realistic install path and explicitly lists remaining limitations.

## 18. Deliverables and final report

Produce the actual implementation in the repository, including:

1. Source code and lockfiles; versioned schema migrations.
2. Single-binary release build and reproducible build instructions.
3. Systemd templates for server/worker and host Caddy agent; example configurations.
4. Documentation: README, architecture, security, development, installation, DNS/Caddy, operations/backup/recovery.
5. Test suite and CI jobs.
6. A concise final implementation report: completed features per milestone, paths to notable files, command list, tests run and results, security tradeoffs, known limitations and exact deployment instructions.

**Quality reminder:** This is a personal, maintainable developer tool. The code should be pleasant to read, straightforward to debug, and safe by default. Do not favour maximal cleverness over clarity. The first release should be useful today while leaving room for database/S3 provisioning, preview environments, and multi-node Depot instances later.
