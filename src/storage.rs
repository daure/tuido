use std::{env, fs, path::PathBuf};

#[cfg(test)]
use sqlx::AnyConnection;
use sqlx::{AnyPool, AssertSqlSafe, Row, any::AnyPoolOptions, migrate::Migrator};

use crate::domain::{
    Person, PersonPatch, Project, ProjectPatch, Tag, TagPatch, Task, TaskField, TaskPatch,
    TaskPriority, TaskSize, TaskState, WorkspaceSnapshot,
};

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

    fn placeholder(self, index: usize) -> String {
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
        sqlx::any::install_default_drivers();
        let database_url = database_url()?;
        let dialect = SqlDialect::from_database_url(&database_url)?;
        let pool = AnyPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await?;
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

    pub async fn load_workspace(&self) -> Result<WorkspaceSnapshot, Box<dyn std::error::Error>> {
        load_workspace(&self.pool, self.dialect).await
    }

    #[cfg(test)]
    async fn initialize_demo_workspace(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut tx = self.pool.begin().await?;
        seed_people(&mut tx, self.dialect).await?;
        seed_projects(&mut tx, self.dialect).await?;
        seed_tasks(&mut tx, self.dialect).await?;
        tx.commit().await?;
        Ok(())
    }
}

pub async fn create_task(
    pool: AnyPool,
    dialect: SqlDialect,
    task: Task,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let query = format!(
        "INSERT INTO tasks (id, title, state, workflow_state, rejected, size, priority, start_date, due_date, detail, created_at, updated_at) VALUES ({}, {}, 'next', {}, {}, {}, {}, {}, {}, {}, {}, {})",
        dialect.placeholder(1),
        dialect.placeholder(2),
        dialect.placeholder(3),
        dialect.placeholder(4),
        dialect.placeholder(5),
        dialect.placeholder(6),
        dialect.placeholder(7),
        dialect.placeholder(8),
        dialect.placeholder(9),
        dialect.placeholder(10),
        dialect.placeholder(11)
    );
    let now = now_text();
    sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(task.id)
        .bind(task.title)
        .bind(storage_state_id(task.state))
        .bind(task.state == TaskState::Rejected)
        .bind(task.size.id())
        .bind(task.priority.id())
        .bind(task.start_date)
        .bind(task.due_date)
        .bind(task.detail)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await?;
    Ok(())
}

pub async fn delete_task(
    pool: AnyPool,
    dialect: SqlDialect,
    task_id: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let query = format!("DELETE FROM tasks WHERE id = {}", dialect.placeholder(1));
    let result = sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(task_id)
        .execute(&pool)
        .await?;
    require_one_row(result.rows_affected(), "task delete")?;
    Ok(())
}

pub async fn save_patch(
    pool: AnyPool,
    dialect: SqlDialect,
    task_id: String,
    patch: TaskPatch,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match patch {
        TaskPatch::Title(value) => {
            update_task_text(
                pool,
                dialect,
                task_id,
                TaskField::Title,
                value.trim().to_string(),
            )
            .await
        }
        TaskPatch::Detail(value) => {
            update_task_text(pool, dialect, task_id, TaskField::Detail, value).await
        }
        TaskPatch::State(value) => update_task_state(pool, dialect, task_id, value).await,
        TaskPatch::Size(value) => {
            update_task_scalar(pool, dialect, task_id, TaskField::Size, value.id()).await
        }
        TaskPatch::Priority(value) => {
            update_task_scalar(pool, dialect, task_id, TaskField::Priority, value.id()).await
        }
        TaskPatch::StartDate(value) => {
            update_task_optional_date(pool, dialect, task_id, TaskField::StartDate, value).await
        }
        TaskPatch::EndDate(value) => {
            update_task_optional_date(pool, dialect, task_id, TaskField::EndDate, value).await
        }
        TaskPatch::People(value) => replace_task_people(pool, dialect, task_id, value).await,
        TaskPatch::Projects(value) => replace_task_projects(pool, dialect, task_id, value).await,
        TaskPatch::Tags(value) => replace_task_tags(pool, dialect, task_id, value).await,
    }
}

