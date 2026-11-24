ALTER TABLE domains ADD COLUMN is_primary INTEGER NOT NULL DEFAULT 0
    CHECK (is_primary IN (0, 1));

UPDATE domains
SET is_primary = 1
WHERE id IN (
    SELECT candidate.id
    FROM domains AS candidate
    WHERE candidate.id = (
        SELECT first_domain.id
        FROM domains AS first_domain
        WHERE first_domain.project_id = candidate.project_id
        ORDER BY first_domain.created_at, first_domain.hostname
        LIMIT 1
    )
);

CREATE UNIQUE INDEX domains_one_primary_per_project_idx
    ON domains(project_id) WHERE is_primary = 1;
