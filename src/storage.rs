use std::{env, fs, path::PathBuf};

use sqlx::{
    AnyPool, AssertSqlSafe, ConnectOptions, Row,
    any::{AnyConnectOptions, AnyPoolOptions},
    migrate::Migrator,
    sqlite::SqliteConnectOptions,
};

use crate::domain::{
    Person, Project, Tag, Task, TaskPriority, TaskSize, TaskState, WorkspaceSnapshot,
};
use crate::snooze::parse_datetime;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, PartialEq, Eq)]
enum MigrationSource {
    Disabled,
    Embedded,
    Directory(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlDialect {
    Sqlite,
    Postgres,
}

impl SqlDialect {
    fn from_database_url(database_url: &str) -> Result<Self, Box<dyn std::error::Error>> {
        if database_url.starts_with("sqlite:") {
            return Ok(Self::Sqlite);
        }
        if database_url.starts_with("postgres:") || database_url.starts_with("postgresql:") {
            return Ok(Self::Postgres);
        }
        Err(format!("unsupported database URL for tuido: {database_url}").into())
    }

    pub(crate) fn placeholder(self, index: usize) -> String {
        match self {
            Self::Sqlite => "?".to_string(),
            Self::Postgres => format!("${index}"),
        }
    }
}

pub struct Storage {
    pool: AnyPool,
    dialect: SqlDialect,
    notification_url: Option<String>,
}

impl Storage {
    pub async fn connect_from_env() -> Result<Self, Box<dyn std::error::Error>> {
        match env::var("TUIDO_DATABASE_URL") {
            Ok(database_url) if database_url.trim().is_empty() => {
                Err("TUIDO_DATABASE_URL must not be empty".into())
            }
            Ok(database_url) => Self::connect(&database_url).await,
            Err(env::VarError::NotPresent) => {
                let path = default_sqlite_path()?;
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                Self::connect_sqlite_path(path).await
            }
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) async fn connect(database_url: &str) -> Result<Self, Box<dyn std::error::Error>> {
        sqlx::any::install_default_drivers();
        let dialect = SqlDialect::from_database_url(database_url)?;
        let max_connections = if dialect == SqlDialect::Sqlite && database_url.contains(":memory:")
        {
            1
        } else {
            5
        };
        let pool_options = AnyPoolOptions::new().max_connections(max_connections);
        let pool_options = if dialect == SqlDialect::Sqlite {
            pool_options.after_connect(|connection, _| {
                Box::pin(async move {
                    sqlx::query("PRAGMA foreign_keys = ON")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("PRAGMA busy_timeout = 5000")
                        .execute(&mut *connection)
                        .await?;
                    Ok(())
                })
            })
        } else {
            pool_options
        };
        let pool = pool_options.connect(database_url).await?;
        if dialect == SqlDialect::Sqlite {
            configure_sqlite_journal(&pool).await?;
        }
        Ok(Self {
            pool,
            dialect,
            notification_url: (dialect == SqlDialect::Postgres).then(|| database_url.to_string()),
        })
    }

    async fn connect_sqlite_path(path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        sqlx::any::install_default_drivers();
        let sqlite_options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let any_options = AnyConnectOptions::from_url(&sqlite_options.to_url_lossy())?;
        let pool = sqlite_pool_options().connect_with(any_options).await?;
        configure_sqlite_journal(&pool).await?;
        Ok(Self {
            pool,
            dialect: SqlDialect::Sqlite,
            notification_url: None,
        })
    }

    pub fn pool(&self) -> AnyPool {
        self.pool.clone()
    }

    pub fn dialect(&self) -> SqlDialect {
        self.dialect
    }

    pub(crate) fn notification_url(&self) -> Option<String> {
        self.notification_url.clone()
    }

    pub async fn migrate(&self) -> Result<(), Box<dyn std::error::Error>> {
        match migration_source(env::var("TUIDO_AUTO_MIGRATE"), || {
            crate::paths::optional_path_env("TUIDO_MIGRATIONS_DIR")
        })? {
            MigrationSource::Disabled => return Ok(()),
            MigrationSource::Embedded => MIGRATOR.run(&self.pool).await?,
            MigrationSource::Directory(dir) => {
                let migrator = Migrator::new(dir.as_path()).await?;
                migrator.run(&self.pool).await?;
            }
        }
        Ok(())
    }
}

fn sqlite_pool_options() -> AnyPoolOptions {
    AnyPoolOptions::new()
        .max_connections(5)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("PRAGMA foreign_keys = ON")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("PRAGMA busy_timeout = 5000")
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        })
}

async fn load_workspace(
    pool: &AnyPool,
    dialect: SqlDialect,
) -> Result<WorkspaceSnapshot, Box<dyn std::error::Error>> {
    let people = load_people(pool).await?;
    let projects = load_projects(pool).await?;
    let tags = load_tags(pool).await?;
    let mut tasks = Vec::new();
    let rows = sqlx::query(
        "SELECT id, rank, title, state, workflow_state, CAST(CASE WHEN rejected THEN 1 ELSE 0 END AS BIGINT) AS rejected, size, priority, start_date, due_date, snoozed_until, description FROM tasks ORDER BY rank, id",
    )
    .fetch_all(pool)
    .await?;

    for row in rows {
        let id: String = row.try_get("id")?;
        let people_ids = load_task_people(pool, dialect, &id).await?;
        let project_ids = load_task_projects(pool, dialect, &id).await?;
        let tag_ids = load_task_tags(pool, dialect, &id).await?;
        let links = load_task_links(pool, dialect, &id).await?;

        let task = Task {
            id,
            rank: row.try_get("rank")?,
            title: row.try_get("title")?,
            state: if row.try_get::<i64, _>("rejected")? != 0 {
                TaskState::Rejected
            } else {
                parse_stored_state(
                    row.try_get::<String, _>("state")?,
                    row.try_get::<String, _>("workflow_state")?,
                )?
            },
            size: parse_size(row.try_get::<String, _>("size")?)?,
            priority: parse_priority(row.try_get::<String, _>("priority")?)?,
            start_date: row.try_get("start_date")?,
            due_date: row.try_get("due_date")?,
            snoozed_until: row
                .try_get::<Option<String>, _>("snoozed_until")?
                .map(|value| parse_datetime(&value))
                .transpose()?,
            people_ids,
            project_ids,
            tag_ids,
            links,
            description: row.try_get("description")?,
        };
        tasks.push(task);
    }

    Ok(WorkspaceSnapshot {
        tasks,
        people,
        projects,
        tags,
    })
}

pub(crate) async fn load_workspace_for_service(
    pool: &AnyPool,
    dialect: SqlDialect,
) -> Result<WorkspaceSnapshot, Box<dyn std::error::Error>> {
    load_workspace(pool, dialect).await
}

async fn load_people(pool: &AnyPool) -> Result<Vec<Person>, Box<dyn std::error::Error>> {
    let rows = sqlx::query(
        "SELECT id, name, email, about, CAST(CASE WHEN active THEN 1 ELSE 0 END AS BIGINT) AS active FROM people ORDER BY sort_order, name",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(Person {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                email: row.try_get("email")?,
                about: row.try_get("about")?,
                active: row.try_get::<i64, _>("active")? != 0,
            })
        })
        .collect()
}

