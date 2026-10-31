CREATE TABLE domains (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    hostname TEXT NOT NULL COLLATE NOCASE UNIQUE,
    routing_status TEXT NOT NULL DEFAULT 'pending'
        CHECK (routing_status IN ('pending', 'active', 'error')),
    last_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX domains_project_idx ON domains(project_id, created_at);
