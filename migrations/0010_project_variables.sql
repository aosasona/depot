CREATE TABLE project_variables (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    ciphertext BLOB NOT NULL,
    nonce BLOB NOT NULL CHECK (length(nonce) = 24),
    is_secret INTEGER NOT NULL CHECK (is_secret IN (0, 1)),
    created_by_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(project_id, name)
);

CREATE INDEX project_variables_project_name_idx
    ON project_variables(project_id, name);
