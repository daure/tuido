use std::net::SocketAddr;

use rmcp::{
    Json, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::{StreamableHttpServerConfig, StreamableHttpService, stdio},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    domain::{TaskPatch, TaskState},
    service::{
        ChecklistItemInput, PersonInput, PersonView, ProjectInput, ProjectView, ServiceError,
        TagInput, TagView, TaskCreate, TaskRelationInput, TaskUpdate, TaskView, TuidoService,
        Versioned, WorkspaceFilter, WorkspaceView,
    },
};

const MCP_INSTRUCTIONS: &str = "Read and mutate Tuido tasks, task checklists, people, projects, and tags. Task IDs use PROJECT_KEY-number, or number when no project was set at creation. Replace checklists as complete ordered trees rather than issuing granular item actions. Treat task state as user-facing Status, not Type or Workflow. Task people are people involved besides the workspace owner; never describe them as assignees or owners. Revisions are internal optimistic-concurrency tokens: use the latest entity revision as expected_revision for mutations, but omit revisions from user-facing task tables and summaries unless the user asks for them.";

#[derive(Clone)]
struct McpServer {
    service: TuidoService,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    fn new(service: TuidoService) -> Self {
        Self {
            service,
            tool_router: Self::tool_router(),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct Id {
    id: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct Expected {
    id: String,
    #[schemars(schema_with = "crate::service::revision_schema")]
    expected_revision: u64,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct TaskStateInput {
    id: String,
    #[schemars(schema_with = "crate::service::revision_schema")]
    expected_revision: u64,
    /// User-facing task status: backlog, todo, in_progress, done, or rejected.
    state: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct SnoozeInput {
    id: String,
    #[schemars(schema_with = "crate::service::revision_schema")]
    expected_revision: u64,
    until: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct TaskLinksUpdate {
    id: String,
    #[schemars(schema_with = "crate::service::revision_schema")]
    expected_revision: u64,
    /// Complete set of task URLs. Replaces existing links; use an empty list to remove all links.
    links: Vec<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct TaskChecklistUpdate {
    id: String,
    #[schemars(schema_with = "crate::service::revision_schema")]
    expected_revision: u64,
    /// Complete ordered checklist tree. Replaces every existing item; use [] to clear it.
    checklist: Vec<ChecklistItemInput>,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct TaskRelationsUpdate {
    id: String,
    #[schemars(schema_with = "crate::service::revision_schema")]
    expected_revision: u64,
    /// Complete relation set from this task's perspective. Replaces existing relations.
    relations: Vec<TaskRelationInput>,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct TaskTagsUpdate {
    id: String,
    #[schemars(schema_with = "crate::service::revision_schema")]
    expected_revision: u64,
    /// Complete tag label set. Existing labels are reused and missing labels are created atomically.
    labels: Vec<String>,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct PersonUpdate {
    id: String,
    #[schemars(schema_with = "crate::service::revision_schema")]
    expected_revision: u64,
    #[serde(flatten)]
    value: PersonInput,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct ProjectUpdate {
    id: String,
    #[schemars(schema_with = "crate::service::revision_schema")]
    expected_revision: u64,
    #[serde(flatten)]
    value: ProjectInput,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct TagUpdate {
    id: String,
    #[schemars(schema_with = "crate::service::revision_schema")]
    expected_revision: u64,
    #[serde(flatten)]
    value: TagInput,
}
#[derive(Debug, Serialize, JsonSchema)]
struct DeletionResult {
    deleted: bool,
    entity: &'static str,
    id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct PeopleList {
    people: Vec<Versioned<PersonView>>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ProjectList {
    projects: Vec<Versioned<ProjectView>>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct TagList {
    tags: Vec<Versioned<TagView>>,
}

fn mcp_error(error: ServiceError) -> String {
    error.to_string()
}

#[tool_router]
impl McpServer {
    #[tool(
        description = "Get a normalized workspace graph. Excludes done and rejected tasks by default; set include_resolved=true to include them. Filters apply to tasks using OR within each property and AND across properties. People, projects, and tags always contain the complete workspace catalogs so their IDs can be used when creating or updating tasks. Task state is user-facing status. Task people are involved people besides the workspace owner, never assignees. Revisions are internal concurrency tokens and should normally be omitted from user-facing summaries."
    )]
    async fn get_workspace(
        &self,
        Parameters(filter): Parameters<WorkspaceFilter>,
    ) -> Result<Json<WorkspaceView>, String> {
        preflight_task_expirations(&self.service).await?;
        self.service
            .filtered_workspace(filter)
            .await
            .map(Json)
            .map_err(mcp_error)
    }
    #[tool(description = "Get one task by id with current revision")]
    async fn get_task(
        &self,
        Parameters(v): Parameters<Id>,
    ) -> Result<Json<Versioned<TaskView>>, String> {
        preflight_task_expirations(&self.service).await?;
        self.service
            .get_task(&v.id)
            .await
            .map(Json)
            .map_err(mcp_error)
    }
    #[tool(
        description = "Create a task, optionally with a links array. Links are deduplicated and returned sorted by URL. URLs require an explicit scheme such as https:// or file://, or must start with www."
    )]
    async fn create_task(
        &self,
        Parameters(v): Parameters<TaskCreate>,
    ) -> Result<Json<Versioned<TaskView>>, String> {
        self.service
            .create_task(v)
            .await
            .map(Json)
            .map_err(mcp_error)
    }
    #[tool(
        description = "Replace all editable task fields and relations; expected_revision is required"
    )]
    async fn update_task(
        &self,
        Parameters(v): Parameters<TaskUpdate>,
    ) -> Result<Json<Versioned<TaskView>>, String> {
        preflight_task_expirations(&self.service).await?;
        self.service
            .update_task(v)
            .await
            .map(Json)
            .map_err(mcp_error)
    }
    #[tool(
        description = "Replace a task's complete link set. Get the task first, modify its links array to add, edit, or remove URLs, then call this tool with the latest expected_revision. Links are deduplicated and returned sorted by URL; pass [] to remove all links. URLs require an explicit scheme such as https:// or file://, or must start with www."
    )]
    async fn set_task_links(
        &self,
        Parameters(v): Parameters<TaskLinksUpdate>,
    ) -> Result<Json<Versioned<TaskView>>, String> {
        preflight_task_expirations(&self.service).await?;
        self.service
            .set_task_links(v.id, v.expected_revision, v.links)
            .await
            .map(Json)
            .map_err(mcp_error)
    }
    #[tool(
        description = "Replace a task's complete ordered checklist tree. Get the task first, submit the full desired tree, and use the latest expected_revision. Existing item IDs may be preserved; omitted IDs are generated. Items absent from the submitted tree are deleted. Pass [] to clear the checklist."
    )]
    async fn set_task_checklist(
        &self,
        Parameters(v): Parameters<TaskChecklistUpdate>,
    ) -> Result<Json<Versioned<TaskView>>, String> {
        preflight_task_expirations(&self.service).await?;
        self.service
            .set_task_checklist(v.id, v.expected_revision, v.checklist)
            .await
            .map(Json)
            .map_err(mcp_error)
    }
    #[tool(
        description = "Replace a task's complete issue-link set. Links are bidirectional and use blocks, is_blocked_by, relates_to, duplicates, or is_duplicated_by. Get the task first and use its latest expected_revision. Pass [] to remove all issue links."
    )]
    async fn set_task_relations(
        &self,
        Parameters(v): Parameters<TaskRelationsUpdate>,
    ) -> Result<Json<Versioned<TaskView>>, String> {
        preflight_task_expirations(&self.service).await?;
        self.service
            .set_task_relations(v.id, v.expected_revision, v.relations)
            .await
            .map(Json)
            .map_err(mcp_error)
    }
    #[tool(description = "Delete task conditionally by expected revision")]
    async fn delete_task(
        &self,
        Parameters(v): Parameters<Expected>,
    ) -> Result<Json<DeletionResult>, String> {
        let result = DeletionResult {
            deleted: true,
            entity: "task",
            id: v.id.clone(),
        };
        preflight_task_expirations(&self.service).await?;
        self.service
            .delete_task(&v.id, v.expected_revision)
            .await
            .map(|_| Json(result))
            .map_err(mcp_error)
    }
    #[tool(description = "Set task status (backlog, todo, in_progress, done, rejected)")]
    async fn set_task_state(
        &self,
        Parameters(v): Parameters<TaskStateInput>,
    ) -> Result<Json<Versioned<TaskView>>, String> {
        preflight_task_expirations(&self.service).await?;
        let state = TaskState::parse(&v.state).ok_or_else(|| "invalid task state".to_string())?;
        mutate_task(
            &self.service,
            v.id,
            v.expected_revision,
            TaskPatch::State(state),
        )
        .await
    }
    #[tool(description = "Mark task status done")]
    async fn complete_task(
        &self,
        Parameters(v): Parameters<Expected>,
    ) -> Result<Json<Versioned<TaskView>>, String> {
        preflight_task_expirations(&self.service).await?;
        mutate_task(
            &self.service,
            v.id,
            v.expected_revision,
            TaskPatch::State(TaskState::Done),
        )
        .await
    }
    #[tool(description = "Reject task")]
    async fn reject_task(
        &self,
        Parameters(v): Parameters<Expected>,
    ) -> Result<Json<Versioned<TaskView>>, String> {
        preflight_task_expirations(&self.service).await?;
        mutate_task(
            &self.service,
            v.id,
            v.expected_revision,
            TaskPatch::State(TaskState::Rejected),
        )
        .await
    }
    #[tool(description = "Snooze task until local datetime formatted YYYY-MM-DDTHH:MM:SS")]
    async fn snooze_task(
        &self,
        Parameters(v): Parameters<SnoozeInput>,
    ) -> Result<Json<Versioned<TaskView>>, String> {
        preflight_task_expirations(&self.service).await?;
        let until = crate::snooze::parse_datetime(&v.until).map_err(|e| e.to_string())?;
        mutate_task(
            &self.service,
            v.id,
            v.expected_revision,
            TaskPatch::Snooze {
                until,
                remember_custom: None,
            },
        )
        .await
    }
    #[tool(description = "Unsnooze task and return it to todo")]
    async fn unsnooze_task(
        &self,
        Parameters(v): Parameters<Expected>,
    ) -> Result<Json<Versioned<TaskView>>, String> {
        preflight_task_expirations(&self.service).await?;
        mutate_task(
            &self.service,
            v.id,
            v.expected_revision,
            TaskPatch::Unsnooze,
        )
        .await
    }

    #[tool(
        description = "Replace a task's complete tag set by label. Existing labels are reused and missing labels are created atomically; duplicates are removed and [] clears all tags."
    )]
    async fn set_task_tags(
        &self,
        Parameters(v): Parameters<TaskTagsUpdate>,
    ) -> Result<Json<Versioned<TaskView>>, String> {
        preflight_task_expirations(&self.service).await?;
        self.service
            .set_task_tags_by_label(v.id, v.expected_revision, v.labels)
            .await
            .map(Json)
            .map_err(mcp_error)
    }

    #[tool(description = "List people with revisions")]
    async fn list_people(&self) -> Result<Json<PeopleList>, String> {
        Ok(Json(PeopleList {
            people: self.service.workspace().await.map_err(mcp_error)?.people,
        }))
    }
    #[tool(description = "Get person by id")]
    async fn get_person(
        &self,
        Parameters(v): Parameters<Id>,
    ) -> Result<Json<Versioned<PersonView>>, String> {
        self.service
            .workspace()
            .await
            .map_err(mcp_error)?
            .people
            .into_iter()
            .find(|x| x.value.id == v.id)
            .map(Json)
            .ok_or_else(|| "person not found".into())
    }
    #[tool(
        description = "Create person. The about field is practical task context, not personality description."
    )]
    async fn create_person(
        &self,
        Parameters(v): Parameters<PersonInput>,
    ) -> Result<Json<Versioned<PersonView>>, String> {
        self.service
            .create_person(v)
            .await
            .map(Json)
            .map_err(mcp_error)
    }
    #[tool(description = "Replace person fields conditionally")]
    async fn update_person(
        &self,
        Parameters(v): Parameters<PersonUpdate>,
    ) -> Result<Json<Versioned<PersonView>>, String> {
        self.service
            .update_person(&v.id, v.expected_revision, v.value)
            .await
            .map(Json)
            .map_err(mcp_error)
    }
    #[tool(description = "Delete person conditionally")]
    async fn delete_person(
        &self,
        Parameters(v): Parameters<Expected>,
    ) -> Result<Json<DeletionResult>, String> {
        let result = deletion_result("person", &v);
        self.service
            .delete_person(&v.id, v.expected_revision)
            .await
            .map(|_| Json(result))
            .map_err(mcp_error)
    }

