Load architecture.md before starting any work.
Always load ~/dev/tuicore/SKILL.md on startup.

Persistence: tasks are stored in SQL through SQLx. Default DB is SQLite unless TUIDO_DATABASE_URL points elsewhere; Postgres is supported by config. Run migrations from migrations/ before app use. Task detail edits for size, state, and entities save immediately.
