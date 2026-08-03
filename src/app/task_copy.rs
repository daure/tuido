use std::collections::HashMap;

use serde::Serialize;

use crate::domain::{Person, Tag, Task, Workspace, task_display_id};

#[derive(Clone, Default)]
pub(super) struct TaskCopyContext {
    people: HashMap<String, Person>,
    workspaces: HashMap<String, Workspace>,
    tags: HashMap<String, Tag>,
}

impl TaskCopyContext {
    pub(super) fn new(people: &[Person], workspaces: &[Workspace], tags: &[Tag]) -> Self {
        Self {
            people: people
                .iter()
                .cloned()
                .map(|person| (person.id.clone(), person))
                .collect(),
            workspaces: workspaces
                .iter()
                .cloned()
                .map(|workspace| (workspace.id.clone(), workspace))
                .collect(),
            tags: tags
                .iter()
                .cloned()
                .map(|tag| (tag.id.clone(), tag))
                .collect(),
        }
    }

    pub(super) fn export(&self, task: &Task) -> String {
        match TaskExport::new(task, self) {
            Ok(export) => pretty_json(&export),
            Err(error) => pretty_json(&error),
        }
    }

    pub(super) fn display_id(&self, task: &Task) -> String {
        let workspace = task
            .workspace_id
            .as_deref()
            .and_then(|workspace_id| self.workspaces.get(workspace_id));
        task_display_id(task, workspace)
    }

    fn person<'a>(
        &'a self,
        task: &Task,
        id: &str,
        relation: &'static str,
    ) -> Result<PersonExport<'a>, CopyError> {
        self.people
            .get(id)
            .map(PersonExport::from)
            .ok_or_else(|| CopyError::unresolved(task, relation, id))
    }
}

#[derive(Serialize)]
struct TaskExport<'a> {
    id: &'a str,
    title: &'a str,
    description: &'a str,
    state: &'static str,
    size: &'static str,
    priority: &'static str,
    snoozed_until: Option<String>,
    people: Vec<PersonExport<'a>>,
    workspace: Option<WorkspaceExport<'a>>,
    tags: Vec<TagExport<'a>>,
    links: Vec<String>,
}

impl<'a> TaskExport<'a> {
    fn new(task: &'a Task, context: &'a TaskCopyContext) -> Result<Self, CopyError> {
        let people = task
            .people_ids
            .iter()
            .map(|id| context.person(task, id, "person"))
            .collect::<Result<_, _>>()?;
        let workspace = task
            .workspace_id
            .as_deref()
            .map(|id| {
                let workspace = context
                    .workspaces
                    .get(id)
                    .ok_or_else(|| CopyError::unresolved(task, "workspace", id))?;
                let lead = workspace
                    .lead_person_id
                    .as_deref()
                    .map(|lead_id| context.person(task, lead_id, "workspace_lead_person"))
                    .transpose()?;
                Ok(WorkspaceExport::new(workspace, lead))
            })
            .transpose()?;
        let tags = task
            .tag_ids
            .iter()
            .map(|id| {
                context
                    .tags
                    .get(id)
                    .map(TagExport::from)
                    .ok_or_else(|| CopyError::unresolved(task, "tag", id))
            })
            .collect::<Result<_, _>>()?;

        Ok(Self {
            id: &task.id,
            title: &task.title,
            description: &task.description,
            state: task.state.id(),
            size: task.size.id(),
            priority: task.priority.id(),
            snoozed_until: task.snoozed_until.map(crate::snooze::format_datetime),
            people,
            workspace,
            tags,
            links: task
                .links
                .iter()
                .map(|url| crate::task_link::browser_target(url))
                .collect(),
        })
    }
}

#[derive(Serialize)]
struct PersonExport<'a> {
    id: &'a str,
    name: &'a str,
    email: &'a str,
    active: bool,
}

impl<'a> From<&'a Person> for PersonExport<'a> {
    fn from(person: &'a Person) -> Self {
        Self {
            id: &person.id,
            name: &person.name,
            email: &person.email,
            active: person.active,
        }
    }
}

#[derive(Serialize)]
struct WorkspaceExport<'a> {
    id: &'a str,
    key: &'a str,
    name: &'a str,
    description: &'a str,
    lead: Option<PersonExport<'a>>,
}

impl<'a> WorkspaceExport<'a> {
    fn new(workspace: &'a Workspace, lead: Option<PersonExport<'a>>) -> Self {
        Self {
            id: &workspace.id,
            key: &workspace.key,
            name: &workspace.name,
            description: &workspace.description,
            lead,
        }
    }
}

#[derive(Serialize)]
struct TagExport<'a> {
    id: &'a str,
    label: &'a str,
}

impl<'a> From<&'a Tag> for TagExport<'a> {
    fn from(tag: &'a Tag) -> Self {
        Self {
            id: &tag.id,
            label: &tag.label,
        }
    }
}

#[derive(Serialize)]
struct CopyError {
    error: ErrorDetail,
}

#[derive(Serialize)]
struct ErrorDetail {
    message: &'static str,
    task_id: String,
    relation: &'static str,
    id: String,
}

impl CopyError {
    fn unresolved(task: &Task, relation: &'static str, id: &str) -> Self {
        Self {
            error: ErrorDetail {
                message: "task copy could not resolve relationship",
                task_id: task.id.clone(),
                relation,
                id: id.to_string(),
            },
        }
    }
}

fn pretty_json(value: &impl Serialize) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| {
        "{\n  \"error\": {\n    \"message\": \"task copy serialization failed\"\n  }\n}".to_string()
    })
}