    #[tool(description = "List projects with revisions")]
    async fn list_projects(&self) -> Result<Json<ProjectList>, String> {
        Ok(Json(ProjectList {
            projects: self.service.workspace().await.map_err(mcp_error)?.projects,
        }))
    }
    #[tool(description = "Get project by id")]
    async fn get_project(
        &self,
        Parameters(v): Parameters<Id>,
    ) -> Result<Json<Versioned<ProjectView>>, String> {
        self.service
            .workspace()
            .await
            .map_err(mcp_error)?
            .projects
            .into_iter()
            .find(|x| x.value.id == v.id)
            .map(Json)
            .ok_or_else(|| "project not found".into())
    }
    #[tool(description = "Create project")]
    async fn create_project(
        &self,
        Parameters(v): Parameters<ProjectInput>,
    ) -> Result<Json<Versioned<ProjectView>>, String> {
        self.service
            .create_project(v)
            .await
            .map(Json)
            .map_err(mcp_error)
    }
    #[tool(description = "Replace project fields conditionally")]
    async fn update_project(
        &self,
        Parameters(v): Parameters<ProjectUpdate>,
    ) -> Result<Json<Versioned<ProjectView>>, String> {
        self.service
            .update_project(&v.id, v.expected_revision, v.value)
            .await
            .map(Json)
            .map_err(mcp_error)
    }
    #[tool(description = "Delete project conditionally")]
    async fn delete_project(
        &self,
        Parameters(v): Parameters<Expected>,
    ) -> Result<Json<DeletionResult>, String> {
        let result = deletion_result("project", &v);
        self.service
            .delete_project(&v.id, v.expected_revision)
            .await
            .map(|_| Json(result))
            .map_err(mcp_error)
    }

