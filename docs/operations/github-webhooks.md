# GitHub push webhooks

Depot can accept GitHub push events without giving GitHub a reusable deployment
token. Each project has a separate unguessable endpoint and signing secret.
Creating the webhook again rotates both values immediately.

## Configure GitHub

1. Open the project in Depot and create a GitHub push webhook using the exact
   `owner/repository` name.
2. Copy the payload URL and secret immediately. Depot returns them only once.
3. In the GitHub repository, open **Settings → Webhooks → Add webhook**.
4. Paste the payload URL, select `application/json`, paste the secret, and
   choose **Just the push event**.
5. Leave the webhook active and add it.

Depot verifies `X-Hub-Signature-256` over the unmodified request body before
parsing it. It accepts only pushes whose repository and branch exactly match
the configured project, then deploys the full commit SHA from the event.
GitHub delivery IDs are recorded transactionally, so a redelivery cannot
enqueue the same event twice. Payloads are limited to 1 MiB.

The endpoint deliberately returns a generic authentication error for invalid
signatures, repositories, branches, event types, and malformed payloads. Check
the configured project branch and GitHub's recent-deliveries view when a valid
push does not enqueue a deployment.
