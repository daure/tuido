ALTER TABLE projects RENAME TO workspaces;
ALTER TABLE task_projects RENAME TO task_workspaces;
ALTER TABLE task_workspaces RENAME COLUMN project_id TO workspace_id;

DROP INDEX IF EXISTS idx_projects_key;
DROP INDEX IF EXISTS idx_task_projects_project_id;
DROP INDEX IF EXISTS idx_task_projects_task_id;

CREATE UNIQUE INDEX idx_workspaces_key ON workspaces(key);
CREATE INDEX idx_task_workspaces_workspace_id ON task_workspaces(workspace_id);
CREATE UNIQUE INDEX idx_task_workspaces_task_id ON task_workspaces(task_id);

UPDATE app_settings
SET key = 'tasks.default_workspace'
WHERE key = 'tasks.default_project';