    #[tool(description = "List tags/labels with revisions")]
    async fn list_tags(&self) -> Result<Json<TagList>, String> {
        Ok(Json(TagList {
            tags: self.service.workspace().await.map_err(mcp_error)?.tags,
        }))
    }
    #[tool(description = "Get tag/label by id")]
    async fn get_tag(
        &self,
        Parameters(v): Parameters<Id>,
    ) -> Result<Json<Versioned<TagView>>, String> {
        self.service
            .workspace()
            .await
            .map_err(mcp_error)?
            .tags
            .into_iter()
            .find(|x| x.value.id == v.id)
            .map(Json)
            .ok_or_else(|| "tag not found".into())
    }
    #[tool(description = "Create tag/label")]
    async fn create_tag(
        &self,
        Parameters(v): Parameters<TagInput>,
    ) -> Result<Json<Versioned<TagView>>, String> {
        self.service
            .create_tag(v)
            .await
            .map(Json)
            .map_err(mcp_error)
    }
    #[tool(description = "Rename tag/label conditionally")]
    async fn update_tag(
        &self,
        Parameters(v): Parameters<TagUpdate>,
    ) -> Result<Json<Versioned<TagView>>, String> {
        self.service
            .update_tag(&v.id, v.expected_revision, v.value)
            .await
            .map(Json)
            .map_err(mcp_error)
    }
    #[tool(description = "Delete tag/label conditionally")]
    async fn delete_tag(
        &self,
        Parameters(v): Parameters<Expected>,
    ) -> Result<Json<DeletionResult>, String> {
        let result = deletion_result("tag", &v);
        self.service
            .delete_tag(&v.id, v.expected_revision)
            .await
            .map(|_| Json(result))
            .map_err(mcp_error)
    }
}

