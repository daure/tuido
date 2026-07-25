use std::{collections::HashMap, error::Error, fmt};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::{Any, AnyPool, AssertSqlSafe, Row, Transaction};
use uuid::Uuid;

use crate::{
    domain::{
        Person, PersonPatch, Project, ProjectPatch, Tag, TagPatch, Task, TaskPatch,
        WorkspaceSnapshot,
    },
    storage::{self, SqlDialect, Storage},
};

mod validation;

use validation::{
    task_matches_workspace_filter, validate_required, validate_task_patch,
    validate_task_temporal_fields, validate_task_update, validate_workspace_filter,
};

pub type ServiceResult<T> = Result<T, ServiceError>;

pub(crate) fn revision_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "integer",
        "minimum": 0,
        "description": "Internal synchronization and optimistic-concurrency token. For entity mutations, pass the latest entity revision as expected_revision. Do not present revisions as user-facing task metadata unless explicitly requested."
    })
}

#[derive(Debug)]
pub enum ServiceError {
    Conflict {
        entity: &'static str,
        id: String,
        expected: u64,
        actual: Option<u64>,
    },
    NotFound {
        entity: &'static str,
        id: String,
    },
    Invalid(String),
    Storage(String),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict {
                entity,
                id,
                expected,
                actual,
            } => write!(
                f,
                "{entity} {id} revision conflict: expected {expected}, actual {actual:?}"
            ),
            Self::NotFound { entity, id } => write!(f, "{entity} {id} not found"),
            Self::Invalid(message) | Self::Storage(message) => f.write_str(message),
        }
    }
}

impl Error for ServiceError {}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Versioned<T> {
    #[schemars(schema_with = "revision_schema")]
    pub revision: u64,
    pub value: T,
}

#[derive(Debug)]
pub struct TaskPatchResult {
    pub revision: u64,
    pub related_revisions: HashMap<String, u64>,
}