pub async fn save_person_patch(
    pool: AnyPool,
    dialect: SqlDialect,
    person_id: String,
    patch: PersonPatch,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match patch {
        PersonPatch::Name(value) => {
            let query = format!(
                "UPDATE people SET name = {} WHERE id = {}",
                dialect.placeholder(1),
                dialect.placeholder(2)
            );
            let result = sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(value.trim())
                .bind(&person_id)
                .execute(&pool)
                .await?;
            require_one_row(result.rows_affected(), "person update")?;
        }
        PersonPatch::Email(value) => {
            let query = format!(
                "UPDATE people SET email = {} WHERE id = {}",
                dialect.placeholder(1),
                dialect.placeholder(2)
            );
            let result = sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(value.trim())
                .bind(&person_id)
                .execute(&pool)
                .await?;
            require_one_row(result.rows_affected(), "person update")?;
        }
        PersonPatch::Active(value) => {
            let query = format!(
                "UPDATE people SET active = {} WHERE id = {}",
                dialect.placeholder(1),
                dialect.placeholder(2)
            );
            let result = sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(value)
                .bind(&person_id)
                .execute(&pool)
                .await?;
            require_one_row(result.rows_affected(), "person update")?;
        }
    }
    Ok(())
}

pub async fn save_project_patch(
    pool: AnyPool,
    dialect: SqlDialect,
    project_id: String,
    patch: ProjectPatch,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match patch {
        ProjectPatch::Key(value) => {
            let query = format!(
                "UPDATE projects SET key = {} WHERE id = {}",
                dialect.placeholder(1),
                dialect.placeholder(2)
            );
            let result = sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(value.trim())
                .bind(&project_id)
                .execute(&pool)
                .await?;
            require_one_row(result.rows_affected(), "project update")?;
        }
        ProjectPatch::Name(value) => {
            let query = format!(
                "UPDATE projects SET name = {} WHERE id = {}",
                dialect.placeholder(1),
                dialect.placeholder(2)
            );
            let result = sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(value.trim())
                .bind(&project_id)
                .execute(&pool)
                .await?;
            require_one_row(result.rows_affected(), "project update")?;
        }
        ProjectPatch::Description(value) => {
            let query = format!(
                "UPDATE projects SET description = {} WHERE id = {}",
                dialect.placeholder(1),
                dialect.placeholder(2)
            );
            let result = sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(value)
                .bind(&project_id)
                .execute(&pool)
                .await?;
            require_one_row(result.rows_affected(), "project update")?;
        }
        ProjectPatch::LeadPerson(value) => {
            let query = format!(
                "UPDATE projects SET lead_person_id = {} WHERE id = {}",
                dialect.placeholder(1),
                dialect.placeholder(2)
            );
            let result = sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(value)
                .bind(&project_id)
                .execute(&pool)
                .await?;
            require_one_row(result.rows_affected(), "project update")?;
        }
    }
    Ok(())
}

pub async fn save_tag_patch(
    pool: AnyPool,
    dialect: SqlDialect,
    tag_id: String,
    patch: TagPatch,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match patch {
        TagPatch::Label(value) => {
            let query = format!(
                "UPDATE tags SET label = {} WHERE id = {}",
                dialect.placeholder(1),
                dialect.placeholder(2)
            );
            let result = sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(value.trim())
                .bind(tag_id)
                .execute(&pool)
                .await?;
            require_one_row(result.rows_affected(), "tag update")?;
        }
    }
    Ok(())
}

async fn update_task_scalar(
    pool: AnyPool,
    dialect: SqlDialect,
    task_id: String,
    field: TaskField,
    value: &'static str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let column = match field {
        TaskField::Title
        | TaskField::Detail
        | TaskField::StartDate
        | TaskField::EndDate
        | TaskField::People
        | TaskField::Projects
        | TaskField::Tags => return Ok(()),
        TaskField::State => return Ok(()),
        TaskField::Size => "size",
        TaskField::Priority => "priority",
    };
    let query = update_task_column_sql(dialect, column);
    let result = sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(value)
        .bind(now_text())
        .bind(task_id)
        .execute(&pool)
        .await?;
    require_one_row(result.rows_affected(), "task update")?;
    Ok(())
}

