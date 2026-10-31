CREATE TABLE service_configs (
    project_id TEXT PRIMARY KEY NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    project_kind TEXT NOT NULL DEFAULT 'auto'
        CHECK (project_kind IN ('auto', 'service', 'static')),
    builder TEXT NOT NULL DEFAULT 'auto'
        CHECK (builder IN ('auto', 'dockerfile', 'railpack')),
    context_path TEXT NOT NULL DEFAULT '.',
    dockerfile_path TEXT NOT NULL DEFAULT 'Dockerfile',
    static_output_dir TEXT,
    exposed_port INTEGER CHECK (exposed_port IS NULL OR exposed_port BETWEEN 1 AND 65535),
    healthcheck_path TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
