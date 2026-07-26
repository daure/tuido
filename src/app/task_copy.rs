use std::collections::HashMap;

use serde::Serialize;

use crate::domain::{Person, Project, Tag, Task};

#[derive(Clone, Default)]
pub(super) struct TaskCopyContext {
    people: HashMap<String, Person>,
    projects: HashMap<String, Project>,
    tags: HashMap<String, Tag>,
}

impl TaskCopyContext {
    pub(super) fn new(people: &[Person], projects: &[Project], tags: &[Tag]) -> Self {
        Self {
            people: people
                .iter()
                .cloned()
                .map(|person| (person.id.clone(), person))
                .collect(),
            projects: projects
                .iter()
                .cloned()
                .map(|project| (project.id.clone(), project))
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
    start_date: Option<&'a str>,
    due_date: Option<&'a str>,
    snoozed_until: Option<String>,
    people: Vec<PersonExport<'a>>,
    projects: Vec<ProjectExport<'a>>,
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
        let projects = task
            .project_ids
            .iter()
            .map(|id| {
                let project = context
                    .projects
                    .get(id)
                    .ok_or_else(|| CopyError::unresolved(task, "project", id))?;
                let lead = project
                    .lead_person_id
                    .as_deref()
                    .map(|lead_id| context.person(task, lead_id, "project_lead_person"))
                    .transpose()?;
                Ok(ProjectExport::new(project, lead))
            })
            .collect::<Result<_, CopyError>>()?;
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
            start_date: task.start_date.as_deref(),
            due_date: task.due_date.as_deref(),
            snoozed_until: task.snoozed_until.map(crate::snooze::format_datetime),
            people,
            projects,
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
struct ProjectExport<'a> {
    id: &'a str,
    key: &'a str,
    name: &'a str,
    description: &'a str,
    lead: Option<PersonExport<'a>>,
}

impl<'a> ProjectExport<'a> {
    fn new(project: &'a Project, lead: Option<PersonExport<'a>>) -> Self {
        Self {
            id: &project.id,
            key: &project.key,
            name: &project.name,
            description: &project.description,
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
