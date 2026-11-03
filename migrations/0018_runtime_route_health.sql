ALTER TABLE deployments ADD COLUMN runtime_route_status TEXT NOT NULL DEFAULT 'pending'
    CHECK (runtime_route_status IN ('pending', 'verified', 'error'));
ALTER TABLE deployments ADD COLUMN runtime_route_error TEXT;
ALTER TABLE deployments ADD COLUMN runtime_route_checked_at INTEGER;

CREATE INDEX deployments_runtime_route_health_idx
    ON deployments(active, runtime_route_status)
    WHERE active = 1 AND container_id IS NOT NULL;