async fn update_task_state(
    pool: AnyPool,
    dialect: SqlDialect,
    task_id: String,
    value: TaskState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let query = format!(
        "UPDATE tasks SET workflow_state = {}, rejected = {}, updated_at = {} WHERE id = {}",
        dialect.placeholder(1),
        dialect.placeholder(2),
        dialect.placeholder(3),
        dialect.placeholder(4)
    );
    let result = sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(storage_state_id(value))
        .bind(value == TaskState::Rejected)
        .bind(now_text())
        .bind(task_id)
        .execute(&pool)
        .await?;
    require_one_row(result.rows_affected(), "task state update")?;
    Ok(())
}

fn storage_state_id(value: TaskState) -> &'static str {
    match value {
        TaskState::Rejected => TaskState::Snoozed.id(),
        _ => value.id(),
    }
}

async fn update_task_text(
    pool: AnyPool,
    dialect: SqlDialect,
    task_id: String,
    field: TaskField,
    value: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let column = match field {
        TaskField::Title => "title",
        TaskField::Detail => "detail",
        _ => return Ok(()),
    };
    let query = update_task_column_sql(dialect, column);
    let result = sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(value)
        .bind(now_text())
        .bind(task_id)
        .execute(&pool)
        .await?;
    require_one_row(result.rows_affected(), "task update")?;
    Ok(())
}

async fn update_task_optional_date(
    pool: AnyPool,
    dialect: SqlDialect,
    task_id: String,
    field: TaskField,
    value: Option<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let column = match field {
        TaskField::StartDate => "start_date",
        TaskField::EndDate => "due_date",
        _ => return Ok(()),
    };
    let query = update_task_column_sql(dialect, column);
    let result = sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(value)
        .bind(now_text())
        .bind(task_id)
        .execute(&pool)
        .await?;
    require_one_row(result.rows_affected(), "task update")?;
    Ok(())
}

async fn replace_task_people(
    pool: AnyPool,
    dialect: SqlDialect,
    task_id: String,
    person_ids: Vec<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    validate_existing_ids(&pool, dialect, "people", &person_ids).await?;
    let delete_query = format!(
        "DELETE FROM task_people WHERE task_id = {}",
        dialect.placeholder(1)
    );
    let insert_query = format!(
        "INSERT INTO task_people (task_id, person_id, sort_order) VALUES ({}, {}, {})",
        dialect.placeholder(1),
        dialect.placeholder(2),
        dialect.placeholder(3)
    );
    let touch_query = update_task_timestamp_sql(dialect);
    let mut tx = pool.begin().await?;
    sqlx::query(AssertSqlSafe(delete_query.as_str()))
        .bind(&task_id)
        .execute(&mut *tx)
        .await?;
    for (index, person_id) in person_ids.iter().enumerate() {
        sqlx::query(AssertSqlSafe(insert_query.as_str()))
            .bind(&task_id)
            .bind(person_id)
            .bind(index as i64)
            .execute(&mut *tx)
            .await?;
    }
    let result = sqlx::query(AssertSqlSafe(touch_query.as_str()))
        .bind(now_text())
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
    require_one_row(result.rows_affected(), "task relation update")?;
    tx.commit().await?;
    Ok(())
}

async fn replace_task_projects(
    pool: AnyPool,
    dialect: SqlDialect,
    task_id: String,
    project_ids: Vec<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    validate_existing_ids(&pool, dialect, "projects", &project_ids).await?;
    let delete_query = format!(
        "DELETE FROM task_projects WHERE task_id = {}",
        dialect.placeholder(1)
    );
    let insert_query = format!(
        "INSERT INTO task_projects (task_id, project_id, sort_order) VALUES ({}, {}, {})",
        dialect.placeholder(1),
        dialect.placeholder(2),
        dialect.placeholder(3)
    );
    let touch_query = update_task_timestamp_sql(dialect);
    let mut tx = pool.begin().await?;
    sqlx::query(AssertSqlSafe(delete_query.as_str()))
        .bind(&task_id)
        .execute(&mut *tx)
        .await?;
    for (index, project_id) in project_ids.iter().enumerate() {
        sqlx::query(AssertSqlSafe(insert_query.as_str()))
            .bind(&task_id)
            .bind(project_id)
            .bind(index as i64)
            .execute(&mut *tx)
            .await?;
    }
    let result = sqlx::query(AssertSqlSafe(touch_query.as_str()))
        .bind(now_text())
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
    require_one_row(result.rows_affected(), "task relation update")?;
    tx.commit().await?;
    Ok(())
}

