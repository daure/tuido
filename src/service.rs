use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::{Any, AnyPool, AssertSqlSafe, Row, Transaction};
use uuid::Uuid;

use crate::{
    domain::{
        ChecklistItem, Person, PersonPatch, Project, ProjectPatch, Tag, TagPatch, Task, TaskPatch,
        TaskRank, WorkspaceSnapshot,
    },
    storage::{self, SqlDialect, Storage},
};

mod settings;
mod validation;

use validation::{
    task_matches_workspace_filter, validate_required, validate_task_checklist, validate_task_patch,
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

#[derive(Debug, Clone)]
pub(crate) struct TaskRankUpdate {
    pub rank: TaskRank,
    pub expected_revision: u64,
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
    /// Match user-facing task statuses such as backlog, todo, in_progress, snoozed, done, or rejected.
    #[schemars(extend("items" = {"type": "string", "enum": ["backlog", "todo", "in_progress", "snoozed", "done", "rejected"]}))]
    pub states: Vec<String>,
    #[schemars(extend("items" = {"type": "string", "enum": ["low", "medium", "high"]}))]
    pub priorities: Vec<String>,
    #[schemars(extend("items" = {"type": "string", "enum": ["small", "medium", "big"]}))]
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
    /// Creation time as Unix epoch nanoseconds.
    pub created_at: String,
    /// Last update time as Unix epoch nanoseconds.
    pub updated_at: String,
    pub title: String,
    /// User-facing task status: backlog, todo, in_progress, snoozed, done, or rejected.
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
    /// Ordered checklist tree. Children are ordered as shown in the task detail view.
    pub checklist: Vec<ChecklistItemView>,
    /// Task URLs, deduplicated and sorted lexicographically.
    pub links: Vec<String>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub struct ChecklistItemView {
    pub id: String,
    pub text: String,
    pub checked: bool,
    pub children: Vec<ChecklistItemView>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ChecklistItemInput {
    /// Existing item ID to preserve, or omit to generate a new ID.
    pub id: Option<String>,
    pub text: String,
    #[serde(default)]
    pub checked: bool,
    #[serde(default)]
    pub children: Vec<ChecklistItemInput>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PersonView {
    pub id: String,
    pub name: String,
    pub email: String,
    pub about: String,
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
    pub description: String,
    #[serde(default = "default_size")]
    #[schemars(extend("enum" = ["small", "medium", "big"]))]
    pub size: String,
    #[serde(default = "default_state")]
    /// User-facing task status: backlog, todo, in_progress, snoozed, done, or rejected.
    #[schemars(extend("enum" = ["backlog", "todo", "in_progress", "snoozed", "done", "rejected"]))]
    pub state: String,
    #[serde(default = "default_priority")]
    #[schemars(extend("enum" = ["low", "medium", "high"]))]
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
    /// Task URLs. Values require an explicit scheme or a www. prefix.
    pub links: Vec<String>,
}
fn default_size() -> String {
    "medium".into()
}
fn default_state() -> String {
    "backlog".into()
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
    /// User-facing task status: backlog, todo, in_progress, snoozed, done, or rejected.
    #[schemars(extend("enum" = ["backlog", "todo", "in_progress", "snoozed", "done", "rejected"]))]
    pub state: String,
    #[schemars(extend("enum" = ["small", "medium", "big"]))]
    pub size: String,
    #[schemars(extend("enum" = ["low", "medium", "high"]))]
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
    /// Complete task URL set. Values require an explicit scheme or a www. prefix.
    pub links: Vec<String>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PersonInput {
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    /// Practical context that helps define, understand, or reason about tasks involving this person.
    pub about: String,
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

    pub async fn process_snooze_expirations(&self) -> ServiceResult<()> {
        let now = crate::snooze::local_now().map_err(storage_error)?;
        self.process_snooze_expirations_at(now).await
    }

    async fn process_snooze_expirations_at(
        &self,
        now: time::PrimitiveDateTime,
    ) -> ServiceResult<()> {
        let cutoff = crate::snooze::format_datetime(now);
        let select = format!(
            "SELECT id, revision FROM tasks WHERE workflow_state = 'snoozed' AND rejected = false AND snoozed_until IS NOT NULL AND snoozed_until <= {}",
            self.dialect.placeholder(1)
        );
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let due = sqlx::query(AssertSqlSafe(select.as_str()))
            .bind(&cutoff)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage_error)?;
        let update = format!(
            "UPDATE tasks SET state = 'next', workflow_state = 'todo', rejected = false, snoozed_until = NULL, revision = revision + 1, updated_at = {} WHERE id = {} AND revision = {} AND workflow_state = 'snoozed' AND rejected = false AND snoozed_until IS NOT NULL AND snoozed_until <= {}",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2),
            self.dialect.placeholder(3),
            self.dialect.placeholder(4)
        );
        let mut changed = false;
        for row in due {
            let result = sqlx::query(AssertSqlSafe(update.as_str()))
                .bind(now_text())
                .bind(row.try_get::<String, _>("id").map_err(storage_error)?)
                .bind(row.try_get::<i64, _>("revision").map_err(storage_error)?)
                .bind(&cutoff)
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
            changed |= result.rows_affected() == 1;
        }
        if changed {
            bump_workspace(&mut tx, self.dialect).await?;
        }
        tx.commit().await.map_err(storage_error)
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
                            about: v.about,
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
        mut task: Task,
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
        normalize_task_links(&mut task.links);
        normalize_task_checklist(&mut task.checklist);
        validation::validate_task_links(&task.links)?;
        validate_task_checklist(&task.checklist)?;
        let sql = format!(
            "INSERT INTO tasks (id, rank, title, state, workflow_state, rejected, size, priority, start_date, due_date, snoozed_until, description, created_at, updated_at) VALUES ({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
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
            self.dialect.placeholder(12),
            self.dialect.placeholder(13),
            self.dialect.placeholder(14)
        );
        let now = now_text();
        task.created_at.clone_from(&now);
        task.updated_at.clone_from(&now);
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        bump_workspace(&mut tx, self.dialect).await?;
        let row = sqlx::query("SELECT COALESCE(MAX(rank), 0) + 1 AS rank FROM tasks")
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
        task.rank = row.try_get("rank").map_err(storage_error)?;
        sqlx::query(AssertSqlSafe(sql.as_str()))
            .bind(&task.id)
            .bind(task.rank)
            .bind(&task.title)
            .bind(storage_legacy_state(task.state.id()))
            .bind(storage_workflow_state(task.state.id()))
            .bind(task.state == crate::domain::TaskState::Rejected)
            .bind(task.size.id())
            .bind(task.priority.id())
            .bind(&task.start_date)
            .bind(&task.due_date)
            .bind(task.snoozed_until.map(crate::snooze::format_datetime))
            .bind(&task.description)
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
        self.replace_task_links(&mut tx, &task.id, &task.links)
            .await?;
        self.replace_task_checklist(&mut tx, &task.id, &task.checklist)
            .await?;
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
            rank: 0,
            created_at: String::new(),
            updated_at: String::new(),
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
            checklist: Vec::new(),
            links: input.links,
            description: input.description,
        })
        .await
    }

    pub async fn update_task(&self, mut input: TaskUpdate) -> ServiceResult<Versioned<TaskView>> {
        normalize_task_links(&mut input.links);
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
            "UPDATE tasks SET title = {}, state = {}, workflow_state = {}, rejected = {}, size = {}, priority = {}, start_date = {}, due_date = {}, snoozed_until = {}, description = {}, updated_at = {} WHERE id = {}",
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
        sqlx::query(AssertSqlSafe(sql.as_str()))
            .bind(input.title.trim())
            .bind(storage_legacy_state(&input.state))
            .bind(storage_workflow_state(&input.state))
            .bind(input.state == "rejected")
            .bind(&input.size)
            .bind(&input.priority)
            .bind(&input.start_date)
            .bind(&input.due_date)
            .bind(&snoozed_until)
            .bind(&input.description)
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
        self.replace_task_links(&mut tx, &input.id, &input.links)
            .await?;
        bump_workspace(&mut tx, self.dialect).await?;
        tx.commit().await.map_err(storage_error)?;
        self.get_task(&input.id).await
    }

    pub(crate) async fn patch_task(
        &self,
        id: String,
        expected: u64,
        mut patch: TaskPatch,
    ) -> ServiceResult<TaskPatchResult> {
        if let TaskPatch::Links(links) = &mut patch {
            normalize_task_links(links);
        }
        if let TaskPatch::Checklist(items) = &mut patch {
            normalize_task_checklist(items);
        }
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
            TaskPatch::Links(links) => self.replace_task_links(&mut tx, &id, &links).await?,
            TaskPatch::Checklist(items) => {
                self.replace_task_checklist(&mut tx, &id, &items).await?
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

    pub async fn set_task_links(
        &self,
        id: String,
        expected_revision: u64,
        links: Vec<String>,
    ) -> ServiceResult<Versioned<TaskView>> {
        self.patch_task(id.clone(), expected_revision, TaskPatch::Links(links))
            .await?;
        self.get_task(&id).await
    }

    pub async fn set_task_checklist(
        &self,
        id: String,
        expected_revision: u64,
        checklist: Vec<ChecklistItemInput>,
    ) -> ServiceResult<Versioned<TaskView>> {
        let mut items = Vec::new();
        flatten_checklist_inputs(checklist, None, &mut items);
        self.patch_task(id.clone(), expected_revision, TaskPatch::Checklist(items))
            .await?;
        self.get_task(&id).await
    }

    pub(crate) async fn reorder_tasks(
        &self,
        updates: Vec<TaskRankUpdate>,
    ) -> ServiceResult<HashMap<String, u64>> {
        if updates.is_empty() {
            return Ok(HashMap::new());
        }
        let mut ids = HashSet::new();
        let mut ranks = HashSet::new();
        if updates
            .iter()
            .any(|update| !ids.insert(update.rank.id.clone()) || !ranks.insert(update.rank.rank))
        {
            return Err(ServiceError::Invalid(
                "task reorder contains duplicate IDs or ranks".into(),
            ));
        }

        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let mut revisions = HashMap::new();
        let mut current_ranks = HashSet::new();
        for update in &updates {
            self.claim(
                &mut tx,
                "tasks",
                "task",
                &update.rank.id,
                update.expected_revision,
            )
            .await?;
            let select = format!(
                "SELECT rank FROM tasks WHERE id = {}",
                self.dialect.placeholder(1)
            );
            let row = sqlx::query(AssertSqlSafe(select.as_str()))
                .bind(&update.rank.id)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
            current_ranks.insert(row.try_get::<i64, _>("rank").map_err(storage_error)?);
        }
        if current_ranks != ranks {
            return Err(ServiceError::Invalid(
                "task reorder must preserve the existing rank set".into(),
            ));
        }
        for update in &updates {
            let sql = format!(
                "UPDATE tasks SET rank = {}, updated_at = {} WHERE id = {}",
                self.dialect.placeholder(1),
                self.dialect.placeholder(2),
                self.dialect.placeholder(3)
            );
            sqlx::query(AssertSqlSafe(sql.as_str()))
                .bind(update.rank.rank)
                .bind(now_text())
                .bind(&update.rank.id)
                .execute(&mut *tx)
                .await
                .map_err(storage_error)?;
            revisions.insert(
                format!("task:{}", update.rank.id),
                update.expected_revision + 1,
            );
        }
        bump_workspace(&mut tx, self.dialect).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(revisions)
    }

    pub async fn set_task_tags_by_label(
        &self,
        id: String,
        expected_revision: u64,
        labels: Vec<String>,
    ) -> ServiceResult<Versioned<TaskView>> {
        let mut seen = HashSet::new();
        let mut tags = Vec::new();
        for label in labels {
            validate_required("tag label", &label)?;
            let label = label.trim().to_string();
            if seen.insert(label.clone()) {
                tags.push(Tag::new(Uuid::new_v4().to_string(), label));
            }
        }
        self.patch_task(id.clone(), expected_revision, TaskPatch::Tags(tags))
            .await?;
        self.get_task(&id).await
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
            about: input.about,
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
            "INSERT INTO people (id, name, email, about, active) VALUES ({}, {}, {}, {}, {})",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2),
            self.dialect.placeholder(3),
            self.dialect.placeholder(4),
            self.dialect.placeholder(5)
        );
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        sqlx::query(AssertSqlSafe(sql.as_str()))
            .bind(&person.id)
            .bind(&person.name)
            .bind(&person.email)
            .bind(&person.about)
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
                about: person.about,
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
                ("about", Value::Text(input.about)),
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
            PersonPatch::About(v) => ("about", Value::Text(v)),
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
                ("key", Value::Text(Project::normalize_key(&input.key))),
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
            ProjectPatch::Key(v) => ("key", Value::Text(Project::normalize_key(&v))),
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
    async fn replace_task_links(
        &self,
        tx: &mut Transaction<'_, Any>,
        task_id: &str,
        links: &[String],
    ) -> ServiceResult<()> {
        let delete = format!(
            "DELETE FROM task_links WHERE task_id = {}",
            self.dialect.placeholder(1)
        );
        sqlx::query(AssertSqlSafe(delete.as_str()))
            .bind(task_id)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
        let insert = format!(
            "INSERT INTO task_links (task_id, url) VALUES ({}, {})",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2)
        );
        for link in links {
            sqlx::query(AssertSqlSafe(insert.as_str()))
                .bind(task_id)
                .bind(link)
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?;
        }
        Ok(())
    }
    async fn replace_task_checklist(
        &self,
        tx: &mut Transaction<'_, Any>,
        task_id: &str,
        items: &[ChecklistItem],
    ) -> ServiceResult<()> {
        let delete = format!(
            "DELETE FROM task_checklist_items WHERE task_id = {}",
            self.dialect.placeholder(1)
        );
        sqlx::query(AssertSqlSafe(delete.as_str()))
            .bind(task_id)
            .execute(&mut **tx)
            .await
            .map_err(storage_error)?;
        let insert = format!(
            "INSERT INTO task_checklist_items (id, task_id, parent_id, position, text, checked) VALUES ({}, {}, NULL, {}, {}, {})",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2),
            self.dialect.placeholder(3),
            self.dialect.placeholder(4),
            self.dialect.placeholder(5)
        );
        for (position, item) in items.iter().enumerate() {
            sqlx::query(AssertSqlSafe(insert.as_str()))
                .bind(&item.id)
                .bind(task_id)
                .bind(position as i64)
                .bind(&item.text)
                .bind(item.checked)
                .execute(&mut **tx)
                .await
                .map_err(storage_error)?;
        }
        let update_parent = format!(
            "UPDATE task_checklist_items SET parent_id = {} WHERE id = {} AND task_id = {}",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2),
            self.dialect.placeholder(3)
        );
        for item in items.iter().filter(|item| item.parent_id.is_some()) {
            sqlx::query(AssertSqlSafe(update_parent.as_str()))
                .bind(&item.parent_id)
                .bind(&item.id)
                .bind(task_id)
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
    let checklist = checklist_views(&v.checklist, None);
    TaskView {
        id: v.id,
        created_at: v.created_at,
        updated_at: v.updated_at,
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
        checklist,
        links: v.links,
        description: v.description,
    }
}

fn checklist_views(items: &[ChecklistItem], parent_id: Option<&str>) -> Vec<ChecklistItemView> {
    items
        .iter()
        .filter(|item| item.parent_id.as_deref() == parent_id)
        .map(|item| ChecklistItemView {
            id: item.id.clone(),
            text: item.text.clone(),
            checked: item.checked,
            children: checklist_views(items, Some(&item.id)),
        })
        .collect()
}

fn flatten_checklist_inputs(
    inputs: Vec<ChecklistItemInput>,
    parent_id: Option<String>,
    output: &mut Vec<ChecklistItem>,
) {
    for input in inputs {
        let id = input.id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let children = input.children;
        output.push(ChecklistItem {
            id: id.clone(),
            parent_id: parent_id.clone(),
            text: input.text,
            checked: input.checked,
        });
        flatten_checklist_inputs(children, Some(id), output);
    }
}

fn normalize_task_links(links: &mut Vec<String>) {
    links.sort();
    links.dedup();
}

fn normalize_task_checklist(items: &mut [ChecklistItem]) {
    for item in items {
        item.text = item.text.trim().to_string();
    }
}

fn storage_workflow_state(v: &str) -> &str {
    match v {
        "backlog" => "todo",
        "rejected" => "snoozed",
        value => value,
    }
}

fn storage_legacy_state(v: &str) -> &str {
    match v {
        "backlog" => "waiting",
        "in_progress" => "doing",
        "done" => "done",
        "snoozed" | "rejected" => "snoozed",
        _ => "next",
    }
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
    let (columns, values): (Vec<&str>, Vec<Value>) = match patch {
        TaskPatch::Title(v) => (vec!["title"], vec![Value::Text(v.trim().into())]),
        TaskPatch::Description(v) => (vec!["description"], vec![Value::Text(v)]),
        TaskPatch::State(v) => (
            vec!["state", "workflow_state", "rejected", "snoozed_until"],
            vec![
                Value::Text(storage_legacy_state(v.id()).into()),
                Value::Text(storage_workflow_state(v.id()).into()),
                Value::Bool(v == crate::domain::TaskState::Rejected),
                Value::Optional(None),
            ],
        ),
        TaskPatch::Size(v) => (vec!["size"], vec![Value::Text(v.id().into())]),
        TaskPatch::Priority(v) => (vec!["priority"], vec![Value::Text(v.id().into())]),
        TaskPatch::StartDate(v) => (vec!["start_date"], vec![Value::Optional(v)]),
        TaskPatch::EndDate(v) => (vec!["due_date"], vec![Value::Optional(v)]),
        TaskPatch::Snooze { until, .. } => (
            vec!["state", "workflow_state", "rejected", "snoozed_until"],
            vec![
                Value::Text("snoozed".into()),
                Value::Text("snoozed".into()),
                Value::Bool(false),
                Value::Text(crate::snooze::format_datetime(until)),
            ],
        ),
        TaskPatch::Unsnooze => (
            vec!["state", "workflow_state", "rejected", "snoozed_until"],
            vec![
                Value::Text("next".into()),
                Value::Text("todo".into()),
                Value::Bool(false),
                Value::Optional(None),
            ],
        ),
        TaskPatch::People(_)
        | TaskPatch::Projects(_)
        | TaskPatch::Tags(_)
        | TaskPatch::Checklist(_)
        | TaskPatch::Links(_) => {
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
    Ok(())
}

#[cfg(test)]
#[path = "service/tests.rs"]
mod tests;
