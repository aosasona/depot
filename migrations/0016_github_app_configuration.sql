CREATE TABLE github_app_configuration (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    app_id INTEGER NOT NULL CHECK (app_id > 0),
    app_slug TEXT NOT NULL,
    private_key_ciphertext BLOB NOT NULL,
    private_key_nonce BLOB NOT NULL,
    webhook_secret_ciphertext BLOB NOT NULL,
    webhook_secret_nonce BLOB NOT NULL,
    configured_by_user_id TEXT NOT NULL REFERENCES users(id),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
