# GitHub App repository onboarding

Depot uses one operator-owned GitHub App to browse repositories, check out
private sources with short-lived credentials, and receive automatic push
deployments. Depot never stores an installation token.

## Recommended: connect from the dashboard

Sign in as a Depot administrator, open **Projects**, choose **Set up GitHub**,
then select **Create GitHub App**. GitHub opens a prefilled, private App form;
review it and create the App. When GitHub returns to Depot, the generated
private key and webhook secret are encrypted with Depot's master key and stored
locally. Select **Install GitHub App**, choose the repositories Depot may use,
and return to the project wizard to browse them.

This flow requires `security.public_origin` to be the externally reachable
HTTPS origin for Depot. The API and worker pick up dashboard-managed App
credentials without a service restart.

## Manual host configuration

The file-based configuration remains available for operators who manage secrets
outside Depot. Dashboard-managed credentials take precedence when both exist.

### 1. Create the GitHub App

In GitHub, create a GitHub App owned by the account or organization that will
install it. Set:

- **Homepage URL:** your Depot public origin, such as `https://depot.example.com`
- **Setup URL:** the same Depot public origin
- **Redirect on update:** enabled
- **Webhook URL:** `https://depot.example.com/api/v1/webhooks/github-app`
- **Webhook secret:** a generated random value of at least 32 characters
- **Repository permissions / Contents:** Read-only
- **Repository permissions / Metadata:** Read-only
- **Subscribe to events:** Push

Generate a private key for the App. Record the numeric App ID and the App slug
from its public URL. Install the App only on repositories Depot should deploy.

### 2. Install the credentials on the Depot VM

Keep both values outside SQLite and readable only by the Depot service account:

```sh
sudo install -o root -g depot -m 0640 github-app.private-key.pem /etc/depot/github-app.pem
sudo sh -c 'umask 027; printf "%s\n" "YOUR_WEBHOOK_SECRET" > /etc/depot/github-webhook-secret'
sudo chown root:depot /etc/depot/github-webhook-secret
```

Add this top-level table to `/etc/depot/config.toml`:

```toml
[github]
app_id = 123456
app_slug = "your-depot-app"
private_key_file = "/etc/depot/github-app.pem"
webhook_secret_file = "/etc/depot/github-webhook-secret"
```

Validate and restart both Depot processes:

```sh
sudo depot validate-config --config /etc/depot/config.toml
sudo systemctl restart depot-api depot-worker
sudo systemctl --no-pager --full status depot-api depot-worker
```

## Import a repository

Sign into Depot, open Projects, and select **Install GitHub App**. GitHub will ask
which account and repositories the App may access, then return to Depot. Search
the resulting repository list and select **Create project**.

Depot verifies the selected repository with the installation-scoped GitHub API
before saving it. Inspection and deployment mint a new short-lived installation
token, pass it through an ephemeral askpass helper, and remove it after Git
exits. Pushes to the configured project branch are verified with the App webhook
secret and deduplicated by GitHub delivery ID.

If a repository is missing, update the App installation's repository access in
GitHub and use **Install GitHub App** again. Archived repositories are visible but
cannot be imported.
