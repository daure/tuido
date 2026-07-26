CREATE TABLE task_links (
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  url TEXT NOT NULL,
  PRIMARY KEY (task_id, url)
);
