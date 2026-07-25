use std::{env, fs, path::PathBuf};

use sqlx::{AnyPool, AssertSqlSafe, Row, any::AnyPoolOptions, migrate::Migrator};

use crate::domain::{
    Person, Project, Tag, Task, TaskPriority, TaskSize, TaskState, WorkspaceSnapshot,
};
use crate::snooze::parse_datetime;
use time::PrimitiveDateTime;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

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
}

impl Storage {
    pub async fn connect_from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let database_url = database_url()?;
        Self::connect(&database_url).await
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
        Ok(Self { pool, dialect })
    }

    pub fn pool(&self) -> AnyPool {
        self.pool.clone()
    }

    pub fn dialect(&self) -> SqlDialect {
        self.dialect
    }

    pub async fn migrate(&self) -> Result<(), Box<dyn std::error::Error>> {
        if env::var("TUIDO_AUTO_MIGRATE").is_ok_and(|value| value == "0" || value == "false") {
            return Ok(());
        }
        if let Ok(dir) = env::var("TUIDO_MIGRATIONS_DIR") {
            let migrator = Migrator::new(PathBuf::from(dir).as_path()).await?;
            migrator.run(&self.pool).await?;
        } else {
            MIGRATOR.run(&self.pool).await?;
        }
        Ok(())
    }

    pub async fn load_last_custom_snooze(
        &self,
    ) -> Result<Option<PrimitiveDateTime>, Box<dyn std::error::Error>> {
        let row = sqlx::query("SELECT value FROM settings WHERE key = 'last_custom_snooze'")
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else { return Ok(None) };
        let value: String = row.try_get("value")?;
        Ok(Some(parse_datetime(&value)?))
    }
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
        "SELECT id, title, workflow_state, CAST(CASE WHEN rejected THEN 1 ELSE 0 END AS BIGINT) AS rejected, size, priority, start_date, due_date, snoozed_until, detail FROM tasks ORDER BY id",
    )
    .fetch_all(pool)
    .await?;

    for row in rows {
        let id: String = row.try_get("id")?;
        let people_ids = load_task_people(pool, dialect, &id).await?;
        let project_ids = load_task_projects(pool, dialect, &id).await?;
        let tag_ids = load_task_tags(pool, dialect, &id).await?;

        let task = Task {
            id,
            title: row.try_get("title")?,
            state: if row.try_get::<i64, _>("rejected")? != 0 {
                TaskState::Rejected
            } else {
                parse_state(row.try_get::<String, _>("workflow_state")?)?
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
            detail: row.try_get("detail")?,
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
        "SELECT id, name, email, CAST(CASE WHEN active THEN 1 ELSE 0 END AS BIGINT) AS active FROM people ORDER BY sort_order, name",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(Person {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                email: row.try_get("email")?,
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

fn database_url() -> Result<String, Box<dyn std::error::Error>> {
    if let Ok(url) = env::var("TUIDO_DATABASE_URL") {
        return Ok(url);
    }
    let path = default_sqlite_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(format!("sqlite://{}?mode=rwc", path.display()))
}

fn default_sqlite_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(data_home) = env::var("XDG_DATA_HOME") {
        return Ok(PathBuf::from(data_home).join("tuido").join("tuido.sqlite"));
    }
    let home = env::var("HOME")?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("tuido")
        .join("tuido.sqlite"))
}

fn parse_state(value: String) -> Result<TaskState, Box<dyn std::error::Error>> {
    TaskState::parse_persisted(&value).ok_or_else(|| format!("unknown task state: {value}").into())
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
}
