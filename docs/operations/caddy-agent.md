# Caddy agent quick start

Depot's Caddy agent runs on the Proxmox host as a mode of the same `depot`
binary. It manages only `/etc/caddy/depot/routes.caddy`, validates the full
Caddy configuration, reloads Caddy, and restores the previous generated file
when validation or reload fails. It never accepts an upstream address from a
project or reconciliation request.

## Trust model

- Bind the agent only to a host-only VPN or private management address.
- Use a random bearer token of at least 32 characters and restrict its file to
  the agent service account.
- Allowlist only domain suffixes that Depot may control.
- Configure exactly one upstream: the private Depot gateway in the VM.
- Firewall the agent listener so it is reachable only by the Depot VM.
- Do not expose Caddy's unrestricted admin API to Depot.

The bearer protocol is intended for a tightly scoped private network. Use mTLS
or a host VPN around it when the network is not already trusted.

## 1. Install the binary and managed directory

Copy the release binary built with `just release` to the Caddy host, then:

```sh
sudo install -m 0755 depot /usr/local/bin/depot
sudo install -d -m 0750 /etc/depot /etc/caddy/depot
sudo install -m 0640 deploy/caddy-agent.example.toml /etc/depot/caddy-agent.toml
```

Generate a token without putting it in shell history, place it in
`/etc/depot/caddy-agent.toml`, and set restrictive ownership and permissions:

```sh
openssl rand -hex 32
sudo chmod 0640 /etc/depot/caddy-agent.toml
```

Edit the configuration:

```toml
bind = "10.0.0.10:9080"
auth_token = "replace-with-at-least-32-random-characters"
gateway_upstream = "10.0.0.20:8080"
allowed_domain_suffixes = ["example.com"]
managed_directory = "/etc/caddy/depot"
caddyfile = "/etc/caddy/Caddyfile"
caddy_binary = "caddy"
command_timeout_seconds = 30
```

`gateway_upstream` is Depot's private VM gateway address. It is fixed by the
host operator and cannot be changed by an API request.

## 2. Import the managed directory once

Add this single operator-owned line to `/etc/caddy/Caddyfile`:

```caddyfile
import depot/*.caddy
```

Create an initial empty managed file if the import requires a match, then
validate the complete configuration:

```sh
sudo install -m 0640 /dev/null /etc/caddy/depot/routes.caddy
sudo caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
```

The agent account needs write access only to `/etc/caddy/depot` plus controlled
permission to run `caddy validate` and `caddy reload`. Do not grant general
write access to `/etc/caddy/Caddyfile` or unrelated host configuration. Exact
service ownership and a minimal privileged reload helper depend on the host's
Caddy package and service manager.

## 3. Start the authenticated agent

```sh
depot caddy-agent --config /etc/depot/caddy-agent.toml
```

The agent exposes only `POST /v1/reconcile`. Reconciliation requests contain
the complete desired hostname set; retries are safe and duplicate domains are
deduplicated.

Until automatic control-plane delivery is implemented, test or reconcile from
the trusted private network with:

```sh
curl --fail-with-body \
  -H 'Authorization: Bearer YOUR_RANDOM_TOKEN' \
  -H 'Content-Type: application/json' \
  --data '{"domains":["app.example.com","www.example.com"]}' \
  http://10.0.0.10:9080/v1/reconcile
```

A successful response acknowledges Caddy validation and reload. It does not
prove that public DNS resolves, ACME challenges succeed, or a certificate has
already been issued.

## 4. Recovery mode

For local host recovery without the network listener, create a JSON string
array and run one-shot reconciliation:

```sh
printf '["app.example.com"]\n' > /tmp/depot-domains.json
depot caddy-agent \
  --config /etc/depot/caddy-agent.toml \
  --domains-file /tmp/depot-domains.json
```

Remove the temporary snapshot afterward if its contents are operationally
sensitive. Domain values are validated and suffix-allowlisted in both modes.

## DNS and firewall checklist

- Point each public A/AAAA record at the host running Caddy.
- Permit public TCP 80 and 443 to Caddy for HTTP redirects and ACME.
- Permit the Caddy host to reach Depot's private gateway address.
- Permit only the Depot VM to reach the private agent listener.
- Do not expose container loopback ports outside the Depot VM.

