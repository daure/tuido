CREATE TABLE task_id_map (
  old_id TEXT PRIMARY KEY,
  new_id BIGINT NOT NULL UNIQUE
);

INSERT INTO task_id_map (old_id, new_id)
SELECT id, ROW_NUMBER() OVER (ORDER BY rank, id)
FROM tasks;

CREATE TABLE tasks_with_numeric_ids (
  id BIGINT PRIMARY KEY,
  key_prefix TEXT NOT NULL DEFAULT '',
  title TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('clarify', 'next', 'doing', 'waiting', 'snoozed', 'done')),
  workflow_state TEXT NOT NULL DEFAULT 'todo' CHECK (workflow_state IN ('todo', 'in_progress', 'done', 'snoozed')),
  rejected BOOLEAN NOT NULL DEFAULT FALSE,
  size TEXT NOT NULL CHECK (size IN ('small', 'medium', 'big')),
  priority TEXT NOT NULL DEFAULT 'medium' CHECK (priority IN ('low', 'medium', 'high')),
  description TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  snoozed_until TEXT,
  revision BIGINT NOT NULL DEFAULT 1,
  rank BIGINT NOT NULL DEFAULT 0
);

INSERT INTO tasks_with_numeric_ids (
  id, key_prefix, title, state, workflow_state, rejected, size, priority, description,
  created_at, updated_at, snoozed_until, revision, rank
)
SELECT
  task_id_map.new_id,
  COALESCE((
    SELECT projects.key
    FROM task_projects
    JOIN projects ON projects.id = task_projects.project_id
    WHERE task_projects.task_id = tasks.id
  ), ''),
  tasks.title, tasks.state, tasks.workflow_state,
  tasks.rejected, tasks.size, tasks.priority, tasks.description,
  tasks.created_at, tasks.updated_at, tasks.snoozed_until, tasks.revision, tasks.rank
FROM tasks
JOIN task_id_map ON task_id_map.old_id = tasks.id;

CREATE TABLE task_people_with_numeric_ids (
  task_id BIGINT NOT NULL REFERENCES tasks_with_numeric_ids(id) ON DELETE CASCADE,
  person_id TEXT NOT NULL REFERENCES people(id) ON DELETE RESTRICT,
  sort_order BIGINT NOT NULL DEFAULT 0,
  PRIMARY KEY (task_id, person_id)
);

INSERT INTO task_people_with_numeric_ids (task_id, person_id, sort_order)
SELECT task_id_map.new_id, task_people.person_id, task_people.sort_order
FROM task_people
JOIN task_id_map ON task_id_map.old_id = task_people.task_id;

CREATE TABLE task_projects_with_numeric_ids (
  task_id BIGINT NOT NULL REFERENCES tasks_with_numeric_ids(id) ON DELETE CASCADE,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE RESTRICT,
  sort_order BIGINT NOT NULL DEFAULT 0,
  PRIMARY KEY (task_id, project_id),
  UNIQUE (task_id)
);

INSERT INTO task_projects_with_numeric_ids (task_id, project_id, sort_order)
SELECT task_id_map.new_id, task_projects.project_id, task_projects.sort_order
FROM task_projects
JOIN task_id_map ON task_id_map.old_id = task_projects.task_id;

CREATE TABLE task_tags_with_numeric_ids (
  task_id BIGINT NOT NULL REFERENCES tasks_with_numeric_ids(id) ON DELETE CASCADE,
  tag_id TEXT NOT NULL REFERENCES tags(id) ON DELETE RESTRICT,
  sort_order BIGINT NOT NULL DEFAULT 0,
  PRIMARY KEY (task_id, tag_id)
);

INSERT INTO task_tags_with_numeric_ids (task_id, tag_id, sort_order)
SELECT task_id_map.new_id, task_tags.tag_id, task_tags.sort_order
FROM task_tags
JOIN task_id_map ON task_id_map.old_id = task_tags.task_id;

CREATE TABLE task_links_with_numeric_ids (
  task_id BIGINT NOT NULL REFERENCES tasks_with_numeric_ids(id) ON DELETE CASCADE,
  url TEXT NOT NULL,
  PRIMARY KEY (task_id, url)
);

INSERT INTO task_links_with_numeric_ids (task_id, url)
SELECT task_id_map.new_id, task_links.url
FROM task_links
JOIN task_id_map ON task_id_map.old_id = task_links.task_id;

CREATE TABLE task_checklist_items_with_numeric_ids (
  id TEXT PRIMARY KEY,
  task_id BIGINT NOT NULL REFERENCES tasks_with_numeric_ids(id) ON DELETE CASCADE,
  parent_id TEXT REFERENCES task_checklist_items_with_numeric_ids(id) ON DELETE CASCADE,
  position BIGINT NOT NULL,
  text TEXT NOT NULL,
  checked BOOLEAN NOT NULL DEFAULT FALSE,
  UNIQUE (task_id, position)
);

INSERT INTO task_checklist_items_with_numeric_ids (
  id, task_id, parent_id, position, text, checked
)
SELECT
  task_checklist_items.id, task_id_map.new_id, task_checklist_items.parent_id,
  task_checklist_items.position, task_checklist_items.text, task_checklist_items.checked
FROM task_checklist_items
JOIN task_id_map ON task_id_map.old_id = task_checklist_items.task_id;

DROP TABLE task_checklist_items;
DROP TABLE task_links;
DROP TABLE task_tags;
DROP TABLE task_projects;
DROP TABLE task_people;
DROP TABLE tasks;

ALTER TABLE tasks_with_numeric_ids RENAME TO tasks;
ALTER TABLE task_people_with_numeric_ids RENAME TO task_people;
ALTER TABLE task_projects_with_numeric_ids RENAME TO task_projects;
ALTER TABLE task_tags_with_numeric_ids RENAME TO task_tags;
ALTER TABLE task_links_with_numeric_ids RENAME TO task_links;
ALTER TABLE task_checklist_items_with_numeric_ids RENAME TO task_checklist_items;

CREATE INDEX idx_tasks_rank ON tasks(rank, id);
CREATE INDEX idx_tasks_due_snooze ON tasks (snoozed_until)
WHERE workflow_state = 'snoozed' AND rejected = false AND snoozed_until IS NOT NULL;
CREATE INDEX idx_task_people_person_id ON task_people(person_id);
CREATE INDEX idx_task_projects_project_id ON task_projects(project_id);
CREATE INDEX idx_task_tags_tag_id ON task_tags(tag_id);
CREATE INDEX idx_task_checklist_items_task_order
ON task_checklist_items(task_id, position);
CREATE UNIQUE INDEX idx_projects_key ON projects(key);

CREATE TABLE task_id_sequence (
  singleton BIGINT PRIMARY KEY CHECK (singleton = 1),
  next_id BIGINT NOT NULL
);

INSERT INTO task_id_sequence (singleton, next_id)
SELECT 1, COALESCE(MAX(id), 0) + 1 FROM tasks;

DROP TABLE task_id_map;
