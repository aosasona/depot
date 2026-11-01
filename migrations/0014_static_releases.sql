ALTER TABLE deployments ADD COLUMN static_release_path TEXT;

CREATE INDEX deployments_static_release_idx
    ON deployments(project_id, static_release_path)
    WHERE static_release_path IS NOT NULL;
