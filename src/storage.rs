use std::{env, fs, path::PathBuf};

use sqlx::{AnyPool, AssertSqlSafe, Row, any::AnyPoolOptions, migrate::Migrator};

use crate::domain::{
    Person, PersonPatch, Project, ProjectPatch, Task, TaskField, TaskPatch, TaskSize, TaskState,
    TaskSubtype, TaskType, WorkspaceSnapshot, task_context_labels,
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
        seed_if_empty(&self.pool, self.dialect).await?;
        load_workspace(&self.pool, self.dialect).await
    }
}

pub async fn create_task(
    pool: AnyPool,
    dialect: SqlDialect,
    task: Task,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let query = format!(
        "INSERT INTO tasks (id, title, task_type, subtype, state, task_kind, workflow_state, size, start_date, due_date, focus_today, frog_candidate, detail, ai_rationale, swap_note, created_at, updated_at) VALUES ({}, {}, {}, {}, 'next', {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
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
        dialect.placeholder(11),
        dialect.placeholder(12),
        dialect.placeholder(13),
        dialect.placeholder(14),
        dialect.placeholder(15),
        dialect.placeholder(16)
    );
    let now = now_text();
    sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(task.id)
        .bind(task.title)
        .bind(task.task_type.id())
        .bind(task.subtype.id())
        .bind(task.subtype.workflow_kind())
        .bind(task.state.id())
        .bind(task.size.id())
        .bind(task.start_date)
        .bind(task.due_date)
        .bind(task.focus_today)
        .bind(task.frog_candidate)
        .bind(task.detail)
        .bind(task.ai_rationale)
        .bind(task.swap_note)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await?;
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
        TaskPatch::Type(value) => {
            update_task_scalar(pool, dialect, task_id, TaskField::Type, value.id()).await
        }
        TaskPatch::Subtype(value) => update_task_subtype(pool, dialect, task_id, value).await,
        TaskPatch::State(value) => {
            update_task_scalar(pool, dialect, task_id, TaskField::State, value.id()).await
        }
        TaskPatch::Size(value) => {
            update_task_scalar(pool, dialect, task_id, TaskField::Size, value.id()).await
        }
        TaskPatch::StartDate(value) => {
            update_task_optional_date(pool, dialect, task_id, TaskField::StartDate, value).await
        }
        TaskPatch::EndDate(value) => {
            update_task_optional_date(pool, dialect, task_id, TaskField::EndDate, value).await
        }
        TaskPatch::People(value) => replace_task_people(pool, dialect, task_id, value).await,
        TaskPatch::Projects(value) => replace_task_projects(pool, dialect, task_id, value).await,
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
            sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(value.trim())
                .bind(&person_id)
                .execute(&pool)
                .await?;
        }
        PersonPatch::Email(value) => {
            let query = format!(
                "UPDATE people SET email = {} WHERE id = {}",
                dialect.placeholder(1),
                dialect.placeholder(2)
            );
            sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(value.trim())
                .bind(&person_id)
                .execute(&pool)
                .await?;
        }
        PersonPatch::Active(value) => {
            let query = format!(
                "UPDATE people SET active = {} WHERE id = {}",
                dialect.placeholder(1),
                dialect.placeholder(2)
            );
            sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(value)
                .bind(&person_id)
                .execute(&pool)
                .await?;
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
            sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(value.trim())
                .bind(&project_id)
                .execute(&pool)
                .await?;
        }
        ProjectPatch::Name(value) => {
            let query = format!(
                "UPDATE projects SET name = {} WHERE id = {}",
                dialect.placeholder(1),
                dialect.placeholder(2)
            );
            sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(value.trim())
                .bind(&project_id)
                .execute(&pool)
                .await?;
        }
        ProjectPatch::Description(value) => {
            let query = format!(
                "UPDATE projects SET description = {} WHERE id = {}",
                dialect.placeholder(1),
                dialect.placeholder(2)
            );
            sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(value)
                .bind(&project_id)
                .execute(&pool)
                .await?;
        }
        ProjectPatch::LeadPerson(value) => {
            let query = format!(
                "UPDATE projects SET lead_person_id = {} WHERE id = {}",
                dialect.placeholder(1),
                dialect.placeholder(2)
            );
            sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(value)
                .bind(&project_id)
                .execute(&pool)
                .await?;
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
        | TaskField::Projects => return Ok(()),
        TaskField::Type => "task_type",
        TaskField::Subtype => "subtype",
        TaskField::State => "workflow_state",
        TaskField::Size => "size",
    };
    let query = update_task_column_sql(dialect, column);
    sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(value)
        .bind(now_text())
        .bind(task_id)
        .execute(&pool)
        .await?;
    Ok(())
}

