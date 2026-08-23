CREATE TABLE IF NOT EXISTS notes (
  id TEXT PRIMARY KEY,
  position BIGINT NOT NULL,
  content TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  revision BIGINT NOT NULL DEFAULT 1
);

-- Do not use IF NOT EXISTS here. A legacy `notes` table lacks `position`, and
-- migration must fail rather than be recorded as applied against that shape.
CREATE INDEX idx_notes_position ON notes(position, id);
