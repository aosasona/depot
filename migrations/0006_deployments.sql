CREATE TABLE deployments (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    job_id TEXT NOT NULL UNIQUE REFERENCES jobs(id) ON DELETE RESTRICT,
    commit_sha TEXT NOT NULL,
    trigger_kind TEXT NOT NULL CHECK (trigger_kind IN ('manual', 'webhook')),
    status TEXT NOT NULL CHECK (status IN ('queued', 'building', 'succeeded', 'failed')),
    created_by_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
    failure_reason TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    finished_at INTEGER
);

CREATE INDEX deployments_project_created_idx
    ON deployments(project_id, created_at DESC);