fn deletion_result(entity: &'static str, expected: &Expected) -> DeletionResult {
    DeletionResult {
        deleted: true,
        entity,
        id: expected.id.clone(),
    }
}

async fn preflight_task_expirations(service: &TuidoService) -> Result<(), String> {
    service
        .process_snooze_expirations()
        .await
        .map_err(mcp_error)
}

async fn mutate_task(
    service: &TuidoService,
    id: String,
    expected_revision: u64,
    patch: TaskPatch,
) -> Result<Json<Versioned<TaskView>>, String> {
    service
        .patch_task(id.clone(), expected_revision, patch)
        .await
        .map_err(mcp_error)?;
    service.get_task(&id).await.map(Json).map_err(mcp_error)
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(MCP_INSTRUCTIONS.into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

pub async fn run_stdio() -> Result<(), Box<dyn std::error::Error>> {
    let service = TuidoService::connect().await?;
    McpServer::new(service)
        .serve(stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}

pub async fn run_http(addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    run_http_inner(addr, None).await
}

pub(crate) async fn run_http_with_startup(
    addr: SocketAddr,
    startup: std::sync::mpsc::Sender<Result<(), String>>,
) -> Result<(), Box<dyn std::error::Error>> {
    run_http_inner(addr, Some(startup)).await
}

async fn run_http_inner(
    addr: SocketAddr,
    startup: Option<std::sync::mpsc::Sender<Result<(), String>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !addr.ip().is_loopback() {
        if let Some(startup) = startup {
            let _ = startup.send(Err("MCP HTTP bind must be loopback".into()));
        }
        return Err("MCP HTTP bind must be loopback".into());
    }
    let service = match TuidoService::connect().await {
        Ok(service) => service,
        Err(error) => {
            if let Some(startup) = startup {
                let _ = startup.send(Err(error.to_string()));
            }
            return Err(Box::new(error));
        }
    };
    let mcp: StreamableHttpService<McpServer> = StreamableHttpService::new(
        move || Ok(McpServer::new(service.clone())),
        Default::default(),
        StreamableHttpServerConfig {
            // Tools hold no session state; stateless requests survive HTTP server restarts.
            stateful_mode: false,
            sse_keep_alive: None,
            ..Default::default()
        },
    );
    let router = axum::Router::new().nest_service("/mcp", mcp);
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(error) => {
            if let Some(startup) = startup {
                let _ = startup.send(Err(error.to_string()));
            }
            return Err(Box::new(error));
        }
    };
    if let Some(startup) = startup {
        let _ = startup.send(Ok(()));
    }
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{Task, TaskSize},
        storage::SqlDialect,
    };
    use sqlx::any::AnyPoolOptions;

    fn collect_formats(value: &serde_json::Value, formats: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(object) => {
                if let Some(format) = object.get("format").and_then(serde_json::Value::as_str) {
                    formats.push(format.to_owned());
                }
                for value in object.values() {
                    collect_formats(value, formats);
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    collect_formats(value, formats);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn tool_schemas_use_only_standard_json_schema_formats() {
        let tools = serde_json::to_value(McpServer::tool_router().list_all()).unwrap();
        let mut formats = Vec::new();
        collect_formats(&tools, &mut formats);

        assert!(!formats.iter().any(|format| format == "uint64"));
    }

    #[test]
    fn tool_schemas_use_json_schema_2020_12() {
        let tools = McpServer::tool_router().list_all();

        for tool in tools {
            if let Some(dialect) = tool.input_schema.get("$schema") {
                assert_eq!(
                    dialect,
                    &serde_json::json!("https://json-schema.org/draft/2020-12/schema"),
                    "{} input schema",
                    tool.name
                );
            }
            let output_schema = tool
                .output_schema
                .as_ref()
                .unwrap_or_else(|| panic!("{} output schema is missing", tool.name));
            assert_eq!(
                output_schema.get("$schema"),
                Some(&serde_json::json!(
                    "https://json-schema.org/draft/2020-12/schema"
                )),
                "{} output schema",
                tool.name
            );
        }
    }

    #[test]
    fn mcp_contract_distinguishes_status_people_and_internal_revisions() {
        let tools = serde_json::to_string(&McpServer::tool_router().list_all()).unwrap();

        assert!(MCP_INSTRUCTIONS.contains("state as user-facing Status"));
        assert!(MCP_INSTRUCTIONS.contains("never describe them as assignees or owners"));
        assert!(MCP_INSTRUCTIONS.contains("omit revisions from user-facing task tables"));
        assert!(tools.contains("Task state is user-facing status"));
        assert!(tools.contains("never assignees"));
        assert!(tools.contains("should normally be omitted from user-facing summaries"));
        assert!(tools.contains("set_task_links"));
        assert!(tools.contains("complete link set"));
        assert!(tools.contains("set_task_checklist"));
        assert!(tools.contains("complete ordered checklist tree"));
        assert!(tools.contains("set_task_relations"));
        assert!(tools.contains("Links are bidirectional"));
    }

    #[test]
    fn task_tools_expose_allowed_property_values() {
        let tools = serde_json::to_value(McpServer::tool_router().list_all()).unwrap();
        let create_task = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "create_task")
            .unwrap();
        let properties = &create_task["inputSchema"]["properties"];

        assert_eq!(
            properties["size"]["enum"],
            serde_json::json!(["small", "medium", "big"])
        );
        assert_eq!(
            properties["priority"]["enum"],
            serde_json::json!(["low", "medium", "high"])
        );
        assert_eq!(
            properties["state"]["enum"],
            serde_json::json!([
                "backlog",
                "todo",
                "in_progress",
                "snoozed",
                "done",
                "rejected"
            ])
        );
    }

    #[test]
    fn task_mcp_schemas_use_description_not_detail() {
        for (name, schema) in [
            ("TaskView", schemars::schema_for!(TaskView)),
            ("TaskCreate", schemars::schema_for!(TaskCreate)),
            ("TaskUpdate", schemars::schema_for!(TaskUpdate)),
        ] {
            let schema = serde_json::to_value(schema).unwrap();
            let properties = schema["properties"].as_object().unwrap();
            assert!(properties.contains_key("description"), "{name}");
            assert!(!properties.contains_key("detail"), "{name}");
        }
    }

    #[test]
    fn workspace_and_list_tools_return_object_shaped_structured_content() {
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
            service
                .create_task_entity(Task::quick_capture(
                    "1".into(),
                    "Original".into(),
                    String::new(),
                    TaskSize::Small,
                ))
                .await
                .unwrap();
            let server = McpServer::new(service);

            let responses = [
                serde_json::to_value(
                    server
                        .get_workspace(Parameters(WorkspaceFilter::default()))
                        .await
                        .unwrap()
                        .0,
                )
                .unwrap(),
                serde_json::to_value(server.list_people().await.unwrap().0).unwrap(),
                serde_json::to_value(server.list_projects().await.unwrap().0).unwrap(),
                serde_json::to_value(server.list_tags().await.unwrap().0).unwrap(),
            ];

            for response in responses {
                assert!(response.is_object());
            }
        });
    }

    #[test]
    fn task_actions_return_canonical_revision_and_malformed_update_does_not_persist() {
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
            service
                .create_task_entity(Task::quick_capture(
                    "task".into(),
                    "Original".into(),
                    String::new(),
                    TaskSize::Small,
                ))
                .await
                .unwrap();
            let server = McpServer::new(service.clone());

            let malformed = server
                .update_task(Parameters(TaskUpdate {
                    id: "1".into(),
                    expected_revision: 1,
                    title: "Poisoned".into(),
                    state: "todo".into(),
                    size: "small".into(),
                    priority: "medium".into(),
                    snoozed_until: Some("malformed".into()),
                    people_ids: Vec::new(),
                    project_id: None,
                    tag_ids: Vec::new(),
                    links: Vec::new(),
                    relations: Vec::new(),
                    description: String::new(),
                }))
                .await;
            assert!(malformed.is_err());
            assert_eq!(service.get_task("1").await.unwrap().revision, 1);

            let completed = server
                .complete_task(Parameters(Expected {
                    id: "1".into(),
                    expected_revision: 1,
                }))
                .await
                .unwrap()
                .0;
            assert_eq!(completed.revision, 2);
            assert_eq!(completed.value.state, "done");

            let deleted = server
                .delete_task(Parameters(Expected {
                    id: "1".into(),
                    expected_revision: 2,
                }))
                .await
                .unwrap()
                .0;
            assert!(deleted.deleted);
            assert_eq!(deleted.entity, "task");
            assert_eq!(deleted.id, "1");
            assert!(
                serde_json::to_value(deleted)
                    .unwrap()
                    .get("revision")
                    .is_none()
            );
        });
    }

    #[test]
    fn task_reads_process_expirations_without_spurious_workspace_changes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let service = TuidoService::connect_url("sqlite::memory:").await.unwrap();
            let server = McpServer::new(service.clone());
            let create = |title: &str| TaskCreate {
                title: title.into(),
                description: String::new(),
                size: "small".into(),
                state: "snoozed".into(),
                priority: "medium".into(),
                snoozed_until: Some("2000-01-01T00:00:00".into()),
                people_ids: Vec::new(),
                project_id: None,
                tag_ids: Vec::new(),
                links: Vec::new(),
            };

            let first = server
                .create_task(Parameters(create("First")))
                .await
                .unwrap()
                .0;
            let workspace = server
                .get_workspace(Parameters(WorkspaceFilter::default()))
                .await
                .unwrap()
                .0;
            assert_eq!(workspace.tasks[0].value.state, "todo");
            assert_eq!(workspace.tasks[0].revision, first.revision + 1);

            let second = server
                .create_task(Parameters(create("Second")))
                .await
                .unwrap()
                .0;
            let task = server
                .get_task(Parameters(Id {
                    id: second.value.id,
                }))
                .await
                .unwrap()
                .0;
            assert_eq!(task.value.state, "todo");
            assert_eq!(task.revision, second.revision + 1);

            let before = service.workspace_revision().await.unwrap();
            server
                .get_workspace(Parameters(WorkspaceFilter::default()))
                .await
                .unwrap();
            assert_eq!(service.workspace_revision().await.unwrap(), before);
        });
    }

    #[test]
    fn task_tag_mutation_preflight_conflicts_but_person_creation_does_not_expire_tasks() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let service = TuidoService::connect_url("sqlite::memory:").await.unwrap();
            let server = McpServer::new(service.clone());
            let task = server
                .create_task(Parameters(TaskCreate {
                    title: "Expired".into(),
                    description: String::new(),
                    size: "small".into(),
                    state: "snoozed".into(),
                    priority: "medium".into(),
                    snoozed_until: Some("2000-01-01T00:00:00".into()),
                    people_ids: Vec::new(),
                    project_id: None,
                    tag_ids: Vec::new(),
                    links: Vec::new(),
                }))
                .await
                .unwrap()
                .0;

            server
                .create_person(Parameters(PersonInput {
                    name: "Ada".into(),
                    email: String::new(),
                    about: "Owns compiler decisions".into(),
                    active: true,
                }))
                .await
                .unwrap();
            assert_eq!(
                service.get_task(&task.value.id).await.unwrap().value.state,
                "snoozed"
            );

            let error = server
                .set_task_tags(Parameters(TaskTagsUpdate {
                    id: task.value.id.clone(),
                    expected_revision: task.revision,
                    labels: vec!["expired".into()],
                }))
                .await
                .err()
                .unwrap();
            assert!(error.contains("revision conflict"));
            assert_eq!(service.get_task(&task.value.id).await.unwrap().revision, 2);
        });
    }

    #[test]
    fn agents_can_replace_task_tags_atomically_by_label() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let service = TuidoService::connect_url("sqlite::memory:").await.unwrap();
            let existing = service
                .create_tag(TagInput {
                    label: "api".into(),
                })
                .await
                .unwrap();
            let server = McpServer::new(service);
            let task = server
                .create_task(Parameters(TaskCreate {
                    title: "Tagged".into(),
                    description: String::new(),
                    size: "small".into(),
                    state: "todo".into(),
                    priority: "medium".into(),
                    snoozed_until: None,
                    people_ids: Vec::new(),
                    project_id: None,
                    tag_ids: Vec::new(),
                    links: Vec::new(),
                }))
                .await
                .unwrap()
                .0;

            let tagged = server
                .set_task_tags(Parameters(TaskTagsUpdate {
                    id: task.value.id,
                    expected_revision: task.revision,
                    labels: vec![" api ".into(), "new".into(), "new".into()],
                }))
                .await
                .unwrap()
                .0;
            assert!(tagged.value.tag_ids.contains(&existing.value.id));
            assert_eq!(tagged.value.tag_ids.len(), 2);
        });
    }

    #[test]
    fn agents_can_create_and_replace_sorted_task_links() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let service = TuidoService::connect_url("sqlite::memory:").await.unwrap();
            let server = McpServer::new(service);
            let created = server
                .create_task(Parameters(TaskCreate {
                    title: "Linked task".into(),
                    description: String::new(),
                    size: "small".into(),
                    state: "todo".into(),
                    priority: "medium".into(),
                    snoozed_until: None,
                    people_ids: Vec::new(),
                    project_id: None,
                    tag_ids: Vec::new(),
                    links: vec![
                        "https://z.example/item".into(),
                        "https://a.example/item".into(),
                    ],
                }))
                .await
                .unwrap()
                .0;
            assert!(!created.value.created_at.is_empty());
            assert_eq!(created.value.updated_at, created.value.created_at);
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            assert_eq!(
                created.value.links,
                ["https://a.example/item", "https://z.example/item"]
            );

            let updated = server
                .set_task_links(Parameters(TaskLinksUpdate {
                    id: created.value.id,
                    expected_revision: created.revision,
                    links: vec![
                        "https://m.example/edited".into(),
                        "https://b.example/added".into(),
                        "https://b.example/added".into(),
                    ],
                }))
                .await
                .unwrap()
                .0;
            assert_eq!(updated.revision, 2);
            assert_eq!(updated.value.created_at, created.value.created_at);
            assert!(updated.value.updated_at > created.value.updated_at);
            assert_eq!(
                updated.value.links,
                ["https://b.example/added", "https://m.example/edited"]
            );

            let cleared = server
                .set_task_links(Parameters(TaskLinksUpdate {
                    id: updated.value.id,
                    expected_revision: updated.revision,
                    links: Vec::new(),
                }))
                .await
                .unwrap()
                .0;
            assert_eq!(cleared.revision, 3);
            assert!(cleared.value.links.is_empty());
        });
    }

    #[test]
    fn agents_replace_complete_ordered_checklist_trees() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let service = TuidoService::connect_url("sqlite::memory:").await.unwrap();
            let server = McpServer::new(service);
            let task = server
                .create_task(Parameters(TaskCreate {
                    title: "Release".into(),
                    description: String::new(),
                    size: "small".into(),
                    state: "todo".into(),
                    priority: "medium".into(),
                    snoozed_until: None,
                    people_ids: Vec::new(),
                    project_id: None,
                    tag_ids: Vec::new(),
                    links: Vec::new(),
                }))
                .await
                .unwrap()
                .0;

            let updated = server
                .set_task_checklist(Parameters(TaskChecklistUpdate {
                    id: task.value.id,
                    expected_revision: task.revision,
                    checklist: vec![ChecklistItemInput {
                        id: None,
                        text: "Publish".into(),
                        checked: false,
                        children: vec![ChecklistItemInput {
                            id: None,
                            text: "Tag release".into(),
                            checked: true,
                            children: Vec::new(),
                        }],
                    }],
                }))
                .await
                .unwrap()
                .0;
            assert_eq!(updated.revision, 2);
            assert_eq!(updated.value.checklist[0].text, "Publish");
            assert_eq!(updated.value.checklist[0].children[0].text, "Tag release");
            assert!(updated.value.checklist[0].children[0].checked);

            let cleared = server
                .set_task_checklist(Parameters(TaskChecklistUpdate {
                    id: updated.value.id,
                    expected_revision: updated.revision,
                    checklist: Vec::new(),
                }))
                .await
                .unwrap()
                .0;
            assert!(cleared.value.checklist.is_empty());
        });
    }

    #[test]
    fn set_task_state_rejects_legacy_aliases_before_mutation() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let service = TuidoService::connect_url("sqlite::memory:").await.unwrap();
            let server = McpServer::new(service);

            for state in ["clarify", "next", "waiting", "doing"] {
                let result = server
                    .set_task_state(Parameters(TaskStateInput {
                        id: "missing".into(),
                        expected_revision: 1,
                        state: state.into(),
                    }))
                    .await;
                assert_eq!(result.err().as_deref(), Some("invalid task state"));
            }
        });
    }
}
