ALTER TABLE task_relations RENAME TO task_relations_legacy;

DROP INDEX idx_task_relations_target;

CREATE TABLE task_relations (
  source_task_id BIGINT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  target_task_id BIGINT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  relation_type TEXT NOT NULL CHECK (relation_type IN ('blocks', 'relates_to', 'duplicates')),
  position BIGINT NOT NULL DEFAULT 0,
  PRIMARY KEY (source_task_id, target_task_id, relation_type),
  CHECK (source_task_id <> target_task_id)
);

WITH ranked AS (
  SELECT
    source_task_id,
    target_task_id,
    relation_type,
    position,
    ROW_NUMBER() OVER (
      PARTITION BY
        CASE
          WHEN source_task_id < target_task_id THEN source_task_id
          ELSE target_task_id
        END,
        CASE
          WHEN source_task_id < target_task_id THEN target_task_id
          ELSE source_task_id
        END
      ORDER BY position, relation_type, source_task_id, target_task_id
    ) AS pair_rank
  FROM task_relations_legacy
)
INSERT INTO task_relations (
  source_task_id,
  target_task_id,
  relation_type,
  position
)
SELECT source_task_id, target_task_id, relation_type, position
FROM ranked
WHERE pair_rank = 1;

DROP TABLE task_relations_legacy;

CREATE INDEX idx_task_relations_target ON task_relations(target_task_id);

CREATE UNIQUE INDEX idx_task_relations_pair ON task_relations (
  (CASE
    WHEN source_task_id < target_task_id THEN source_task_id
    ELSE target_task_id
  END),
  (CASE
    WHEN source_task_id < target_task_id THEN target_task_id
    ELSE source_task_id
  END)
);
