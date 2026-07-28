ALTER TABLE tasks ADD COLUMN rank BIGINT NOT NULL DEFAULT 0;

UPDATE tasks
SET rank = (
  SELECT COUNT(*)
  FROM tasks AS earlier
  WHERE earlier.created_at < tasks.created_at
     OR (earlier.created_at = tasks.created_at AND earlier.id <= tasks.id)
);

CREATE INDEX idx_tasks_rank ON tasks(rank, id);