async fn replace_task_tags(
    pool: AnyPool,
    dialect: SqlDialect,
    task_id: String,
    tags: Vec<Tag>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let select_query = format!(
        "SELECT id FROM tags WHERE label = {}",
        dialect.placeholder(1)
    );
    let insert_tag_query = format!(
        "INSERT INTO tags (id, label, sort_order) VALUES ({}, {}, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM tags)) ON CONFLICT(label) DO NOTHING",
        dialect.placeholder(1),
        dialect.placeholder(2)
    );
    let delete_query = format!(
        "DELETE FROM task_tags WHERE task_id = {}",
        dialect.placeholder(1)
    );
    let insert_link_query = format!(
        "INSERT INTO task_tags (task_id, tag_id, sort_order) VALUES ({}, {}, {})",
        dialect.placeholder(1),
        dialect.placeholder(2),
        dialect.placeholder(3)
    );
    let touch_query = update_task_timestamp_sql(dialect);
    let mut tx = pool.begin().await?;
    let mut tag_ids = Vec::new();
    for tag in tags {
        let label = tag.label.trim();
        if label.is_empty() {
            continue;
        }
        sqlx::query(AssertSqlSafe(insert_tag_query.as_str()))
            .bind(&tag.id)
            .bind(label)
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query(AssertSqlSafe(select_query.as_str()))
            .bind(label)
            .fetch_one(&mut *tx)
            .await?;
        let id: String = row.try_get("id")?;
        if !tag_ids.contains(&id) {
            tag_ids.push(id);
        }
    }
    sqlx::query(AssertSqlSafe(delete_query.as_str()))
        .bind(&task_id)
        .execute(&mut *tx)
        .await?;
    for (index, tag_id) in tag_ids.iter().enumerate() {
        sqlx::query(AssertSqlSafe(insert_link_query.as_str()))
            .bind(&task_id)
            .bind(tag_id)
            .bind(index as i64)
            .execute(&mut *tx)
            .await?;
    }
    let result = sqlx::query(AssertSqlSafe(touch_query.as_str()))
        .bind(now_text())
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
    require_one_row(result.rows_affected(), "task relation update")?;
    tx.commit().await?;
    Ok(())
}

async fn validate_existing_ids(
    pool: &AnyPool,
    dialect: SqlDialect,
    table: &'static str,
    ids: &[String],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let query = format!(
        "SELECT COUNT(*) AS count FROM {table} WHERE id = {}",
        dialect.placeholder(1)
    );
    for id in ids {
        let row = sqlx::query(AssertSqlSafe(query.as_str()))
            .bind(id)
            .fetch_one(pool)
            .await?;
        let count: i64 = row.try_get("count")?;
        if count == 0 {
            return Err(format!("unknown {table} id: {id}").into());
        }
    }
    Ok(())
}

fn update_task_column_sql(dialect: SqlDialect, column: &str) -> String {
    format!(
        "UPDATE tasks SET {column} = {}, updated_at = {} WHERE id = {}",
        dialect.placeholder(1),
        dialect.placeholder(2),
        dialect.placeholder(3)
    )
}

