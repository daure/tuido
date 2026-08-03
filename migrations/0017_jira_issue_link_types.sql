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

WITH converted AS (
  SELECT
    CASE
      WHEN relation_type IN ('depends_on', 'is_duplicated_by') THEN target_task_id
      ELSE source_task_id
    END AS source_task_id,
    CASE
      WHEN relation_type IN ('depends_on', 'is_duplicated_by') THEN source_task_id
      ELSE target_task_id
    END AS target_task_id,
    CASE
      WHEN relation_type IN ('depends_on', 'has_to_be_done_before') THEN 'blocks'
      ELSE 'duplicates'
    END AS relation_type,
    position
  FROM task_relations_legacy
)
INSERT INTO task_relations (
  source_task_id,
  target_task_id,
  relation_type,
  position
)
SELECT source_task_id, target_task_id, relation_type, MIN(position)
FROM converted
GROUP BY source_task_id, target_task_id, relation_type;

DROP TABLE task_relations_legacy;

CREATE INDEX idx_task_relations_target ON task_relations(target_task_id);
