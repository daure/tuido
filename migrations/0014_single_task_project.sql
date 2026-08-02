DELETE FROM task_projects
WHERE EXISTS (
  SELECT 1
  FROM task_projects AS retained
  WHERE retained.task_id = task_projects.task_id
    AND (
      retained.sort_order < task_projects.sort_order
      OR (
        retained.sort_order = task_projects.sort_order
        AND retained.project_id < task_projects.project_id
      )
    )
);

CREATE UNIQUE INDEX idx_task_projects_task_id
  ON task_projects(task_id);
