CREATE TABLE github_installations (
    installation_id INTEGER PRIMARY KEY,
    account_login TEXT NOT NULL,
    account_type TEXT NOT NULL,
    repository_selection TEXT NOT NULL,
    linked_by_user_id TEXT NOT NULL REFERENCES users(id),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE github_project_repositories (
    project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    installation_id INTEGER NOT NULL REFERENCES github_installations(installation_id),
    repository_id INTEGER NOT NULL,
    repository_full_name TEXT NOT NULL,
    private INTEGER NOT NULL CHECK (private IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (installation_id, repository_id)
);

CREATE TABLE github_app_deliveries (
    installation_id INTEGER NOT NULL,
    repository_id INTEGER NOT NULL,
    delivery_id TEXT NOT NULL,
    commit_sha TEXT NOT NULL,
    deployment_id TEXT REFERENCES deployments(id),
    received_at INTEGER NOT NULL,
    PRIMARY KEY (installation_id, delivery_id)
);

CREATE INDEX github_project_repository_lookup
    ON github_project_repositories(installation_id, repository_id);
