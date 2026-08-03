CREATE TABLE task_relations (
  source_task_id BIGINT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  target_task_id BIGINT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  relation_type TEXT NOT NULL CHECK (relation_type IN ('depends_on', 'is_duplicated_by', 'has_to_be_done_before')),
  position BIGINT NOT NULL DEFAULT 0,
  PRIMARY KEY (source_task_id, target_task_id, relation_type),
  CHECK (source_task_id <> target_task_id)
);

CREATE INDEX idx_task_relations_target ON task_relations(target_task_id);