fn update_task_timestamp_sql(dialect: SqlDialect) -> String {
    format!(
        "UPDATE tasks SET updated_at = {} WHERE id = {}",
        dialect.placeholder(1),
        dialect.placeholder(2)
    )
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
        "SELECT id, title, workflow_state, CAST(CASE WHEN rejected THEN 1 ELSE 0 END AS BIGINT) AS rejected, size, priority, start_date, due_date, detail FROM tasks ORDER BY id",
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

#[cfg(test)]
async fn seed_people(
    connection: &mut AnyConnection,
    dialect: SqlDialect,
) -> Result<(), Box<dyn std::error::Error>> {
    let people = [
        ("alice", "Alice", "alice@example.com"),
        ("carter", "Carter", "carter@example.com"),
    ];
    let query = format!(
        "INSERT INTO people (id, name, email, active, sort_order) VALUES ({}, {}, {}, {}, {})",
        dialect.placeholder(1),
        dialect.placeholder(2),
        dialect.placeholder(3),
        dialect.placeholder(4),
        dialect.placeholder(5)
    );
    for (index, (id, name, email)) in people.iter().enumerate() {
        sqlx::query(AssertSqlSafe(query.as_str()))
            .bind(id)
            .bind(name)
            .bind(email)
            .bind(true)
            .bind(index as i64)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
async fn seed_projects(
    connection: &mut AnyConnection,
    dialect: SqlDialect,
) -> Result<(), Box<dyn std::error::Error>> {
    let projects = [
        (
            "launch",
            "LAUNCH",
            "Launch",
            "Launch planning",
            Some("alice"),
        ),
        (
            "renewal",
            "RENEW",
            "Renewal",
            "Renewal tracking",
            Some("carter"),
        ),
        ("audit", "AUDIT", "Audit", "Audit evidence", None),
    ];
    let query = format!(
        "INSERT INTO projects (id, key, name, description, lead_person_id, sort_order) VALUES ({}, {}, {}, {}, {}, {})",
        dialect.placeholder(1),
        dialect.placeholder(2),
        dialect.placeholder(3),
        dialect.placeholder(4),
        dialect.placeholder(5),
        dialect.placeholder(6)
    );
    for (index, (id, key, name, description, lead_person_id)) in projects.iter().enumerate() {
        sqlx::query(AssertSqlSafe(query.as_str()))
            .bind(id)
            .bind(key)
            .bind(name)
            .bind(description)
            .bind(lead_person_id)
            .bind(index as i64)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
async fn seed_tasks(
    connection: &mut AnyConnection,
    dialect: SqlDialect,
) -> Result<(), Box<dyn std::error::Error>> {
    let now = now_text();
    let tasks = seed_task_rows();
    let task_query = format!(
        "INSERT INTO tasks (id, title, state, workflow_state, rejected, size, priority, start_date, due_date, detail, created_at, updated_at) VALUES ({}, {}, 'next', {}, {}, {}, {}, {}, {}, {}, {}, {})",
        dialect.placeholder(1),
        dialect.placeholder(2),
        dialect.placeholder(3),
        dialect.placeholder(4),
        dialect.placeholder(5),
        dialect.placeholder(6),
        dialect.placeholder(7),
        dialect.placeholder(8),
        dialect.placeholder(9),
        dialect.placeholder(10),
        dialect.placeholder(11)
    );
    let task_people_query = format!(
        "INSERT INTO task_people (task_id, person_id, sort_order) VALUES ({}, {}, {})",
        dialect.placeholder(1),
        dialect.placeholder(2),
        dialect.placeholder(3)
    );
    let task_project_query = format!(
        "INSERT INTO task_projects (task_id, project_id, sort_order) VALUES ({}, {}, {})",
        dialect.placeholder(1),
        dialect.placeholder(2),
        dialect.placeholder(3)
    );
    for task in tasks {
        sqlx::query(AssertSqlSafe(task_query.as_str()))
            .bind(task.id)
            .bind(task.title)
            .bind(task.state)
            .bind(false)
            .bind(task.size)
            .bind(task.priority)
            .bind(task.start_date)
            .bind(task.due_date)
            .bind(task.detail)
            .bind(&now)
            .bind(&now)
            .execute(&mut *connection)
            .await?;
        for (index, person_id) in task.person_ids.iter().enumerate() {
            sqlx::query(AssertSqlSafe(task_people_query.as_str()))
                .bind(task.id)
                .bind(person_id)
                .bind(index as i64)
                .execute(&mut *connection)
                .await?;
        }
        for (index, project_id) in task.project_ids.iter().enumerate() {
            sqlx::query(AssertSqlSafe(task_project_query.as_str()))
                .bind(task.id)
                .bind(project_id)
                .bind(index as i64)
                .execute(&mut *connection)
                .await?;
        }
    }
    Ok(())
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
    TaskState::parse(&value).ok_or_else(|| format!("unknown task state: {value}").into())
}

fn parse_size(value: String) -> Result<TaskSize, Box<dyn std::error::Error>> {
    TaskSize::parse(&value).ok_or_else(|| format!("unknown task size: {value}").into())
}

fn parse_priority(value: String) -> Result<TaskPriority, Box<dyn std::error::Error>> {
    TaskPriority::parse(&value).ok_or_else(|| format!("unknown task priority: {value}").into())
}

fn now_text() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs().to_string()
}

fn require_one_row(
    rows_affected: u64,
    operation: &'static str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if rows_affected == 1 {
        Ok(())
    } else {
        Err(format!("{operation} affected {rows_affected} rows; expected 1").into())
    }
}

#[cfg(test)]
struct SeedTask<'a> {
    id: &'a str,
    title: &'a str,
    state: &'a str,
    size: &'a str,
    priority: &'a str,
    start_date: Option<&'a str>,
    due_date: Option<&'a str>,
    person_ids: &'a [&'a str],
    project_ids: &'a [&'a str],
    detail: &'a str,
}

