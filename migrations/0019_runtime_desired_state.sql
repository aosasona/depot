ALTER TABLE deployments ADD COLUMN runtime_desired_state TEXT NOT NULL DEFAULT 'running'
    CHECK (runtime_desired_state IN ('running', 'stopped'));