async fn load_projects(pool: &AnyPool) -> Result<Vec<Project>, Box<dyn std::error::Error>> {
    let rows = sqlx::query(
        "SELECT id, key, name, description, lead_person_id FROM projects ORDER BY sort_order, name",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(Project {
                id: row.try_get("id")?,
                key: row.try_get("key")?,
                name: row.try_get("name")?,
                description: row.try_get("description")?,
                lead_person_id: row.try_get("lead_person_id")?,
            })
        })
        .collect()
}

async fn load_task_people(
    pool: &AnyPool,
    dialect: SqlDialect,
    task_id: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let query = format!(
        "SELECT person_id FROM task_people WHERE task_id = {} ORDER BY sort_order, person_id",
        dialect.placeholder(1)
    );
    let rows = sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(task_id)
        .fetch_all(pool)
        .await?;
    rows.into_iter()
        .map(|row| Ok(row.try_get("person_id")?))
        .collect()
}

async fn load_task_projects(
    pool: &AnyPool,
    dialect: SqlDialect,
    task_id: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let query = format!(
        "SELECT project_id FROM task_projects WHERE task_id = {} ORDER BY sort_order, project_id",
        dialect.placeholder(1)
    );
    let rows = sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(task_id)
        .fetch_all(pool)
        .await?;
    rows.into_iter()
        .map(|row| Ok(row.try_get("project_id")?))
        .collect()
}

async fn load_tags(pool: &AnyPool) -> Result<Vec<Tag>, Box<dyn std::error::Error>> {
    let rows = sqlx::query("SELECT id, label FROM tags ORDER BY sort_order, label")
        .fetch_all(pool)
        .await?;
    rows.into_iter()
        .map(|row| {
            Ok(Tag {
                id: row.try_get("id")?,
                label: row.try_get("label")?,
            })
        })
        .collect()
}