async fn update_task_subtype(
    pool: AnyPool,
    dialect: SqlDialect,
    task_id: String,
    value: TaskSubtype,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let query = format!(
        "UPDATE tasks SET subtype = {}, task_kind = {}, updated_at = {} WHERE id = {}",
        dialect.placeholder(1),
        dialect.placeholder(2),
        dialect.placeholder(3),
        dialect.placeholder(4)
    );
    sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(value.id())
        .bind(value.workflow_kind())
        .bind(now_text())
        .bind(task_id)
        .execute(&pool)
        .await?;
    Ok(())
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
    sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(value)
        .bind(now_text())
        .bind(task_id)
        .execute(&pool)
        .await?;
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
    sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(value)
        .bind(now_text())
        .bind(task_id)
        .execute(&pool)
        .await?;
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
    sqlx::query(AssertSqlSafe(touch_query.as_str()))
        .bind(now_text())
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
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
    sqlx::query(AssertSqlSafe(touch_query.as_str()))
        .bind(now_text())
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
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
    let mut tasks = Vec::new();
    let rows = sqlx::query(
        "SELECT id, title, task_type, subtype, workflow_state, size, start_date, due_date, CAST(CASE WHEN focus_today THEN 1 ELSE 0 END AS BIGINT) AS focus_today, CAST(CASE WHEN frog_candidate THEN 1 ELSE 0 END AS BIGINT) AS frog_candidate, detail, ai_rationale, swap_note FROM tasks ORDER BY id",
    )
    .fetch_all(pool)
    .await?;

    for row in rows {
        let id: String = row.try_get("id")?;
        let people_ids = load_task_people(pool, dialect, &id).await?;
        let project_ids = load_task_projects(pool, dialect, &id).await?;
        let generic_entity_labels = load_task_generic_entity_labels(pool, dialect, &id).await?;

        let mut task = Task {
            id,
            title: row.try_get("title")?,
            task_type: parse_task_type(row.try_get::<String, _>("task_type")?)?,
            subtype: parse_subtype(row.try_get::<String, _>("subtype")?)?,
            state: parse_state(row.try_get::<String, _>("workflow_state")?)?,
            size: parse_size(row.try_get::<String, _>("size")?)?,
            start_date: row.try_get("start_date")?,
            due_date: row.try_get("due_date")?,
            people_ids,
            project_ids,
            entity_labels: Vec::new(),
            focus_today: row.try_get::<i64, _>("focus_today")? != 0,
            frog_candidate: row.try_get::<i64, _>("frog_candidate")? != 0,
            detail: row.try_get("detail")?,
            ai_rationale: row.try_get("ai_rationale")?,
            swap_note: row.try_get("swap_note")?,
        };
        task.entity_labels = task_context_labels(&task, &people, &projects);
        task.entity_labels.extend(generic_entity_labels);
        tasks.push(task);
    }

    Ok(WorkspaceSnapshot {
        tasks,
        people,
        projects,
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

async fn load_task_generic_entity_labels(
    pool: &AnyPool,
    dialect: SqlDialect,
    task_id: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let query = format!(
        "SELECT e.label FROM task_entities te INNER JOIN entities e ON e.id = te.entity_id WHERE te.task_id = {} AND e.entity_type NOT IN ('person', 'project') ORDER BY te.sort_order, e.label",
        dialect.placeholder(1)
    );
    let rows = sqlx::query(AssertSqlSafe(query.as_str()))
        .bind(task_id)
        .fetch_all(pool)
        .await?;
    rows.into_iter()
        .map(|row| Ok(row.try_get("label")?))
        .collect()
}

async fn seed_if_empty(
    pool: &AnyPool,
    dialect: SqlDialect,
) -> Result<(), Box<dyn std::error::Error>> {
    let row = sqlx::query("SELECT COUNT(*) AS count FROM tasks")
        .fetch_one(pool)
        .await?;
    let count: i64 = row.try_get("count")?;
    if count > 0 {
        return Ok(());
    }
    seed_people(pool, dialect).await?;
    seed_projects(pool, dialect).await?;
    seed_tasks(pool, dialect).await?;
    Ok(())
}

async fn seed_people(
    pool: &AnyPool,
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
            .execute(pool)
            .await?;
    }
    Ok(())
}

async fn seed_projects(
    pool: &AnyPool,
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
            .execute(pool)
            .await?;
    }
    Ok(())
}

async fn seed_tasks(pool: &AnyPool, dialect: SqlDialect) -> Result<(), Box<dyn std::error::Error>> {
    let now = now_text();
    let tasks = seed_task_rows();
    let task_query = format!(
        "INSERT INTO tasks (id, title, task_type, subtype, state, task_kind, workflow_state, size, start_date, due_date, focus_today, frog_candidate, detail, ai_rationale, swap_note, created_at, updated_at) VALUES ({}, {}, {}, {}, 'next', {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
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
        dialect.placeholder(11),
        dialect.placeholder(12),
        dialect.placeholder(13),
        dialect.placeholder(14),
        dialect.placeholder(15),
        dialect.placeholder(16)
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
            .bind(task.task_type)
            .bind(task.subtype)
            .bind(seed_workflow_kind(task.subtype))
            .bind(task.state)
            .bind(task.size)
            .bind(task.start_date)
            .bind(task.due_date)
            .bind(task.focus_today)
            .bind(task.frog_candidate)
            .bind(task.detail)
            .bind(task.ai_rationale)
            .bind(task.swap_note)
            .bind(&now)
            .bind(&now)
            .execute(pool)
            .await?;
        for (index, person_id) in task.person_ids.iter().enumerate() {
            sqlx::query(AssertSqlSafe(task_people_query.as_str()))
                .bind(task.id)
                .bind(person_id)
                .bind(index as i64)
                .execute(pool)
                .await?;
        }
        for (index, project_id) in task.project_ids.iter().enumerate() {
            sqlx::query(AssertSqlSafe(task_project_query.as_str()))
                .bind(task.id)
                .bind(project_id)
                .bind(index as i64)
                .execute(pool)
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

fn parse_task_type(value: String) -> Result<TaskType, Box<dyn std::error::Error>> {
    TaskType::parse(&value).ok_or_else(|| format!("unknown task type: {value}").into())
}

fn parse_subtype(value: String) -> Result<TaskSubtype, Box<dyn std::error::Error>> {
    TaskSubtype::parse(&value).ok_or_else(|| format!("unknown task subtype: {value}").into())
}

fn parse_state(value: String) -> Result<TaskState, Box<dyn std::error::Error>> {
    TaskState::parse(&value).ok_or_else(|| format!("unknown task state: {value}").into())
}

fn parse_size(value: String) -> Result<TaskSize, Box<dyn std::error::Error>> {
    TaskSize::parse(&value).ok_or_else(|| format!("unknown task size: {value}").into())
}

fn now_text() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_secs().to_string()
}

fn seed_workflow_kind(subtype: &str) -> &'static str {
    match subtype {
        "waiting" => "waiting",
        "follow_up" => "follow_up",
        _ => "action",
    }
}

struct SeedTask<'a> {
    id: &'a str,
    title: &'a str,
    task_type: &'a str,
    subtype: &'a str,
    state: &'a str,
    size: &'a str,
    start_date: Option<&'a str>,
    due_date: Option<&'a str>,
    person_ids: &'a [&'a str],
    project_ids: &'a [&'a str],
    focus_today: bool,
    frog_candidate: bool,
    detail: &'a str,
    ai_rationale: &'a str,
    swap_note: &'a str,
}