pub(crate) struct ConsistentWorkspace {
    pub snapshot: WorkspaceSnapshot,
    pub revision: u64,
    pub entity_revisions: HashMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct WorkspaceView {
    /// Internal change token for workspace refresh detection, not task metadata.
    #[schemars(schema_with = "revision_schema")]
    pub revision: u64,
    pub tasks: Vec<Versioned<TaskView>>,
    pub people: Vec<Versioned<PersonView>>,
    pub projects: Vec<Versioned<ProjectView>>,
    pub tags: Vec<Versioned<TagView>>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(default)]
pub struct WorkspaceFilter {
    /// Include done and rejected tasks, which are excluded by default.
    pub include_resolved: bool,
    /// Match user-facing task statuses such as todo, in_progress, snoozed, done, or rejected.
    pub states: Vec<String>,
    pub priorities: Vec<String>,
    pub sizes: Vec<String>,
    /// Match tasks involving any of these people. People are not assignees or owners.
    pub person_ids: Vec<String>,
    pub project_ids: Vec<String>,
    pub tag_ids: Vec<String>,
    pub due_before: Option<String>,
    pub due_after: Option<String>,
    pub query: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TaskView {
    pub id: String,
    pub title: String,
    /// User-facing task status: todo, in_progress, snoozed, done, or rejected.
    pub state: String,
    pub size: String,
    pub priority: String,
    pub start_date: Option<String>,
    pub due_date: Option<String>,
    pub snoozed_until: Option<String>,
    /// People involved in this task besides the workspace owner. These are related people, not
    /// assignees or owners.
    pub people_ids: Vec<String>,
    pub project_ids: Vec<String>,
    pub tag_ids: Vec<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PersonView {
    pub id: String,
    pub name: String,
    pub email: String,
    pub active: bool,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ProjectView {
    pub id: String,
    pub key: String,
    pub name: String,
    pub description: String,
    pub lead_person_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TagView {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TaskCreate {
    pub title: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default = "default_size")]
    pub size: String,
    #[serde(default = "default_state")]
    /// User-facing task status: todo, in_progress, snoozed, done, or rejected.
    pub state: String,
    #[serde(default = "default_priority")]
    pub priority: String,
    pub start_date: Option<String>,
    pub due_date: Option<String>,
    pub snoozed_until: Option<String>,
    #[serde(default)]
    /// People involved in this task besides the workspace owner; not assignees or owners.
    pub people_ids: Vec<String>,
    #[serde(default)]
    pub project_ids: Vec<String>,
    #[serde(default)]
    pub tag_ids: Vec<String>,
}
fn default_size() -> String {
    "medium".into()
}
fn default_state() -> String {
    "todo".into()
}
fn default_priority() -> String {
    "medium".into()
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TaskUpdate {
    pub id: String,
    #[schemars(schema_with = "revision_schema")]
    pub expected_revision: u64,
    pub title: String,
    /// User-facing task status: todo, in_progress, snoozed, done, or rejected.
    pub state: String,
    pub size: String,
    pub priority: String,
    pub start_date: Option<String>,
    pub due_date: Option<String>,
    pub snoozed_until: Option<String>,
    #[serde(default)]
    /// People involved in this task besides the workspace owner; not assignees or owners.
    pub people_ids: Vec<String>,
    #[serde(default)]
    pub project_ids: Vec<String>,
    #[serde(default)]
    pub tag_ids: Vec<String>,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PersonInput {
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default = "default_true")]
    pub active: bool,
}
fn default_true() -> bool {
    true
}
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ProjectInput {
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub lead_person_id: Option<String>,
}
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TagInput {
    pub label: String,
}

#[derive(Clone)]
pub struct TuidoService {
    pool: AnyPool,
    dialect: SqlDialect,
}

impl TuidoService {
    pub async fn connect() -> ServiceResult<Self> {
        let storage = Storage::connect_from_env().await.map_err(storage_error)?;
        storage.migrate().await.map_err(storage_error)?;
        Ok(Self {
            pool: storage.pool(),
            dialect: storage.dialect(),
        })
    }

    pub async fn connect_url(database_url: &str) -> ServiceResult<Self> {
        let storage = Storage::connect(database_url)
            .await
            .map_err(storage_error)?;
        storage.migrate().await.map_err(storage_error)?;
        Ok(Self::from_storage(&storage))
    }

    pub(crate) fn from_storage(storage: &Storage) -> Self {
        Self {
            pool: storage.pool(),
            dialect: storage.dialect(),
        }
    }

    pub(crate) fn from_parts(pool: AnyPool, dialect: SqlDialect) -> Self {
        Self { pool, dialect }
    }

    pub async fn workspace_revision(&self) -> ServiceResult<u64> {
        let row = sqlx::query("SELECT revision FROM workspace_revision WHERE singleton = 1")
            .fetch_one(&self.pool)
            .await
            .map_err(storage_error)?;
        row.try_get::<i64, _>("revision")
            .map(|v| v as u64)
            .map_err(storage_error)
    }

    pub async fn workspace(&self) -> ServiceResult<WorkspaceView> {
        let consistent = self.consistent_workspace().await?;
        let snapshot = consistent.snapshot;
        let revisions = consistent.entity_revisions;
        Ok(WorkspaceView {
            revision: consistent.revision,
            tasks: snapshot
                .tasks
                .into_iter()
                .map(|v| versioned("task", v.id.clone(), task_view(v), &revisions))
                .collect::<ServiceResult<_>>()?,
            people: snapshot
                .people
                .into_iter()
                .map(|v| {
                    versioned(
                        "person",
                        v.id.clone(),
                        PersonView {
                            id: v.id,
                            name: v.name,
                            email: v.email,
                            active: v.active,
                        },
                        &revisions,
                    )
                })
                .collect::<ServiceResult<_>>()?,
            projects: snapshot
                .projects
                .into_iter()
                .map(|v| {
                    versioned(
                        "project",
                        v.id.clone(),
                        ProjectView {
                            id: v.id,
                            key: v.key,
                            name: v.name,
                            description: v.description,
                            lead_person_id: v.lead_person_id,
                        },
                        &revisions,
                    )
                })
                .collect::<ServiceResult<_>>()?,
            tags: snapshot
                .tags
                .into_iter()
                .map(|v| {
                    versioned(
                        "tag",
                        v.id.clone(),
                        TagView {
                            id: v.id,
                            label: v.label,
                        },
                        &revisions,
                    )
                })
                .collect::<ServiceResult<_>>()?,
        })
    }

    pub async fn filtered_workspace(
        &self,
        filter: WorkspaceFilter,
    ) -> ServiceResult<WorkspaceView> {
        let mut workspace = self.workspace().await?;
        validate_workspace_filter(&filter, &workspace)?;
        workspace
            .tasks
            .retain(|task| task_matches_workspace_filter(&task.value, &filter));
        Ok(workspace)
    }

    pub async fn revision_map(&self) -> ServiceResult<HashMap<String, u64>> {
        self.revisions().await
    }
    pub(crate) async fn domain_snapshot(&self) -> ServiceResult<WorkspaceSnapshot> {
        StorageView {
            pool: self.pool.clone(),
            dialect: self.dialect,
        }
        .load()
        .await
    }

    pub(crate) async fn consistent_workspace(&self) -> ServiceResult<ConsistentWorkspace> {
        const MAX_ATTEMPTS: usize = 8;
        for _ in 0..MAX_ATTEMPTS {
            let before = self.workspace_revision().await?;
            let snapshot = self.domain_snapshot().await?;
            let entity_revisions = self.revisions().await?;
            let revision = self.workspace_revision().await?;
            if before == revision {
                return Ok(ConsistentWorkspace {
                    snapshot,
                    revision,
                    entity_revisions,
                });
            }
            tokio::task::yield_now().await;
        }
        Err(ServiceError::Storage(
            "workspace kept changing while loading a consistent snapshot".into(),
        ))
    }

    async fn revisions(&self) -> ServiceResult<HashMap<String, u64>> {
        let mut result = HashMap::new();
        for (kind, table) in [
            ("task", "tasks"),
            ("person", "people"),
            ("project", "projects"),
            ("tag", "tags"),
        ] {
            let sql = format!("SELECT id, revision FROM {table}");
            for row in sqlx::query(AssertSqlSafe(sql.as_str()))
                .fetch_all(&self.pool)
                .await
                .map_err(storage_error)?
            {
                result.insert(
                    format!(
                        "{kind}:{}",
                        row.try_get::<String, _>("id").map_err(storage_error)?
                    ),
                    row.try_get::<i64, _>("revision").map_err(storage_error)? as u64,
                );
            }
        }
        Ok(result)
    }

    pub(crate) async fn create_task_entity(
        &self,
        task: Task,
    ) -> ServiceResult<Versioned<TaskView>> {
        if task.title.trim().is_empty() {
            return Err(ServiceError::Invalid("task title is required".into()));
        }
        validate_task_temporal_fields(
            task.state,
            task.start_date.as_deref(),
            task.due_date.as_deref(),
            task.snoozed_until.is_some(),
        )?;
        let sql = format!(
            "INSERT INTO tasks (id, title, state, workflow_state, rejected, size, priority, start_date, due_date, snoozed_until, detail, created_at, updated_at) VALUES ({}, {}, 'next', {}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2),
            self.dialect.placeholder(3),
            self.dialect.placeholder(4),
            self.dialect.placeholder(5),
            self.dialect.placeholder(6),
            self.dialect.placeholder(7),
            self.dialect.placeholder(8),
            self.dialect.placeholder(9),
            self.dialect.placeholder(10),
            self.dialect.placeholder(11),
            self.dialect.placeholder(12)
        );
        let now = now_text();
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query(AssertSqlSafe(sql.as_str()))
            .bind(&task.id)
            .bind(&task.title)
            .bind(storage_state(task.state.id()))
            .bind(task.state == crate::domain::TaskState::Rejected)
            .bind(task.size.id())
            .bind(task.priority.id())
            .bind(&task.start_date)
            .bind(&task.due_date)
            .bind(task.snoozed_until.map(crate::snooze::format_datetime))
            .bind(&task.detail)
            .bind(&now)
            .bind(&now)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        self.replace_links(
            &mut tx,
            "task_people",
            "person_id",
            &task.id,
            &task.people_ids,
        )
        .await?;
        self.replace_links(
            &mut tx,
            "task_projects",
            "project_id",
            &task.id,
            &task.project_ids,
        )
        .await?;
        self.replace_links(&mut tx, "task_tags", "tag_id", &task.id, &task.tag_ids)
            .await?;
        bump_workspace(&mut tx, self.dialect).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Versioned {
            revision: 1,
            value: task_view(task),
        })
    }

    pub async fn create_task(&self, input: TaskCreate) -> ServiceResult<Versioned<TaskView>> {
        let size = crate::domain::TaskSize::parse(&input.size)
            .ok_or_else(|| ServiceError::Invalid("size must be small, medium, or big".into()))?;
        let state = crate::domain::TaskState::parse(&input.state)
            .ok_or_else(|| ServiceError::Invalid("invalid task state".into()))?;
        let priority = crate::domain::TaskPriority::parse(&input.priority)
            .ok_or_else(|| ServiceError::Invalid("invalid task priority".into()))?;
        let snoozed_until = input
            .snoozed_until
            .as_deref()
            .map(crate::snooze::parse_datetime)
            .transpose()
            .map_err(|e| ServiceError::Invalid(e.to_string()))?;
        self.create_task_entity(Task {
            id: Uuid::new_v4().to_string(),
            title: input.title.trim().into(),
            state,
            size,
            priority,
            start_date: input.start_date,
            due_date: input.due_date,
            snoozed_until,
            people_ids: input.people_ids,
            project_ids: input.project_ids,
            tag_ids: input.tag_ids,
            detail: input.detail,
        })
        .await
    }

    pub async fn update_task(&self, input: TaskUpdate) -> ServiceResult<Versioned<TaskView>> {
        validate_task_update(&input)?;
        let snoozed_until = input
            .snoozed_until
            .as_deref()
            .map(crate::snooze::parse_datetime)
            .transpose()
            .map_err(|error| ServiceError::Invalid(error.to_string()))?
            .map(crate::snooze::format_datetime);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.claim(&mut tx, "tasks", "task", &input.id, input.expected_revision)
            .await?;
        let sql = format!(
            "UPDATE tasks SET title = {}, workflow_state = {}, rejected = {}, size = {}, priority = {}, start_date = {}, due_date = {}, snoozed_until = {}, detail = {}, updated_at = {} WHERE id = {}",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2),
            self.dialect.placeholder(3),
            self.dialect.placeholder(4),
            self.dialect.placeholder(5),
            self.dialect.placeholder(6),
            self.dialect.placeholder(7),
            self.dialect.placeholder(8),
            self.dialect.placeholder(9),
            self.dialect.placeholder(10),
            self.dialect.placeholder(11)
        );
        sqlx::query(AssertSqlSafe(sql.as_str()))
            .bind(input.title.trim())
            .bind(storage_state(&input.state))
            .bind(input.state == "rejected")
            .bind(&input.size)
            .bind(&input.priority)
            .bind(&input.start_date)
            .bind(&input.due_date)
            .bind(&snoozed_until)
            .bind(&input.detail)
            .bind(now_text())
            .bind(&input.id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        self.replace_links(
            &mut tx,
            "task_people",
            "person_id",
            &input.id,
            &input.people_ids,
        )
        .await?;
        self.replace_links(
            &mut tx,
            "task_projects",
            "project_id",
            &input.id,
            &input.project_ids,
        )
        .await?;
        self.replace_links(&mut tx, "task_tags", "tag_id", &input.id, &input.tag_ids)
            .await?;
        bump_workspace(&mut tx, self.dialect).await?;
        tx.commit().await.map_err(storage_error)?;
        self.get_task(&input.id).await
    }

    pub(crate) async fn patch_task(
        &self,
        id: String,
        expected: u64,
        patch: TaskPatch,
    ) -> ServiceResult<TaskPatchResult> {
        validate_task_patch(&patch)?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.claim(&mut tx, "tasks", "task", &id, expected).await?;
        let mut related_revisions = HashMap::new();
        match patch {
            TaskPatch::People(ids) => {
                self.replace_links(&mut tx, "task_people", "person_id", &id, &ids)
                    .await?
            }
            TaskPatch::Projects(ids) => {
                self.replace_links(&mut tx, "task_projects", "project_id", &id, &ids)
                    .await?
            }
            TaskPatch::Tags(tags) => {
                let mut ids = Vec::new();
                for tag in tags {
                    validate_required("tag label", &tag.label)?;
                    let insert = format!(
                        "INSERT INTO tags (id, label) VALUES ({}, {}) ON CONFLICT(label) DO NOTHING",
                        self.dialect.placeholder(1),
                        self.dialect.placeholder(2)
                    );
                    sqlx::query(AssertSqlSafe(insert.as_str()))
                        .bind(&tag.id)
                        .bind(tag.label.trim())
                        .execute(&mut *tx)
                        .await
                        .map_err(storage_error)?;
                    let select = format!(
                        "SELECT id, revision FROM tags WHERE label = {}",
                        self.dialect.placeholder(1)
                    );
                    let row = sqlx::query(AssertSqlSafe(select.as_str()))
                        .bind(tag.label.trim())
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(storage_error)?;
                    let tag_id = row.try_get::<String, _>("id").map_err(storage_error)?;
                    related_revisions.insert(
                        format!("tag:{tag_id}"),
                        row.try_get::<i64, _>("revision").map_err(storage_error)? as u64,
                    );
                    ids.push(tag_id);
                }
                self.replace_links(&mut tx, "task_tags", "tag_id", &id, &ids)
                    .await?;
            }
            patch => apply_task_patch(&mut tx, self.dialect, &id, patch).await?,
        }
        let touch = format!(
            "UPDATE tasks SET updated_at = {} WHERE id = {}",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2)
        );
        sqlx::query(AssertSqlSafe(touch.as_str()))
            .bind(now_text())
            .bind(&id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        bump_workspace(&mut tx, self.dialect).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(TaskPatchResult {
            revision: expected + 1,
            related_revisions,
        })
    }

    pub async fn delete_task(&self, id: &str, expected: u64) -> ServiceResult<()> {
        self.delete("tasks", "task", id, expected).await
    }
    pub async fn get_task(&self, id: &str) -> ServiceResult<Versioned<TaskView>> {
        self.workspace()
            .await?
            .tasks
            .into_iter()
            .find(|v| v.value.id == id)
            .ok_or_else(|| ServiceError::NotFound {
                entity: "task",
                id: id.into(),
            })
    }

    pub async fn create_person(&self, input: PersonInput) -> ServiceResult<Versioned<PersonView>> {
        let person = Person {
            id: Uuid::new_v4().to_string(),
            name: input.name.trim().into(),
            email: input.email.trim().into(),
            active: input.active,
        };
        self.create_person_entity(person).await
    }
    pub(crate) async fn create_person_entity(
        &self,
        person: Person,
    ) -> ServiceResult<Versioned<PersonView>> {
        if person.name.is_empty() {
            return Err(ServiceError::Invalid("person name is required".into()));
        }
        let sql = format!(
            "INSERT INTO people (id, name, email, active) VALUES ({}, {}, {}, {})",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2),
            self.dialect.placeholder(3),
            self.dialect.placeholder(4)
        );
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query(AssertSqlSafe(sql.as_str()))
            .bind(&person.id)
            .bind(&person.name)
            .bind(&person.email)
            .bind(person.active)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        bump_workspace(&mut tx, self.dialect).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Versioned {
            revision: 1,
            value: PersonView {
                id: person.id,
                name: person.name,
                email: person.email,
                active: person.active,
            },
        })
    }
    pub async fn update_person(
        &self,
        id: &str,
        expected: u64,
        input: PersonInput,
    ) -> ServiceResult<Versioned<PersonView>> {
        validate_required("person name", &input.name)?;
        self.update_simple(
            "people",
            "person",
            id,
            expected,
            &[
                ("name", Value::Text(input.name.trim().into())),
                ("email", Value::Text(input.email.trim().into())),
                ("active", Value::Bool(input.active)),
            ],
        )
        .await?;
        self.workspace()
            .await?
            .people
            .into_iter()
            .find(|v| v.value.id == id)
            .ok_or_else(|| ServiceError::NotFound {
                entity: "person",
                id: id.into(),
            })
    }
    pub(crate) async fn patch_person(
        &self,
        id: String,
        expected: u64,
        patch: PersonPatch,
    ) -> ServiceResult<u64> {
        if let PersonPatch::Name(value) = &patch {
            validate_required("person name", value)?;
        }
        let input = match patch {
            PersonPatch::Name(v) => ("name", Value::Text(v.trim().into())),
            PersonPatch::Email(v) => ("email", Value::Text(v.trim().into())),
            PersonPatch::Active(v) => ("active", Value::Bool(v)),
        };
        self.update_simple("people", "person", &id, expected, &[input])
            .await?;
        Ok(expected + 1)
    }
    pub async fn delete_person(&self, id: &str, expected: u64) -> ServiceResult<()> {
        self.delete("people", "person", id, expected).await
    }