async fn load_task_tags(
    pool: &AnyPool,
    dialect: SqlDialect,
    task_id: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let query = format!(
        "SELECT tag_id FROM task_tags WHERE task_id = {} ORDER BY sort_order, tag_id",
        dialect.placeholder(1)
    );
    let rows = sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(task_id)
        .fetch_all(pool)
        .await?;
    rows.into_iter()
        .map(|row| Ok(row.try_get("tag_id")?))
        .collect()
}

async fn load_task_links(
    pool: &AnyPool,
    dialect: SqlDialect,
    task_id: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let query = format!(
        "SELECT url FROM task_links WHERE task_id = {}",
        dialect.placeholder(1)
    );
    let rows = sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(task_id)
        .fetch_all(pool)
        .await?;
    let mut links = rows
        .into_iter()
        .map(|row| Ok(row.try_get("url")?))
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    links.sort();
    Ok(links)
}

fn default_sqlite_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(crate::paths::data_dir()?.join("tuido").join("tuido.sqlite"))
}

fn auto_migrate(value: Result<String, env::VarError>) -> Result<bool, Box<dyn std::error::Error>> {
    let value = match value {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Ok(true),
        Err(error) => return Err(error.into()),
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(format!(
            "TUIDO_AUTO_MIGRATE must be one of 1, true, yes, on, 0, false, no, or off; got `{value}`"
        )
        .into()),
    }
}

fn migration_source(
    auto_migrate_value: Result<String, env::VarError>,
    migrations_dir: impl FnOnce() -> Result<Option<PathBuf>, Box<dyn std::error::Error>>,
) -> Result<MigrationSource, Box<dyn std::error::Error>> {
    if !auto_migrate(auto_migrate_value)? {
        return Ok(MigrationSource::Disabled);
    }
    match migrations_dir()? {
        Some(path) if !path.is_absolute() => Err(format!(
            "TUIDO_MIGRATIONS_DIR must be an absolute path: {}",
            path.display()
        )
        .into()),
        Some(path) => Ok(MigrationSource::Directory(path)),
        None => Ok(MigrationSource::Embedded),
    }
}

fn parse_state(value: String) -> Result<TaskState, Box<dyn std::error::Error>> {
    TaskState::parse_persisted(&value).ok_or_else(|| format!("unknown task state: {value}").into())
}

fn parse_stored_state(
    legacy_state: String,
    workflow_state: String,
) -> Result<TaskState, Box<dyn std::error::Error>> {
    if legacy_state == "waiting" && workflow_state == "todo" {
        Ok(TaskState::Backlog)
    } else {
        parse_state(workflow_state)
    }
}

async fn configure_sqlite_journal(pool: &AnyPool) -> Result<(), Box<dyn std::error::Error>> {
    let mode = sqlx::query("PRAGMA journal_mode = WAL")
        .fetch_one(pool)
        .await?
        .try_get::<String, _>(0)?;
    match mode.to_ascii_lowercase().as_str() {
        "wal" | "memory" => Ok(()),
        mode => Err(format!("SQLite WAL request returned unexpected journal mode: {mode}").into()),
    }
}

fn parse_size(value: String) -> Result<TaskSize, Box<dyn std::error::Error>> {
    TaskSize::parse(&value).ok_or_else(|| format!("unknown task size: {value}").into())
}