fn seed_task_rows() -> Vec<SeedTask<'static>> {
    vec![
        SeedTask {
            id: "T-101",
            title: "Email Carter for contract redlines",
            task_type: "action",
            subtype: "follow_up",
            state: "todo",
            size: "small",
            start_date: Some("today"),
            due_date: Some("Fri"),
            person_ids: &["carter"],
            project_ids: &["renewal"],
            focus_today: true,
            frog_candidate: false,
            detail: "Clarified from a messy Sales note. Needs one concise email asking Carter for redline status and blockers.",
            ai_rationale: "Due date plus named person makes this a concrete follow-up, not a reference note.",
            swap_note: "Small enough to add without removing a big item.",
        },
        SeedTask {
            id: "T-102",
            title: "Draft launch cutover checklist",
            task_type: "action",
            subtype: "task",
            state: "in_progress",
            size: "big",
            start_date: Some("today"),
            due_date: Some("Tue"),
            person_ids: &["alice"],
            project_ids: &["launch"],
            focus_today: true,
            frog_candidate: true,
            detail: "Create the first useful checklist pass. Include owners, rollback trigger, comms, and validation steps.",
            ai_rationale: "Large, time-relevant artifact: good frog candidate if uninterrupted time exists.",
            swap_note: "If urgent work enters, move a medium item out rather than silently overloading today.",
        },
        SeedTask {
            id: "T-103",
            title: "Wait for owner on pricing question",
            task_type: "action",
            subtype: "waiting",
            state: "todo",
            size: "small",
            start_date: Some("today"),
            due_date: None,
            person_ids: &[],
            project_ids: &[],
            focus_today: false,
            frog_candidate: false,
            detail: "Track dependency without letting it pollute active doing. Follow up only if no owner appears by tomorrow.",
            ai_rationale: "Waiting is still actionable context, but not a separate top-level item type.",
            swap_note: "Can be snoozed until follow-up date once clarified.",
        },
        SeedTask {
            id: "T-104",
            title: "Clarify voicemail about audit evidence",
            task_type: "action",
            subtype: "task",
            state: "todo",
            size: "medium",
            start_date: None,
            due_date: Some("next week"),
            person_ids: &[],
            project_ids: &["audit"],
            focus_today: false,
            frog_candidate: false,
            detail: "Raw voicemail mentions evidence but lacks owner. Needs user review before board pull or snooze.",
            ai_rationale: "Insufficient trust: it has action shape, but missing context prevents silent organization.",
            swap_note: "Do not pull until clarified.",
        },
        SeedTask {
            id: "T-105",
            title: "Review returned docs reminder",
            task_type: "action",
            subtype: "task",
            state: "snoozed",
            size: "medium",
            start_date: Some("tomorrow"),
            due_date: None,
            person_ids: &[],
            project_ids: &[],
            focus_today: false,
            frog_candidate: false,
            detail: "Returned-from-snooze marker should appear before user decides whether this is action or reference.",
            ai_rationale: "Snoozed clarified items return to the clarified list, not straight to the board.",
            swap_note: "Hidden work stays safe without cluttering today's focus.",
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
    fn sqlite_migrations_seed_and_immediate_task_saves_reload() {
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
            let seeded = storage
                .load_workspace()
                .await
                .expect("seeded workspace loads");
            assert!(!seeded.tasks.is_empty());
            assert!(!seeded.people.is_empty());
            assert!(!seeded.projects.is_empty());

            save_patch(
                pool.clone(),
                SqlDialect::Sqlite,
                "T-103".to_string(),
                TaskPatch::Type(TaskType::Note),
            )
            .await
            .expect("task type saves");
            save_patch(
                pool.clone(),
                SqlDialect::Sqlite,
                "T-103".to_string(),
                TaskPatch::Subtype(TaskSubtype::ArtifactUpdate),
            )
            .await
            .expect("task subtype saves");
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
                TaskPatch::State(TaskState::Done),
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
            create_task(
                pool.clone(),
                SqlDialect::Sqlite,
                Task::quick_capture("new-task".to_string(), "Captured task".to_string()),
            )
            .await
            .expect("quick-captured task saves");

            let reloaded = storage.load_workspace().await.expect("workspace reloads");
            let task = reloaded
                .tasks
                .iter()
                .find(|task| task.id == "T-103")
                .expect("edited task exists after reload");
            assert_eq!(task.task_type, TaskType::Note);
            assert_eq!(task.subtype, TaskSubtype::ArtifactUpdate);
            assert_eq!(task.size, TaskSize::Big);
            assert_eq!(task.state, TaskState::Done);
            assert_eq!(task.people_ids, vec!["alice"]);
            assert_eq!(task.project_ids, vec!["audit"]);
            let created = reloaded
                .tasks
                .iter()
                .find(|task| task.id == "new-task")
                .expect("quick-captured task exists after reload");
            assert_eq!(created.title, "Captured task");
            assert_eq!(created.state, TaskState::Todo);
            assert_eq!(created.size, TaskSize::Small);

            drop(storage);
            drop(pool);
            let _ = std::fs::remove_file(database_path);
        });
    }
}