    pub async fn create_project(
        &self,
        input: ProjectInput,
    ) -> ServiceResult<Versioned<ProjectView>> {
        let mut project = Project::new(
            Uuid::new_v4().to_string(),
            input.key,
            input.name,
            input.description,
        );
        project.lead_person_id = input.lead_person_id;
        self.create_project_entity(project).await
    }
    pub(crate) async fn create_project_entity(
        &self,
        project: Project,
    ) -> ServiceResult<Versioned<ProjectView>> {
        if project.key.is_empty() || project.name.is_empty() {
            return Err(ServiceError::Invalid(
                "project key and name are required".into(),
            ));
        }
        let sql = format!(
            "INSERT INTO projects (id, key, name, description, lead_person_id) VALUES ({}, {}, {}, {}, {})",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2),
            self.dialect.placeholder(3),
            self.dialect.placeholder(4),
            self.dialect.placeholder(5)
        );
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query(AssertSqlSafe(sql.as_str()))
            .bind(&project.id)
            .bind(&project.key)
            .bind(&project.name)
            .bind(&project.description)
            .bind(&project.lead_person_id)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        bump_workspace(&mut tx, self.dialect).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Versioned {
            revision: 1,
            value: ProjectView {
                id: project.id,
                key: project.key,
                name: project.name,
                description: project.description,
                lead_person_id: project.lead_person_id,
            },
        })
    }
    pub async fn update_project(
        &self,
        id: &str,
        expected: u64,
        input: ProjectInput,
    ) -> ServiceResult<Versioned<ProjectView>> {
        validate_required("project key", &input.key)?;
        validate_required("project name", &input.name)?;
        self.update_simple(
            "projects",
            "project",
            id,
            expected,
            &[
                ("key", Value::Text(input.key.trim().into())),
                ("name", Value::Text(input.name.trim().into())),
                ("description", Value::Text(input.description)),
                ("lead_person_id", Value::Optional(input.lead_person_id)),
            ],
        )
        .await?;
        self.workspace()
            .await?
            .projects
            .into_iter()
            .find(|v| v.value.id == id)
            .ok_or_else(|| ServiceError::NotFound {
                entity: "project",
                id: id.into(),
            })
    }
    pub(crate) async fn patch_project(
        &self,
        id: String,
        expected: u64,
        patch: ProjectPatch,
    ) -> ServiceResult<u64> {
        match &patch {
            ProjectPatch::Key(value) => validate_required("project key", value)?,
            ProjectPatch::Name(value) => validate_required("project name", value)?,
            ProjectPatch::Description(_) | ProjectPatch::LeadPerson(_) => {}
        }
        let input = match patch {
            ProjectPatch::Key(v) => ("key", Value::Text(v.trim().into())),
            ProjectPatch::Name(v) => ("name", Value::Text(v.trim().into())),
            ProjectPatch::Description(v) => ("description", Value::Text(v)),
            ProjectPatch::LeadPerson(v) => ("lead_person_id", Value::Optional(v)),
        };
        self.update_simple("projects", "project", &id, expected, &[input])
            .await?;
        Ok(expected + 1)
    }
    pub async fn delete_project(&self, id: &str, expected: u64) -> ServiceResult<()> {
        self.delete("projects", "project", id, expected).await
    }

    pub async fn create_tag(&self, input: TagInput) -> ServiceResult<Versioned<TagView>> {
        self.create_tag_entity(Tag::new(Uuid::new_v4().to_string(), input.label))
            .await
    }
    pub(crate) async fn create_tag_entity(&self, tag: Tag) -> ServiceResult<Versioned<TagView>> {
        if tag.label.is_empty() {
            return Err(ServiceError::Invalid("tag label is required".into()));
        }
        let sql = format!(
            "INSERT INTO tags (id, label) VALUES ({}, {})",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2)
        );
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query(AssertSqlSafe(sql.as_str()))
            .bind(&tag.id)
            .bind(&tag.label)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        bump_workspace(&mut tx, self.dialect).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(Versioned {
            revision: 1,
            value: TagView {
                id: tag.id,
                label: tag.label,
            },
        })
    }
    pub async fn update_tag(
        &self,
        id: &str,
        expected: u64,
        input: TagInput,
    ) -> ServiceResult<Versioned<TagView>> {
        validate_required("tag label", &input.label)?;
        self.update_simple(
            "tags",
            "tag",
            id,
            expected,
            &[("label", Value::Text(input.label.trim().into()))],
        )
        .await?;
        self.workspace()
            .await?
            .tags
            .into_iter()
            .find(|v| v.value.id == id)
            .ok_or_else(|| ServiceError::NotFound {
                entity: "tag",
                id: id.into(),
            })
    }
    pub(crate) async fn patch_tag(
        &self,
        id: String,
        expected: u64,
        patch: TagPatch,
    ) -> ServiceResult<u64> {
        let TagPatch::Label(v) = patch;
        validate_required("tag label", &v)?;
        self.update_simple(
            "tags",
            "tag",
            &id,
            expected,
            &[("label", Value::Text(v.trim().into()))],
        )
        .await?;
        Ok(expected + 1)
    }
    pub async fn delete_tag(&self, id: &str, expected: u64) -> ServiceResult<()> {
        self.delete("tags", "tag", id, expected).await
    }

    async fn claim(
        &self,
        tx: &mut Transaction<'_, Any>,
        table: &'static str,
        entity: &'static str,
        id: &str,
        expected: u64,
    ) -> ServiceResult<()> {
        let sql = format!(
            "UPDATE {table} SET revision = revision + 1 WHERE id = {} AND revision = {}",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2)
        );
        let rows = sqlx::query(AssertSqlSafe(sql.as_str()))
            .bind(id)
            .bind(expected as i64)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?
            .rows_affected();
        if rows == 1 {
            return Ok(());
        }
        let actual = self.actual_revision(tx, table, id).await?;
        Err(if actual.is_none() {
            ServiceError::NotFound {
                entity,
                id: id.into(),
            }
        } else {
            ServiceError::Conflict {
                entity,
                id: id.into(),
                expected,
                actual,
            }
        })
    }
    async fn actual_revision(
        &self,
        tx: &mut Transaction<'_, Any>,
        table: &str,
        id: &str,
    ) -> ServiceResult<Option<u64>> {
        let sql = format!(
            "SELECT revision FROM {table} WHERE id = {}",
            self.dialect.placeholder(1)
        );
        Ok(sqlx::query(AssertSqlSafe(sql.as_str()))
            .bind(id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage_error)?
            .map(|r| r.get::<i64, _>("revision") as u64))
    }
    async fn replace_links(
        &self,
        tx: &mut Transaction<'_, Any>,
        table: &str,
        column: &str,
        task_id: &str,
        ids: &[String],
    ) -> ServiceResult<()> {
        let del = format!(
            "DELETE FROM {table} WHERE task_id = {}",
            self.dialect.placeholder(1)
        );
        sqlx::query(AssertSqlSafe(del.as_str()))
            .bind(task_id)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
        let ins = format!(
            "INSERT INTO {table} (task_id, {column}, sort_order) VALUES ({}, {}, {})",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2),
            self.dialect.placeholder(3)
        );
        for (index, id) in ids.iter().enumerate() {
            sqlx::query(AssertSqlSafe(ins.as_str()))
                .bind(task_id)
                .bind(id)
                .bind(index as i64)
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?;
        }
        Ok(())
    }
    async fn delete(
        &self,
        table: &'static str,
        entity: &'static str,
        id: &str,
        expected: u64,
    ) -> ServiceResult<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.claim(&mut tx, table, entity, id, expected).await?;
        self.bump_cascade_revisions(&mut tx, table, id).await?;
        let cleanup = match table {
            "people" => vec![
                "DELETE FROM task_people WHERE person_id = ",
                "UPDATE projects SET lead_person_id = NULL WHERE lead_person_id = ",
            ],
            "projects" => vec!["DELETE FROM task_projects WHERE project_id = "],
            "tags" => vec!["DELETE FROM task_tags WHERE tag_id = "],
            _ => Vec::new(),
        };
        for prefix in cleanup {
            let sql = format!("{prefix}{}", self.dialect.placeholder(1));
            sqlx::query(AssertSqlSafe(sql.as_str()))
                .bind(id)
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
        }
        let sql = format!(
            "DELETE FROM {table} WHERE id = {} AND revision = {}",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2)
        );
        let rows = sqlx::query(AssertSqlSafe(sql.as_str()))
            .bind(id)
            .bind((expected + 1) as i64)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?
            .rows_affected();
        if rows != 1 {
            return Err(ServiceError::Conflict {
                entity,
                id: id.into(),
                expected,
                actual: self.actual_revision(&mut tx, table, id).await?,
            });
        }
        bump_workspace(&mut tx, self.dialect).await?;
        tx.commit().await.map_err(storage_error)
    }

    async fn bump_cascade_revisions(
        &self,
        tx: &mut Transaction<'_, Any>,
        table: &str,
        id: &str,
    ) -> ServiceResult<()> {
        let statements = match table {
            "people" => vec![
                "UPDATE tasks SET revision = revision + 1 WHERE id IN (SELECT task_id FROM task_people WHERE person_id = ",
                "UPDATE projects SET revision = revision + 1 WHERE lead_person_id = ",
            ],
            "projects" => vec![
                "UPDATE tasks SET revision = revision + 1 WHERE id IN (SELECT task_id FROM task_projects WHERE project_id = ",
            ],
            "tags" => vec![
                "UPDATE tasks SET revision = revision + 1 WHERE id IN (SELECT task_id FROM task_tags WHERE tag_id = ",
            ],
            _ => Vec::new(),
        };
        for prefix in statements {
            let suffix = if prefix.contains(" IN (") { ")" } else { "" };
            let sql = format!("{prefix}{}{suffix}", self.dialect.placeholder(1));
            sqlx::query(AssertSqlSafe(sql.as_str()))
                .bind(id)
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?;
        }
        Ok(())
    }
    async fn update_simple(
        &self,
        table: &'static str,
        entity: &'static str,
        id: &str,
        expected: u64,
        values: &[(&str, Value)],
    ) -> ServiceResult<()> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        self.claim(&mut tx, table, entity, id, expected).await?;
        for (column, value) in values {
            let sql = format!(
                "UPDATE {table} SET {column} = {} WHERE id = {}",
                self.dialect.placeholder(1),
                self.dialect.placeholder(2)
            );
            let query = sqlx::query(AssertSqlSafe(sql.as_str()));
            match value {
                Value::Text(v) => query.bind(v).bind(id).execute(&mut *tx).await,
                Value::Bool(v) => query.bind(v).bind(id).execute(&mut *tx).await,
                Value::Optional(v) => query.bind(v).bind(id).execute(&mut *tx).await,
            }
            .map_err(storage_error)?;
        }
        bump_workspace(&mut tx, self.dialect).await?;
        tx.commit().await.map_err(storage_error)
    }
}

