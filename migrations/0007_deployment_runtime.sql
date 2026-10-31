ALTER TABLE deployments ADD COLUMN image_id TEXT;
ALTER TABLE deployments ADD COLUMN container_id TEXT;
ALTER TABLE deployments ADD COLUMN host_port INTEGER
    CHECK (host_port IS NULL OR host_port BETWEEN 1 AND 65535);
