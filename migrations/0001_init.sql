CREATE TABLE IF NOT EXISTS tasks (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  task_type TEXT NOT NULL CHECK (task_type IN ('action', 'note')),
  subtype TEXT NOT NULL CHECK (subtype IN ('task', 'waiting', 'follow_up', 'artifact_update')),
  state TEXT NOT NULL CHECK (state IN ('clarify', 'next', 'doing', 'waiting', 'snoozed', 'done')),
  task_kind TEXT NOT NULL DEFAULT 'action' CHECK (task_kind IN ('action', 'waiting', 'follow_up')),
  workflow_state TEXT NOT NULL DEFAULT 'todo' CHECK (workflow_state IN ('todo', 'in_progress', 'done', 'snoozed')),
  size TEXT NOT NULL CHECK (size IN ('small', 'medium', 'big')),
  start_date TEXT,
  due_date TEXT,
  focus_today BOOLEAN NOT NULL DEFAULT FALSE,
  frog_candidate BOOLEAN NOT NULL DEFAULT FALSE,
  detail TEXT NOT NULL DEFAULT '',
  ai_rationale TEXT NOT NULL DEFAULT '',
  swap_note TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS people (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  email TEXT NOT NULL DEFAULT '',
  active BOOLEAN NOT NULL DEFAULT TRUE,
  sort_order INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY,
  key TEXT NOT NULL,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  lead_person_id TEXT REFERENCES people(id) ON DELETE SET NULL,
  sort_order INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS task_people (
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  person_id TEXT NOT NULL REFERENCES people(id) ON DELETE RESTRICT,
  sort_order INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (task_id, person_id)
);

CREATE INDEX IF NOT EXISTS idx_task_people_person_id
  ON task_people(person_id);

CREATE TABLE IF NOT EXISTS task_projects (
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE RESTRICT,
  sort_order INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (task_id, project_id)
);

CREATE INDEX IF NOT EXISTS idx_task_projects_project_id
  ON task_projects(project_id);

CREATE TABLE IF NOT EXISTS entities (
  id TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  entity_type TEXT NOT NULL DEFAULT 'other',
  sort_order INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS task_entities (
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE RESTRICT,
  sort_order INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (task_id, entity_id)
);

CREATE INDEX IF NOT EXISTS idx_task_entities_entity_id
  ON task_entities(entity_id);
