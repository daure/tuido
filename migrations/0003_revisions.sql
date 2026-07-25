ALTER TABLE tasks ADD COLUMN revision BIGINT NOT NULL DEFAULT 1;
ALTER TABLE people ADD COLUMN revision BIGINT NOT NULL DEFAULT 1;
ALTER TABLE projects ADD COLUMN revision BIGINT NOT NULL DEFAULT 1;
ALTER TABLE tags ADD COLUMN revision BIGINT NOT NULL DEFAULT 1;

CREATE TABLE workspace_revision (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  revision BIGINT NOT NULL
);

INSERT INTO workspace_revision (singleton, revision) VALUES (1, 1);
