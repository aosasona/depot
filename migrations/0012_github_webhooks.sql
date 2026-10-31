CREATE TABLE github_webhooks (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL UNIQUE REFERENCES projects(id) ON DELETE CASCADE,
    endpoint_token_hash BLOB NOT NULL UNIQUE CHECK (length(endpoint_token_hash) = 32),
    secret_ciphertext BLOB NOT NULL,
    secret_nonce BLOB NOT NULL CHECK (length(secret_nonce) = 24),
    repository_full_name TEXT NOT NULL,
    created_by_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE github_webhook_deliveries (
    webhook_id TEXT NOT NULL REFERENCES github_webhooks(id) ON DELETE CASCADE,
    delivery_id TEXT NOT NULL,
    commit_sha TEXT NOT NULL,
    deployment_id TEXT REFERENCES deployments(id) ON DELETE SET NULL,
    received_at INTEGER NOT NULL,
    PRIMARY KEY(webhook_id, delivery_id)
);
