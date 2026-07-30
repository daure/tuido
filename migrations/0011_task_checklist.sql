CREATE TABLE task_checklist_items (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  parent_id TEXT REFERENCES task_checklist_items(id) ON DELETE CASCADE,
  position BIGINT NOT NULL,
  text TEXT NOT NULL,
  checked BOOLEAN NOT NULL DEFAULT FALSE,
  UNIQUE (task_id, position)
);

CREATE INDEX idx_task_checklist_items_task_order
  ON task_checklist_items(task_id, position);