#[cfg(test)]
fn seed_task_rows() -> Vec<SeedTask<'static>> {
    vec![
        SeedTask {
            id: "T-101",
            title: "Email Carter for contract redlines",
            state: "todo",
            size: "small",
            priority: "high",
            start_date: None,
            due_date: None,
            person_ids: &["carter"],
            project_ids: &["renewal"],
            detail: "Clarified from a messy Sales note. Needs one concise email asking Carter for redline status and blockers.",
        },
        SeedTask {
            id: "T-102",
            title: "Draft launch cutover checklist",
            state: "in_progress",
            size: "big",
            priority: "high",
            start_date: None,
            due_date: None,
            person_ids: &["alice"],
            project_ids: &["launch"],
            detail: "Create the first useful checklist pass. Include owners, rollback trigger, comms, and validation steps.",
        },
        SeedTask {
            id: "T-103",
            title: "Wait for owner on pricing question",
            state: "todo",
            size: "small",
            priority: "low",
            start_date: None,
            due_date: None,
            person_ids: &[],
            project_ids: &[],
            detail: "Track dependency without letting it pollute active doing. Follow up only if no owner appears by tomorrow.",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn run_async<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime builds")
            .block_on(future)
    }

    fn sqlite_test_url(test_name: &str) -> (String, PathBuf) {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "tuido-{test_name}-{}-{nonce}.sqlite",
            std::process::id()
        ));
        (format!("sqlite://{}?mode=rwc", path.display()), path)
    }

    #[test]
    fn postgres_sql_uses_numbered_placeholders() {
        assert_eq!(
            update_task_column_sql(SqlDialect::Postgres, "title"),
            "UPDATE tasks SET title = $1, updated_at = $2 WHERE id = $3"
        );
        assert_eq!(
            update_task_timestamp_sql(SqlDialect::Postgres),
            "UPDATE tasks SET updated_at = $1 WHERE id = $2"
        );
    }

    #[test]
    fn sqlite_sql_uses_question_placeholders() {
        assert_eq!(
            update_task_column_sql(SqlDialect::Sqlite, "title"),
            "UPDATE tasks SET title = ?, updated_at = ? WHERE id = ?"
        );
        assert_eq!(
            update_task_timestamp_sql(SqlDialect::Sqlite),
            "UPDATE tasks SET updated_at = ? WHERE id = ?"
        );
    }

    #[test]
    fn fresh_migrated_database_loads_empty() {
        run_async(async {
            sqlx::any::install_default_drivers();
            let (database_url, database_path) = sqlite_test_url("fresh-empty");
            let pool = AnyPoolOptions::new()
                .max_connections(1)
                .connect(&database_url)
                .await
                .expect("sqlite test database connects");
            MIGRATOR.run(&pool).await.expect("migrations run");
            let storage = Storage {
                pool: pool.clone(),
                dialect: SqlDialect::Sqlite,
            };

            let snapshot = storage.load_workspace().await.expect("workspace loads");

            assert!(snapshot.tasks.is_empty());
            assert!(snapshot.people.is_empty());
            assert!(snapshot.projects.is_empty());
            assert!(snapshot.tags.is_empty());
            drop(storage);
            drop(pool);
            let _ = std::fs::remove_file(database_path);
        });
    }

    #[test]
    fn explicit_demo_initialization_and_immediate_task_saves_reload() {
        run_async(async {
            sqlx::any::install_default_drivers();
            let (database_url, database_path) = sqlite_test_url("round-trip");
            let pool = AnyPoolOptions::new()
                .max_connections(1)
                .connect(&database_url)
                .await
                .expect("sqlite test database connects");
            let storage = Storage {
                pool: pool.clone(),
                dialect: SqlDialect::Sqlite,
            };

            MIGRATOR.run(&pool).await.expect("migrations run");
            storage
                .initialize_demo_workspace()
                .await
                .expect("demo workspace initializes");
            let seeded = storage
                .load_workspace()
                .await
                .expect("seeded workspace loads");
            assert!(!seeded.tasks.is_empty());
            assert!(!seeded.people.is_empty());
            assert!(!seeded.projects.is_empty());
            let launch = seeded
                .tasks
                .iter()
                .find(|task| task.id == "T-102")
                .expect("launch task is seeded");
            assert_eq!(launch.start_date, None);
            assert_eq!(launch.due_date, None);

            save_patch(
                pool.clone(),
                SqlDialect::Sqlite,
                "T-103".to_string(),
                TaskPatch::Size(TaskSize::Big),
            )
            .await
            .expect("task size saves");
            save_patch(
                pool.clone(),
                SqlDialect::Sqlite,
                "T-103".to_string(),
                TaskPatch::Priority(TaskPriority::High),
            )
            .await
            .expect("task priority saves");
            save_patch(
                pool.clone(),
                SqlDialect::Sqlite,
                "T-103".to_string(),
                TaskPatch::State(TaskState::Rejected),
            )
            .await
            .expect("task state saves");
            save_patch(
                pool.clone(),
                SqlDialect::Sqlite,
                "T-103".to_string(),
                TaskPatch::People(vec!["alice".to_string()]),
            )
            .await
            .expect("task people save");
            save_patch(
                pool.clone(),
                SqlDialect::Sqlite,
                "T-103".to_string(),
                TaskPatch::Projects(vec!["audit".to_string()]),
            )
            .await
            .expect("task projects save");
            save_patch(
                pool.clone(),
                SqlDialect::Sqlite,
                "T-103".to_string(),
                TaskPatch::Tags(vec![Tag {
                    id: "tag-api".to_string(),
                    label: "api".to_string(),
                }]),
            )
            .await
            .expect("new task tag saves");
            save_patch(
                pool.clone(),
                SqlDialect::Sqlite,
                "T-101".to_string(),
                TaskPatch::Tags(vec![
                    Tag {
                        id: "different-api-id".to_string(),
                        label: "api".to_string(),
                    },
                    Tag {
                        id: "tag-backend".to_string(),
                        label: "backend".to_string(),
                    },
                ]),
            )
            .await
            .expect("existing and new task tags save");
            create_task(
                pool.clone(),
                SqlDialect::Sqlite,
                Task::quick_capture(
                    "new-task".to_string(),
                    "Captured task".to_string(),
                    "Captured details".to_string(),
                    TaskSize::Medium,
                ),
            )
            .await
            .expect("quick-captured task saves");

            let reloaded = storage.load_workspace().await.expect("workspace reloads");
            let task = reloaded
                .tasks
                .iter()
                .find(|task| task.id == "T-103")
                .expect("edited task exists after reload");
            assert_eq!(task.size, TaskSize::Big);
            assert_eq!(task.priority, TaskPriority::High);
            assert_eq!(task.state, TaskState::Rejected);
            assert_eq!(task.people_ids, vec!["alice"]);
            assert_eq!(task.project_ids, vec!["audit"]);
            assert_eq!(task.tag_ids, vec!["tag-api"]);
            let other_task = reloaded
                .tasks
                .iter()
                .find(|task| task.id == "T-101")
                .expect("second tagged task exists after reload");
            assert_eq!(
                other_task.tag_ids,
                vec!["tag-api".to_string(), "tag-backend".to_string()]
            );
            assert_eq!(
                reloaded.tags,
                vec![
                    Tag {
                        id: "tag-api".to_string(),
                        label: "api".to_string(),
                    },
                    Tag {
                        id: "tag-backend".to_string(),
                        label: "backend".to_string(),
                    },
                ]
            );
            save_tag_patch(
                pool.clone(),
                SqlDialect::Sqlite,
                "tag-backend".to_string(),
                TagPatch::Label("platform".to_string()),
            )
            .await
            .expect("tag label saves");
            let renamed = storage.load_workspace().await.expect("renamed tag reloads");
            assert!(
                renamed
                    .tags
                    .iter()
                    .any(|tag| tag.id == "tag-backend" && tag.label == "platform")
            );
            let created = reloaded
                .tasks
                .iter()
                .find(|task| task.id == "new-task")
                .expect("quick-captured task exists after reload");
            assert_eq!(created.title, "Captured task");
            assert_eq!(created.detail, "Captured details");
            assert_eq!(created.state, TaskState::Todo);
            assert_eq!(created.size, TaskSize::Medium);
            assert_eq!(created.priority, TaskPriority::Medium);

            delete_task(pool.clone(), SqlDialect::Sqlite, "new-task".to_string())
                .await
                .expect("task deletes");
            let after_delete = storage.load_workspace().await.expect("workspace reloads");
            assert!(after_delete.tasks.iter().all(|task| task.id != "new-task"));

            for task in after_delete.tasks {
                delete_task(pool.clone(), SqlDialect::Sqlite, task.id)
                    .await
                    .expect("seeded task deletes");
            }
            let empty = storage
                .load_workspace()
                .await
                .expect("empty initialized workspace reloads");
            assert!(empty.tasks.is_empty());

            drop(storage);
            drop(pool);
            let _ = std::fs::remove_file(database_path);
        });
    }

    #[test]
    fn existing_workspace_loads_without_samples() {
        run_async(async {
            sqlx::any::install_default_drivers();
            let (database_url, database_path) = sqlite_test_url("existing-workspace");
            let pool = AnyPoolOptions::new()
                .max_connections(1)
                .connect(&database_url)
                .await
                .expect("sqlite test database connects");
            MIGRATOR.run(&pool).await.expect("migrations run");
            create_task(
                pool.clone(),
                SqlDialect::Sqlite,
                Task::quick_capture(
                    "existing".to_string(),
                    "Existing".to_string(),
                    String::new(),
                    TaskSize::Small,
                ),
            )
            .await
            .expect("existing task inserts");
            let storage = Storage {
                pool: pool.clone(),
                dialect: SqlDialect::Sqlite,
            };

            let snapshot = storage.load_workspace().await.expect("workspace loads");

            assert_eq!(snapshot.tasks.len(), 1);
            assert_eq!(snapshot.tasks[0].id, "existing");
            assert!(snapshot.people.is_empty());
            assert!(snapshot.projects.is_empty());
            drop(storage);
            drop(pool);
            let _ = std::fs::remove_file(database_path);
        });
    }

    #[test]
    fn writes_to_unknown_ids_fail() {
        run_async(async {
            sqlx::any::install_default_drivers();
            let (database_url, database_path) = sqlite_test_url("unknown-ids");
            let pool = AnyPoolOptions::new()
                .max_connections(1)
                .connect(&database_url)
                .await
                .expect("sqlite test database connects");
            MIGRATOR.run(&pool).await.expect("migrations run");

            assert!(
                delete_task(pool.clone(), SqlDialect::Sqlite, "missing".to_string())
                    .await
                    .is_err()
            );
            assert!(
                save_patch(
                    pool.clone(),
                    SqlDialect::Sqlite,
                    "missing".to_string(),
                    TaskPatch::Title("No task".to_string()),
                )
                .await
                .is_err()
            );
            assert!(
                save_patch(
                    pool.clone(),
                    SqlDialect::Sqlite,
                    "missing".to_string(),
                    TaskPatch::People(Vec::new()),
                )
                .await
                .is_err()
            );
            assert!(
                save_person_patch(
                    pool.clone(),
                    SqlDialect::Sqlite,
                    "missing".to_string(),
                    PersonPatch::Name("Nobody".to_string()),
                )
                .await
                .is_err()
            );
            assert!(
                save_project_patch(
                    pool.clone(),
                    SqlDialect::Sqlite,
                    "missing".to_string(),
                    ProjectPatch::Name("Nothing".to_string()),
                )
                .await
                .is_err()
            );
            assert!(
                save_tag_patch(
                    pool.clone(),
                    SqlDialect::Sqlite,
                    "missing".to_string(),
                    TagPatch::Label("none".to_string()),
                )
                .await
                .is_err()
            );

            drop(pool);
            let _ = std::fs::remove_file(database_path);
        });
    }
}