enum Value {
    Text(String),
    Bool(bool),
    Optional(Option<String>),
}
struct StorageView {
    pool: AnyPool,
    dialect: SqlDialect,
}
impl StorageView {
    async fn load(&self) -> ServiceResult<WorkspaceSnapshot> {
        storage::load_workspace_for_service(&self.pool, self.dialect)
            .await
            .map_err(storage_error)
    }
}

fn versioned<T>(
    entity: &'static str,
    id: String,
    value: T,
    revisions: &HashMap<String, u64>,
) -> ServiceResult<Versioned<T>> {
    revisions
        .get(&format!("{entity}:{id}"))
        .copied()
        .map(|revision| Versioned { revision, value })
        .ok_or(ServiceError::NotFound { entity, id })
}
fn task_view(v: Task) -> TaskView {
    TaskView {
        id: v.id,
        title: v.title,
        state: v.state.id().into(),
        size: v.size.id().into(),
        priority: v.priority.id().into(),
        start_date: v.start_date,
        due_date: v.due_date,
        snoozed_until: v.snoozed_until.map(crate::snooze::format_datetime),
        people_ids: v.people_ids,
        project_ids: v.project_ids,
        tag_ids: v.tag_ids,
        detail: v.detail,
    }
}

fn storage_state(v: &str) -> &str {
    if v == "rejected" { "snoozed" } else { v }
}
fn now_text() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string()
}
fn storage_error(error: impl fmt::Display) -> ServiceError {
    ServiceError::Storage(error.to_string())
}
async fn bump_workspace(tx: &mut Transaction<'_, Any>, dialect: SqlDialect) -> ServiceResult<()> {
    sqlx::query("UPDATE workspace_revision SET revision = revision + 1 WHERE singleton = 1")
        .execute(&mut **tx)
        .await
        .map_err(storage_error)?;
    if dialect == SqlDialect::Postgres {
        sqlx::query("NOTIFY tuido_changes")
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    Ok(())
}

async fn apply_task_patch(
    tx: &mut Transaction<'_, Any>,
    dialect: SqlDialect,
    id: &str,
    patch: TaskPatch,
) -> ServiceResult<()> {
    let remembered_custom = match &patch {
        TaskPatch::Snooze {
            remember_custom, ..
        } => *remember_custom,
        _ => None,
    };
    let (columns, values): (Vec<&str>, Vec<Value>) = match patch {
        TaskPatch::Title(v) => (vec!["title"], vec![Value::Text(v.trim().into())]),
        TaskPatch::Detail(v) => (vec!["detail"], vec![Value::Text(v)]),
        TaskPatch::State(v) => (
            vec!["workflow_state", "rejected", "snoozed_until"],
            vec![
                Value::Text(if v == crate::domain::TaskState::Rejected {
                    "snoozed".into()
                } else {
                    v.id().into()
                }),
                Value::Bool(v == crate::domain::TaskState::Rejected),
                Value::Optional(None),
            ],
        ),
        TaskPatch::Size(v) => (vec!["size"], vec![Value::Text(v.id().into())]),
        TaskPatch::Priority(v) => (vec!["priority"], vec![Value::Text(v.id().into())]),
        TaskPatch::StartDate(v) => (vec!["start_date"], vec![Value::Optional(v)]),
        TaskPatch::EndDate(v) => (vec!["due_date"], vec![Value::Optional(v)]),
        TaskPatch::Snooze { until, .. } => (
            vec!["workflow_state", "rejected", "snoozed_until"],
            vec![
                Value::Text("snoozed".into()),
                Value::Bool(false),
                Value::Text(crate::snooze::format_datetime(until)),
            ],
        ),
        TaskPatch::Unsnooze => (
            vec!["workflow_state", "rejected", "snoozed_until"],
            vec![
                Value::Text("todo".into()),
                Value::Bool(false),
                Value::Optional(None),
            ],
        ),
        TaskPatch::People(_) | TaskPatch::Projects(_) | TaskPatch::Tags(_) => {
            return Err(ServiceError::Invalid(
                "relation patch requires full task update".into(),
            ));
        }
    };
    for (column, value) in columns.into_iter().zip(values) {
        let sql = format!(
            "UPDATE tasks SET {column} = {} WHERE id = {}",
            dialect.placeholder(1),
            dialect.placeholder(2)
        );
        let q = sqlx::query(AssertSqlSafe(sql.as_str()));
        match value {
            Value::Text(v) => q.bind(v).bind(id).execute(&mut **tx).await,
            Value::Bool(v) => q.bind(v).bind(id).execute(&mut **tx).await,
            Value::Optional(v) => q.bind(v).bind(id).execute(&mut **tx).await,
        }
        .map_err(storage_error)?;
    }
    if let Some(custom) = remembered_custom {
        let sql = format!(
            "INSERT INTO settings (key, value) VALUES ('last_custom_snooze', {}) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            dialect.placeholder(1)
        );
        sqlx::query(AssertSqlSafe(sql.as_str()))
            .bind(crate::snooze::format_datetime(custom))
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{TaskSize, TaskState};
    use sqlx::any::AnyPoolOptions;

    async fn test_service() -> TuidoService {
        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        TuidoService::from_parts(pool, SqlDialect::Sqlite)
    }

    #[test]
    fn filtered_workspace_filters_tasks_and_returns_complete_entity_catalogs() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let service = test_service().await;
            let person = service
                .create_person(PersonInput {
                    name: "Marlo".into(),
                    email: "marlo@example.com".into(),
                    active: true,
                })
                .await
                .unwrap();
            service
                .create_person(PersonInput {
                    name: "Unrelated".into(),
                    email: String::new(),
                    active: true,
                })
                .await
                .unwrap();
            let project = service
                .create_project(ProjectInput {
                    key: "LAUNCH".into(),
                    name: "Launch".into(),
                    description: String::new(),
                    lead_person_id: Some(person.value.id.clone()),
                })
                .await
                .unwrap();
            service
                .create_project(ProjectInput {
                    key: "OTHER".into(),
                    name: "Unrelated".into(),
                    description: String::new(),
                    lead_person_id: None,
                })
                .await
                .unwrap();
            let tag = service
                .create_tag(TagInput { label: "UI".into() })
                .await
                .unwrap();
            service
                .create_tag(TagInput {
                    label: "Unrelated".into(),
                })
                .await
                .unwrap();
            service
                .create_task(TaskCreate {
                    title: "Keep selection stable".into(),
                    detail: "Select task after refresh".into(),
                    size: "small".into(),
                    state: "todo".into(),
                    priority: "high".into(),
                    start_date: None,
                    due_date: Some("2026-07-30".into()),
                    snoozed_until: None,
                    people_ids: Vec::new(),
                    project_ids: vec![project.value.id.clone()],
                    tag_ids: vec![tag.value.id.clone()],
                })
                .await
                .unwrap();
            service
                .create_task(TaskCreate {
                    title: "Resolved task".into(),
                    detail: String::new(),
                    size: "medium".into(),
                    state: "done".into(),
                    priority: "medium".into(),
                    start_date: None,
                    due_date: None,
                    snoozed_until: None,
                    people_ids: Vec::new(),
                    project_ids: Vec::new(),
                    tag_ids: Vec::new(),
                })
                .await
                .unwrap();

            let workspace = service
                .filtered_workspace(WorkspaceFilter {
                    states: vec!["todo".into()],
                    priorities: vec!["high".into()],
                    sizes: vec!["small".into()],
                    project_ids: vec![project.value.id.clone()],
                    tag_ids: vec![tag.value.id.clone()],
                    due_before: Some("2026-08-01".into()),
                    due_after: Some("2026-07-01".into()),
                    query: Some("selection".into()),
                    ..WorkspaceFilter::default()
                })
                .await
                .unwrap();

            assert_eq!(workspace.tasks.len(), 1);
            assert_eq!(workspace.people.len(), 2);
            assert!(
                workspace
                    .people
                    .iter()
                    .any(|candidate| candidate.value.id == person.value.id)
            );
            assert_eq!(workspace.projects.len(), 2);
            assert_eq!(workspace.tags.len(), 2);

            let with_resolved = service
                .filtered_workspace(WorkspaceFilter {
                    include_resolved: true,
                    ..WorkspaceFilter::default()
                })
                .await
                .unwrap();
            assert_eq!(with_resolved.tasks.len(), 2);
        });
    }

    #[test]
    fn filtered_workspace_rejects_resolved_states_and_unknown_relations_by_default() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let service = test_service().await;

            let resolved = service
                .filtered_workspace(WorkspaceFilter {
                    states: vec!["done".into()],
                    ..WorkspaceFilter::default()
                })
                .await;
            assert!(matches!(resolved, Err(ServiceError::Invalid(_))));

            let unknown = service
                .filtered_workspace(WorkspaceFilter {
                    tag_ids: vec!["missing".into()],
                    ..WorkspaceFilter::default()
                })
                .await;
            assert!(matches!(unknown, Err(ServiceError::Invalid(_))));
        });
    }

    #[test]
    fn public_task_inputs_reject_legacy_state_aliases() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let service = test_service().await;

            for alias in ["clarify", "next", "waiting", "doing"] {
                let create = service
                    .create_task(TaskCreate {
                        title: format!("Legacy {alias}"),
                        detail: String::new(),
                        size: "small".into(),
                        state: alias.into(),
                        priority: "medium".into(),
                        start_date: None,
                        due_date: None,
                        snoozed_until: None,
                        people_ids: Vec::new(),
                        project_ids: Vec::new(),
                        tag_ids: Vec::new(),
                    })
                    .await;
                assert!(matches!(create, Err(ServiceError::Invalid(_))));

                let filter = service
                    .filtered_workspace(WorkspaceFilter {
                        states: vec![alias.into()],
                        ..WorkspaceFilter::default()
                    })
                    .await;
                assert!(matches!(filter, Err(ServiceError::Invalid(_))));
            }

            let task = service
                .create_task(TaskCreate {
                    title: "Canonical".into(),
                    detail: String::new(),
                    size: "small".into(),
                    state: "todo".into(),
                    priority: "medium".into(),
                    start_date: None,
                    due_date: None,
                    snoozed_until: None,
                    people_ids: Vec::new(),
                    project_ids: Vec::new(),
                    tag_ids: Vec::new(),
                })
                .await
                .unwrap();
            let update = service
                .update_task(TaskUpdate {
                    id: task.value.id,
                    expected_revision: task.revision,
                    title: "Legacy update".into(),
                    state: "doing".into(),
                    size: "small".into(),
                    priority: "medium".into(),
                    start_date: None,
                    due_date: None,
                    snoozed_until: None,
                    people_ids: Vec::new(),
                    project_ids: Vec::new(),
                    tag_ids: Vec::new(),
                    detail: String::new(),
                })
                .await;
            assert!(matches!(update, Err(ServiceError::Invalid(_))));
        });
    }

    #[test]
    fn stale_task_write_returns_typed_conflict_and_preserves_winner() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            sqlx::any::install_default_drivers();
            let pool = AnyPoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();
            sqlx::migrate!().run(&pool).await.unwrap();
            let service = TuidoService::from_parts(pool, SqlDialect::Sqlite);
            let task = Task::quick_capture(
                "task".into(),
                "Original".into(),
                String::new(),
                TaskSize::Small,
            );
            service.create_task_entity(task).await.unwrap();

            service
                .patch_task("task".into(), 1, TaskPatch::State(TaskState::Done))
                .await
                .unwrap();
            let error = service
                .patch_task("task".into(), 1, TaskPatch::State(TaskState::Rejected))
                .await
                .unwrap_err();

            assert!(matches!(
                error,
                ServiceError::Conflict {
                    expected: 1,
                    actual: Some(2),
                    ..
                }
            ));
            assert_eq!(service.get_task("task").await.unwrap().value.state, "done");
            assert_eq!(service.workspace_revision().await.unwrap(), 3);
        });
    }

    #[test]
    fn failed_relation_replacement_rolls_back_entity_and_workspace_revisions() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            sqlx::any::install_default_drivers();
            let pool = AnyPoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();
            sqlx::migrate!().run(&pool).await.unwrap();
            sqlx::query("PRAGMA foreign_keys = ON")
                .execute(&pool)
                .await
                .unwrap();
            let service = TuidoService::from_parts(pool, SqlDialect::Sqlite);
            service
                .create_task_entity(Task::quick_capture(
                    "task".into(),
                    "Task".into(),
                    String::new(),
                    TaskSize::Small,
                ))
                .await
                .unwrap();

            assert!(
                service
                    .patch_task("task".into(), 1, TaskPatch::People(vec!["missing".into()]))
                    .await
                    .is_err()
            );

            assert_eq!(service.get_task("task").await.unwrap().revision, 1);
            assert_eq!(service.workspace_revision().await.unwrap(), 2);
        });
    }

    #[test]
    fn malformed_task_snooze_update_is_rejected_without_poisoning_workspace() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let service = test_service().await;
            service
                .create_task_entity(Task::quick_capture(
                    "task".into(),
                    "Original".into(),
                    String::new(),
                    TaskSize::Small,
                ))
                .await
                .unwrap();

            let error = service
                .update_task(TaskUpdate {
                    id: "task".into(),
                    expected_revision: 1,
                    title: "Changed".into(),
                    state: "todo".into(),
                    size: "small".into(),
                    priority: "medium".into(),
                    start_date: None,
                    due_date: None,
                    snoozed_until: Some("not-a-date".into()),
                    people_ids: Vec::new(),
                    project_ids: Vec::new(),
                    tag_ids: Vec::new(),
                    detail: String::new(),
                })
                .await
                .unwrap_err();

            assert!(matches!(error, ServiceError::Invalid(_)));
            let task = service.get_task("task").await.unwrap();
            assert_eq!(task.revision, 1);
            assert_eq!(task.value.title, "Original");
            assert!(service.workspace().await.is_ok());
        });
    }

    #[test]
    fn task_temporal_fields_and_snooze_invariants_are_enforced() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let service = test_service().await;
            let mut invalid_date = Task::quick_capture(
                "invalid-date".into(),
                "Invalid date".into(),
                String::new(),
                TaskSize::Small,
            );
            invalid_date.start_date = Some("2026-02-30".into());
            assert!(matches!(
                service.create_task_entity(invalid_date).await,
                Err(ServiceError::Invalid(_))
            ));

            let mut missing_until = Task::quick_capture(
                "missing-until".into(),
                "Missing until".into(),
                String::new(),
                TaskSize::Small,
            );
            missing_until.state = TaskState::Snoozed;
            assert!(matches!(
                service.create_task_entity(missing_until).await,
                Err(ServiceError::Invalid(_))
            ));

            let mut stale_until = Task::quick_capture(
                "stale-until".into(),
                "Stale until".into(),
                String::new(),
                TaskSize::Small,
            );
            stale_until.snoozed_until = Some(time::macros::datetime!(2026-07-25 08:00));
            assert!(matches!(
                service.create_task_entity(stale_until).await,
                Err(ServiceError::Invalid(_))
            ));
        });
    }

    #[test]
    fn state_patch_clears_snooze_timestamp_and_snoozed_state_requires_snooze_action() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let service = test_service().await;
            let mut task =
                Task::quick_capture("task".into(), "Task".into(), String::new(), TaskSize::Small);
            task.state = TaskState::Snoozed;
            task.snoozed_until = Some(time::macros::datetime!(2026-07-25 08:00));
            service.create_task_entity(task).await.unwrap();

            service
                .patch_task("task".into(), 1, TaskPatch::State(TaskState::Done))
                .await
                .unwrap();
            let completed = service.get_task("task").await.unwrap();
            assert_eq!(completed.value.state, "done");
            assert_eq!(completed.value.snoozed_until, None);

            let error = service
                .patch_task("task".into(), 2, TaskPatch::State(TaskState::Snoozed))
                .await
                .unwrap_err();
            assert!(matches!(error, ServiceError::Invalid(_)));
            assert_eq!(service.get_task("task").await.unwrap().revision, 2);
        });
    }

    #[test]
    fn cascade_deletes_increment_every_affected_revision() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let service = test_service().await;
            let person = Person::new("person".into(), "Ada".into(), String::new());
            service.create_person_entity(person).await.unwrap();
            let mut project = Project::new(
                "project".into(),
                "CORE".into(),
                "Core".into(),
                String::new(),
            );
            project.lead_person_id = Some("person".into());
            service.create_project_entity(project).await.unwrap();
            service
                .create_tag_entity(Tag::new("tag".into(), "api".into()))
                .await
                .unwrap();
            let mut task =
                Task::quick_capture("task".into(), "Task".into(), String::new(), TaskSize::Small);
            task.people_ids = vec!["person".into()];
            task.project_ids = vec!["project".into()];
            task.tag_ids = vec!["tag".into()];
            service.create_task_entity(task).await.unwrap();

            service.delete_person("person", 1).await.unwrap();
            assert_eq!(service.get_task("task").await.unwrap().revision, 2);
            let project = service
                .workspace()
                .await
                .unwrap()
                .projects
                .into_iter()
                .next()
                .unwrap();
            assert_eq!(project.revision, 2);
            assert_eq!(project.value.lead_person_id, None);
            assert!(matches!(
                service
                    .patch_project("project".into(), 1, ProjectPatch::Name("Stale".into()))
                    .await,
                Err(ServiceError::Conflict {
                    actual: Some(2),
                    ..
                })
            ));

            service.delete_project("project", 2).await.unwrap();
            assert_eq!(service.get_task("task").await.unwrap().revision, 3);
            service.delete_tag("tag", 1).await.unwrap();
            assert_eq!(service.get_task("task").await.unwrap().revision, 4);
            assert!(matches!(
                service
                    .patch_task("task".into(), 1, TaskPatch::Title("Stale".into()))
                    .await,
                Err(ServiceError::Conflict {
                    actual: Some(4),
                    ..
                })
            ));
        });
    }

    #[test]
    fn empty_management_updates_are_rejected() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let service = test_service().await;
            service
                .create_person_entity(Person::new("person".into(), "Ada".into(), String::new()))
                .await
                .unwrap();
            service
                .create_project_entity(Project::new(
                    "project".into(),
                    "CORE".into(),
                    "Core".into(),
                    String::new(),
                ))
                .await
                .unwrap();
            service
                .create_tag_entity(Tag::new("tag".into(), "api".into()))
                .await
                .unwrap();

            assert!(matches!(
                service
                    .patch_person("person".into(), 1, PersonPatch::Name("  ".into()))
                    .await,
                Err(ServiceError::Invalid(_))
            ));
            assert!(matches!(
                service
                    .patch_project("project".into(), 1, ProjectPatch::Key(String::new()))
                    .await,
                Err(ServiceError::Invalid(_))
            ));
            assert!(matches!(
                service
                    .patch_tag("tag".into(), 1, TagPatch::Label("\t".into()))
                    .await,
                Err(ServiceError::Invalid(_))
            ));
        });
    }

    #[test]
    fn concurrent_workspace_reads_return_matching_values_and_revisions() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            sqlx::any::install_default_drivers();
            let path = std::env::temp_dir().join(format!("tuido-{}.sqlite", Uuid::new_v4()));
            let url = format!("sqlite://{}?mode=rwc", path.display());
            let pool = AnyPoolOptions::new()
                .max_connections(4)
                .connect(&url)
                .await
                .unwrap();
            sqlx::migrate!().run(&pool).await.unwrap();
            sqlx::query("PRAGMA busy_timeout = 5000")
                .execute(&pool)
                .await
                .unwrap();
            let service = TuidoService::from_parts(pool.clone(), SqlDialect::Sqlite);
            service
                .create_task_entity(Task::quick_capture(
                    "task".into(),
                    "v0".into(),
                    String::new(),
                    TaskSize::Small,
                ))
                .await
                .unwrap();
            let writer = {
                let service = service.clone();
                tokio::spawn(async move {
                    for revision in 1..=20 {
                        service
                            .patch_task(
                                "task".into(),
                                revision,
                                TaskPatch::Title(format!("v{revision}")),
                            )
                            .await
                            .unwrap();
                        tokio::task::yield_now().await;
                    }
                })
            };

            for _ in 0..40 {
                let workspace = service.workspace().await.unwrap();
                let task = &workspace.tasks[0];
                assert_eq!(workspace.revision, task.revision + 1);
                let value_revision = task
                    .value
                    .title
                    .strip_prefix('v')
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
                assert_eq!(value_revision + 1, task.revision);
                tokio::task::yield_now().await;
            }
            writer.await.unwrap();
            pool.close().await;
            let _ = std::fs::remove_file(path);
        });
    }
}