fn parse_priority(value: String) -> Result<TaskPriority, Box<dyn std::error::Error>> {
    TaskPriority::parse(&value).ok_or_else(|| format!("unknown task priority: {value}").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn fresh_migrated_database_loads_empty() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                sqlx::any::install_default_drivers();
                let pool = AnyPoolOptions::new()
                    .max_connections(1)
                    .connect("sqlite::memory:")
                    .await
                    .unwrap();
                MIGRATOR.run(&pool).await.unwrap();
                let storage = Storage {
                    pool,
                    dialect: SqlDialect::Sqlite,
                    notification_url: None,
                };

                let snapshot = load_workspace(&storage.pool, storage.dialect)
                    .await
                    .unwrap();

                assert!(snapshot.tasks.is_empty());
                assert!(snapshot.people.is_empty());
                assert!(snapshot.projects.is_empty());
                assert!(snapshot.tags.is_empty());
            });
    }

    #[test]
    fn task_description_migration_preserves_existing_data() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                sqlx::any::install_default_drivers();
                let pool = AnyPoolOptions::new()
                    .max_connections(1)
                    .connect("sqlite::memory:")
                    .await
                    .unwrap();
                sqlx::query(
                    "CREATE TABLE tasks (id TEXT PRIMARY KEY, detail TEXT NOT NULL DEFAULT '')",
                )
                .execute(&pool)
                .await
                .unwrap();
                sqlx::query("INSERT INTO tasks (id, detail) VALUES ('task-1', 'Keep me')")
                    .execute(&pool)
                    .await
                    .unwrap();

                sqlx::query(include_str!("../migrations/0008_task_description.sql"))
                    .execute(&pool)
                    .await
                    .unwrap();

                let row = sqlx::query("SELECT description FROM tasks WHERE id = 'task-1'")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                assert_eq!(row.try_get::<String, _>("description").unwrap(), "Keep me");
            });
    }

    #[test]
    fn workspace_tasks_keep_creation_order() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                sqlx::any::install_default_drivers();
                let pool = AnyPoolOptions::new()
                    .max_connections(1)
                    .connect("sqlite::memory:")
                    .await
                    .unwrap();
                MIGRATOR.run(&pool).await.unwrap();
                for (id, title, created_at, rank) in [
                    ("z-first", "First", "2026-07-25T10:00:00", 1_i64),
                    ("a-second", "Second", "2026-07-25T10:01:00", 2_i64),
                ] {
                    sqlx::query("INSERT INTO tasks (id, rank, title, state, workflow_state, size, priority, created_at, updated_at) VALUES (?, ?, ?, 'next', 'todo', 'small', 'medium', ?, ?)")
                        .bind(id)
                        .bind(rank)
                        .bind(title)
                        .bind(created_at)
                        .bind(created_at)
                        .execute(&pool)
                        .await
                        .unwrap();
                }

                let snapshot = load_workspace(&pool, SqlDialect::Sqlite).await.unwrap();

                assert_eq!(
                    snapshot
                        .tasks
                        .iter()
                        .map(|task| task.id.as_str())
                        .collect::<Vec<_>>(),
                    ["z-first", "a-second"]
                );
            });
    }

    #[test]
    fn file_backed_sqlite_enables_connection_safety_settings() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let path =
                    std::env::temp_dir().join(format!("tuido-storage-{}.sqlite", Uuid::new_v4()));
                let url = format!("sqlite://{}?mode=rwc", path.display());
                let storage = Storage::connect(&url).await.unwrap();
                let journal_mode: String = sqlx::query("PRAGMA journal_mode")
                    .fetch_one(&storage.pool)
                    .await
                    .unwrap()
                    .try_get(0)
                    .unwrap();
                let foreign_keys: i64 = sqlx::query("PRAGMA foreign_keys")
                    .fetch_one(&storage.pool)
                    .await
                    .unwrap()
                    .try_get(0)
                    .unwrap();
                let busy_timeout: i64 = sqlx::query("PRAGMA busy_timeout")
                    .fetch_one(&storage.pool)
                    .await
                    .unwrap()
                    .try_get(0)
                    .unwrap();

                assert_eq!(journal_mode, "wal");
                assert_eq!(foreign_keys, 1);
                assert_eq!(busy_timeout, 5000);

                storage.pool.close().await;
                let _ = fs::remove_file(path);
            });
    }

    #[test]
    fn sqlite_paths_with_url_characters_are_not_reinterpreted() {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let path = std::env::temp_dir()
                    .join(format!("tuido storage #{}?100%.sqlite", Uuid::new_v4()));
                let storage = Storage::connect_sqlite_path(path.clone()).await.unwrap();

                assert!(path.is_file());

                storage.pool.close().await;
                let _ = fs::remove_file(path);
            });
    }

    #[test]
    fn migration_opt_out_parsing_is_explicit() {
        for value in ["0", "false", "FALSE", " no ", "off"] {
            assert!(!auto_migrate(Ok(value.into())).unwrap());
        }
        for value in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(auto_migrate(Ok(value.into())).unwrap());
        }
        assert!(auto_migrate(Err(env::VarError::NotPresent)).unwrap());
        assert_eq!(
            auto_migrate(Ok("sometimes".into()))
                .unwrap_err()
                .to_string(),
            "TUIDO_AUTO_MIGRATE must be one of 1, true, yes, on, 0, false, no, or off; got `sometimes`"
        );
    }

    #[test]
    fn disabled_auto_migration_does_not_evaluate_migrations_dir() {
        let source = migration_source(Ok("false".into()), || {
            panic!("TUIDO_MIGRATIONS_DIR must not be evaluated")
        })
        .unwrap();

        assert_eq!(source, MigrationSource::Disabled);
    }
}
