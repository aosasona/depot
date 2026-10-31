CREATE TABLE project_ssh_credentials (
    project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    private_key_ciphertext BLOB NOT NULL,
    private_key_nonce BLOB NOT NULL CHECK (length(private_key_nonce) = 24),
    known_hosts TEXT NOT NULL,
    created_by_user_id TEXT NOT NULL REFERENCES users(id),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
