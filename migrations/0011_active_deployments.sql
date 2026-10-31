ALTER TABLE deployments ADD COLUMN active INTEGER NOT NULL DEFAULT 0
    CHECK (active IN (0, 1));
ALTER TABLE deployments ADD COLUMN retired_at INTEGER;
ALTER TABLE deployments ADD COLUMN image_retired_at INTEGER;

UPDATE deployments
SET active = 1
WHERE id = (
    SELECT candidate.id
    FROM deployments AS candidate
    WHERE candidate.project_id = deployments.project_id
      AND candidate.status = 'succeeded'
      AND candidate.host_port IS NOT NULL
    ORDER BY candidate.finished_at DESC, candidate.id DESC
    LIMIT 1
);

CREATE UNIQUE INDEX deployments_one_active_per_project_idx
    ON deployments(project_id) WHERE active = 1;
