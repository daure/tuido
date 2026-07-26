CREATE INDEX IF NOT EXISTS idx_tasks_due_snooze ON tasks (snoozed_until) WHERE workflow_state = 'snoozed' AND rejected = false AND snoozed_until IS NOT NULL;
