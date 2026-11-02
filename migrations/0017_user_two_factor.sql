CREATE TABLE user_two_factor (
    user_id TEXT PRIMARY KEY NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    secret_ciphertext BLOB NOT NULL,
    secret_nonce BLOB NOT NULL CHECK (length(secret_nonce) = 24),
    enabled_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE user_two_factor_recovery_codes (
    user_id TEXT NOT NULL REFERENCES user_two_factor(user_id) ON DELETE CASCADE,
    code_hash BLOB NOT NULL CHECK (length(code_hash) = 32),
    created_at INTEGER NOT NULL,
    used_at INTEGER,
    PRIMARY KEY (user_id, code_hash)
);

CREATE INDEX user_two_factor_recovery_available_idx
    ON user_two_factor_recovery_codes(user_id)
    WHERE used_at IS NULL;
