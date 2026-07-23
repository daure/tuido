use std::collections::HashMap;

use tuicore::{ChipColorRole, DispatchOutcome};

#[derive(Debug, Clone)]
pub struct WorkspaceSnapshot {
    pub tasks: Vec<Task>,
    pub people: Vec<Person>,
    pub projects: Vec<Project>,
    pub tags: Vec<Tag>,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub tasks: Vec<Task>,
    pub people: Vec<Person>,
    pub projects: Vec<Project>,
    pub tags: Vec<Tag>,
    pub selected_task_id: Option<String>,
    pub selected_person_id: Option<String>,
    pub selected_project_id: Option<String>,
    pub selected_tag_id: Option<String>,
    pub save_errors: HashMap<SaveTarget, String>,
    pub version: u64,
}

impl AppState {
    pub fn from_snapshot(snapshot: WorkspaceSnapshot) -> Self {
        Self {
            selected_task_id: snapshot.tasks.first().map(|task| task.id.clone()),
            selected_person_id: snapshot.people.first().map(|person| person.id.clone()),
            selected_project_id: snapshot.projects.first().map(|project| project.id.clone()),
            selected_tag_id: snapshot.tags.first().map(|tag| tag.id.clone()),
            tasks: snapshot.tasks,
            people: snapshot.people,
            projects: snapshot.projects,
            tags: snapshot.tags,
            save_errors: HashMap::new(),
            version: 0,
        }
    }

    pub fn task_save_error(&self, task_id: &str) -> Option<&str> {
        self.save_error_for(task_id, |field| matches!(field, SaveEntityField::Task(_)))
    }

    pub fn person_save_error(&self, person_id: &str) -> Option<&str> {
        self.save_error_for(person_id, |field| {
            matches!(field, SaveEntityField::Person(_))
        })
    }

    pub fn project_save_error(&self, project_id: &str) -> Option<&str> {
        self.save_error_for(project_id, |field| {
            matches!(field, SaveEntityField::Project(_))
        })
    }

    pub fn tag_save_error(&self, tag_id: &str) -> Option<&str> {
        self.save_error_for(tag_id, |field| matches!(field, SaveEntityField::Tag(_)))
    }

    fn save_error_for(
        &self,
        entity_id: &str,
        matches_field: impl Fn(SaveEntityField) -> bool,
    ) -> Option<&str> {
        self.save_errors.iter().find_map(|(target, error)| {
            (target.entity_id == entity_id && matches_field(target.field)).then_some(error.as_str())
        })
    }
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    TaskCreated(Task),
    TaskDeleted(String),
    SelectTask(String),
    PatchTask {
        task_id: String,
        patch: TaskPatch,
    },
    SelectPerson(String),
    PatchPerson {
        person_id: String,
        patch: PersonPatch,
    },
    SelectProject(String),
    PatchProject {
        project_id: String,
        patch: ProjectPatch,
    },
    SelectTag(String),
    PatchTag {
        tag_id: String,
        patch: TagPatch,
    },
    SaveCompleted {
        target: SaveTarget,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SaveEntityField {
    Task(TaskField),
    Person(PersonField),
    Project(ProjectField),
    Tag(TagField),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SaveTarget {
    entity_id: String,
    field: SaveEntityField,
}

impl SaveTarget {
    pub fn task(entity_id: String, field: TaskField) -> Self {
        Self {
            entity_id,
            field: SaveEntityField::Task(field),
        }
    }

    pub fn person(entity_id: String, field: PersonField) -> Self {
        Self {
            entity_id,
            field: SaveEntityField::Person(field),
        }
    }

    pub fn project(entity_id: String, field: ProjectField) -> Self {
        Self {
            entity_id,
            field: SaveEntityField::Project(field),
        }
    }

    pub fn tag(entity_id: String, field: TagField) -> Self {
        Self {
            entity_id,
            field: SaveEntityField::Tag(field),
        }
    }
}

pub fn reduce_app_state(state: &mut AppState, event: AppEvent) -> DispatchOutcome {
    match event {
        AppEvent::TaskCreated(task) => {
            state.selected_task_id = Some(task.id.clone());
            state.tasks.push(task);
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::TaskDeleted(task_id) => {
            let Some(index) = state.tasks.iter().position(|task| task.id == task_id) else {
                return DispatchOutcome::unchanged();
            };
            state.tasks.remove(index);
            state
                .save_errors
                .retain(|target, _| target.entity_id != task_id);
            if state.selected_task_id.as_deref() == Some(&task_id) {
                state.selected_task_id = state
                    .tasks
                    .get(index)
                    .or_else(|| state.tasks.last())
                    .map(|task| task.id.clone());
            }
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::SelectTask(task_id) => {
            if state.selected_task_id.as_deref() == Some(&task_id) {
                DispatchOutcome::unchanged()
            } else {
                state.selected_task_id = Some(task_id);
                DispatchOutcome::layout()
            }
        }
        AppEvent::PatchTask { task_id, patch } => {
            let Some(index) = state.tasks.iter().position(|task| task.id == task_id) else {
                return DispatchOutcome::unchanged();
            };
            if !apply_task_patch(&mut state.tasks[index], &mut state.tags, &patch) {
                return DispatchOutcome::unchanged();
            }
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::SelectPerson(person_id) => {
            if state.selected_person_id.as_deref() == Some(&person_id) {
                DispatchOutcome::unchanged()
            } else {
                state.selected_person_id = Some(person_id);
                DispatchOutcome::layout()
            }
        }
        AppEvent::PatchPerson { person_id, patch } => {
            let Some(index) = state
                .people
                .iter()
                .position(|person| person.id == person_id)
            else {
                return DispatchOutcome::unchanged();
            };
            if !apply_person_patch(&mut state.people[index], &patch) {
                return DispatchOutcome::unchanged();
            }
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::SelectProject(project_id) => {
            if state.selected_project_id.as_deref() == Some(&project_id) {
                DispatchOutcome::unchanged()
            } else {
                state.selected_project_id = Some(project_id);
                DispatchOutcome::layout()
            }
        }
        AppEvent::PatchProject { project_id, patch } => {
            let Some(index) = state
                .projects
                .iter()
                .position(|project| project.id == project_id)
            else {
                return DispatchOutcome::unchanged();
            };
            if !apply_project_patch(&mut state.projects[index], &patch) {
                return DispatchOutcome::unchanged();
            }
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::SelectTag(tag_id) => {
            if state.selected_tag_id.as_deref() == Some(&tag_id) {
                DispatchOutcome::unchanged()
            } else {
                state.selected_tag_id = Some(tag_id);
                DispatchOutcome::layout()
            }
        }
        AppEvent::PatchTag { tag_id, patch } => {
            let Some(index) = state.tags.iter().position(|tag| tag.id == tag_id) else {
                return DispatchOutcome::unchanged();
            };
            if !apply_tag_patch(&mut state.tags[index], &patch) {
                return DispatchOutcome::unchanged();
            }
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::SaveCompleted { target, error } => {
            let changed = if let Some(error) = error {
                let message = format!(
                    "Save failed for {} {:?}: {error}",
                    target.entity_id, target.field
                );
                state.save_errors.get(&target) != Some(&message) && {
                    state.save_errors.insert(target, message);
                    true
                }
            } else {
                state.save_errors.remove(&target).is_some()
            };
            if changed {
                state.version += 1;
                DispatchOutcome::changed()
            } else {
                DispatchOutcome::unchanged()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub state: TaskState,
    pub size: TaskSize,
    pub priority: TaskPriority,
    pub start_date: Option<String>,
    pub due_date: Option<String>,
    pub people_ids: Vec<String>,
    pub project_ids: Vec<String>,
    pub tag_ids: Vec<String>,
    pub detail: String,
}

impl Task {
    pub fn quick_capture(id: String, title: String, detail: String, size: TaskSize) -> Self {
        Self {
            id,
            title: title.trim().to_string(),
            state: TaskState::Todo,
            size,
            priority: TaskPriority::Medium,
            start_date: None,
            due_date: None,
            people_ids: Vec::new(),
            project_ids: Vec::new(),
            tag_ids: Vec::new(),
            detail,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub email: String,
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub id: String,
    pub key: String,
    pub name: String,
    pub description: String,
    pub lead_person_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskField {
    Title,
    Detail,
    State,
    Size,
    Priority,
    StartDate,
    EndDate,
    People,
    Projects,
    Tags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PersonField {
    Name,
    Email,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectField {
    Key,
    Name,
    Description,
    LeadPerson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagField {
    Label,
}

#[derive(Debug, Clone)]
pub enum TaskPatch {
    Title(String),
    Detail(String),
    State(TaskState),
    Size(TaskSize),
    Priority(TaskPriority),
    StartDate(Option<String>),
    EndDate(Option<String>),
    People(Vec<String>),
    Projects(Vec<String>),
    Tags(Vec<Tag>),
}

impl TaskPatch {
    pub fn field(&self) -> TaskField {
        match self {
            Self::Title(_) => TaskField::Title,
            Self::Detail(_) => TaskField::Detail,
            Self::State(_) => TaskField::State,
            Self::Size(_) => TaskField::Size,
            Self::Priority(_) => TaskField::Priority,
            Self::StartDate(_) => TaskField::StartDate,
            Self::EndDate(_) => TaskField::EndDate,
            Self::People(_) => TaskField::People,
            Self::Projects(_) => TaskField::Projects,
            Self::Tags(_) => TaskField::Tags,
        }
    }
}

#[derive(Debug, Clone)]
pub enum PersonPatch {
    Name(String),
    Email(String),
    Active(bool),
}

impl PersonPatch {
    pub fn field(&self) -> PersonField {
        match self {
            Self::Name(_) => PersonField::Name,
            Self::Email(_) => PersonField::Email,
            Self::Active(_) => PersonField::Active,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ProjectPatch {
    Key(String),
    Name(String),
    Description(String),
    LeadPerson(Option<String>),
}

#[derive(Debug, Clone)]
pub enum TagPatch {
    Label(String),
}

impl TagPatch {
    pub fn field(&self) -> TagField {
        match self {
            Self::Label(_) => TagField::Label,
        }
    }
}

impl ProjectPatch {
    pub fn field(&self) -> ProjectField {
        match self {
            Self::Key(_) => ProjectField::Key,
            Self::Name(_) => ProjectField::Name,
            Self::Description(_) => ProjectField::Description,
            Self::LeadPerson(_) => ProjectField::LeadPerson,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Todo,
    InProgress,
    Done,
    Snoozed,
    Rejected,
}

impl TaskState {
    pub fn id(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Snoozed => "snoozed",
            Self::Rejected => "rejected",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Todo => "TODO",
            Self::InProgress => "IN-PROGRESS",
            Self::Done => "DONE",
            Self::Snoozed => "SNOOZED",
            Self::Rejected => "REJECTED",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "todo" | "clarify" | "next" | "waiting" => Some(Self::Todo),
            "in_progress" | "doing" => Some(Self::InProgress),
            "done" => Some(Self::Done),
            "snoozed" => Some(Self::Snoozed),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSize {
    Small,
    Medium,
    Big,
}

impl TaskSize {
    pub fn id(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Big => "big",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "SMALL",
            Self::Medium => "MED",
            Self::Big => "BIG",
        }
    }

    pub fn role(self) -> ChipColorRole {
        match self {
            Self::Small => ChipColorRole::Success,
            Self::Medium => ChipColorRole::Accent,
            Self::Big => ChipColorRole::Error,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "small" => Some(Self::Small),
            "medium" => Some(Self::Medium),
            "big" => Some(Self::Big),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPriority {
    Low,
    Medium,
    High,
}

impl TaskPriority {
    pub fn id(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

fn apply_task_patch(task: &mut Task, available_tags: &mut Vec<Tag>, patch: &TaskPatch) -> bool {
    match patch {
        TaskPatch::Title(title) if task.title != title.trim() && !title.trim().is_empty() => {
            task.title = title.trim().to_string();
            true
        }
        TaskPatch::Detail(detail) if task.detail != *detail => {
            task.detail = detail.clone();
            true
        }
        TaskPatch::State(value) if task.state != *value => {
            task.state = *value;
            true
        }
        TaskPatch::Size(value) if task.size != *value => {
            task.size = *value;
            true
        }
        TaskPatch::Priority(value) if task.priority != *value => {
            task.priority = *value;
            true
        }
        TaskPatch::StartDate(value) if task.start_date != *value => {
            task.start_date = value.clone();
            true
        }
        TaskPatch::EndDate(value) if task.due_date != *value => {
            task.due_date = value.clone();
            true
        }
        TaskPatch::People(ids) if task.people_ids != *ids => {
            task.people_ids = ids.clone();
            true
        }
        TaskPatch::Projects(ids) if task.project_ids != *ids => {
            task.project_ids = ids.clone();
            true
        }
        TaskPatch::Tags(tags) => {
            let mut next_tag_ids = Vec::new();
            for tag in tags {
                let label = tag.label.trim();
                if label.is_empty() {
                    continue;
                }
                let id = if let Some(existing) = available_tags
                    .iter()
                    .find(|existing| existing.label == label)
                {
                    existing.id.clone()
                } else {
                    let tag = Tag {
                        id: tag.id.clone(),
                        label: label.to_string(),
                    };
                    let id = tag.id.clone();
                    available_tags.push(tag);
                    id
                };
                if !next_tag_ids.contains(&id) {
                    next_tag_ids.push(id);
                }
            }
            if task.tag_ids == next_tag_ids {
                false
            } else {
                task.tag_ids = next_tag_ids;
                true
            }
        }
        _ => false,
    }
}

fn apply_person_patch(person: &mut Person, patch: &PersonPatch) -> bool {
    match patch {
        PersonPatch::Name(name) if person.name != name.trim() && !name.trim().is_empty() => {
            person.name = name.trim().to_string();
            true
        }
        PersonPatch::Email(email) if person.email != email.trim() => {
            person.email = email.trim().to_string();
            true
        }
        PersonPatch::Active(active) if person.active != *active => {
            person.active = *active;
            true
        }
        _ => false,
    }
}

fn apply_project_patch(project: &mut Project, patch: &ProjectPatch) -> bool {
    match patch {
        ProjectPatch::Key(key) if project.key != key.trim() && !key.trim().is_empty() => {
            project.key = key.trim().to_string();
            true
        }
        ProjectPatch::Name(name) if project.name != name.trim() && !name.trim().is_empty() => {
            project.name = name.trim().to_string();
            true
        }
        ProjectPatch::Description(description) if project.description != *description => {
            project.description = description.clone();
            true
        }
        ProjectPatch::LeadPerson(lead_person_id)
            if project.lead_person_id.as_ref() != lead_person_id.as_ref() =>
        {
            project.lead_person_id = lead_person_id.clone();
            true
        }
        _ => false,
    }
}

fn apply_tag_patch(tag: &mut Tag, patch: &TagPatch) -> bool {
    match patch {
        TagPatch::Label(label) if tag.label != label.trim() && !label.trim().is_empty() => {
            tag.label = label.trim().to_string();
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_success_only_clears_matching_failed_field() {
        let mut state = AppState::from_snapshot(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });

        reduce_app_state(
            &mut state,
            AppEvent::SaveCompleted {
                target: SaveTarget::task("T-1".to_string(), TaskField::Title),
                error: Some("disk full".to_string()),
            },
        );
        reduce_app_state(
            &mut state,
            AppEvent::SaveCompleted {
                target: SaveTarget::task("T-2".to_string(), TaskField::Title),
                error: None,
            },
        );

        assert_eq!(
            state.task_save_error("T-1"),
            Some("Save failed for T-1 Task(Title): disk full")
        );
        assert_eq!(state.task_save_error("T-2"), None);

        reduce_app_state(
            &mut state,
            AppEvent::SaveCompleted {
                target: SaveTarget::task("T-1".to_string(), TaskField::Title),
                error: None,
            },
        );

        assert_eq!(state.task_save_error("T-1"), None);
    }

    #[test]
    fn save_completion_changes_version_only_when_visible_status_changes() {
        let mut state = AppState::from_snapshot(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let target = SaveTarget::task("T-1".to_string(), TaskField::Detail);

        let success = reduce_app_state(
            &mut state,
            AppEvent::SaveCompleted {
                target: target.clone(),
                error: None,
            },
        );
        assert!(!success.changed);
        assert_eq!(state.version, 0);

        let failure = reduce_app_state(
            &mut state,
            AppEvent::SaveCompleted {
                target: target.clone(),
                error: Some("disk full".to_string()),
            },
        );
        assert!(failure.changed);
        assert_eq!(state.version, 1);

        let repeated_failure = reduce_app_state(
            &mut state,
            AppEvent::SaveCompleted {
                target: target.clone(),
                error: Some("disk full".to_string()),
            },
        );
        assert!(!repeated_failure.changed);
        assert_eq!(state.version, 1);

        let recovered = reduce_app_state(
            &mut state,
            AppEvent::SaveCompleted {
                target,
                error: None,
            },
        );
        assert!(recovered.changed);
        assert_eq!(state.version, 2);
    }

    #[test]
    fn deleting_selected_task_selects_next_available_task() {
        let first = Task::quick_capture(
            "first".to_string(),
            "First".to_string(),
            String::new(),
            TaskSize::Small,
        );
        let second = Task::quick_capture(
            "second".to_string(),
            "Second".to_string(),
            String::new(),
            TaskSize::Small,
        );
        let mut state = AppState::from_snapshot(WorkspaceSnapshot {
            tasks: vec![first, second],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });

        let outcome = reduce_app_state(&mut state, AppEvent::TaskDeleted("first".to_string()));

        assert!(outcome.changed);
        assert_eq!(state.tasks.len(), 1);
        assert_eq!(state.selected_task_id.as_deref(), Some("second"));
    }

    #[test]
    fn newly_created_tags_become_available_to_other_tasks() {
        let first = Task::quick_capture(
            "first".to_string(),
            "First".to_string(),
            String::new(),
            TaskSize::Small,
        );
        let second = Task::quick_capture(
            "second".to_string(),
            "Second".to_string(),
            String::new(),
            TaskSize::Small,
        );
        let mut state = AppState::from_snapshot(WorkspaceSnapshot {
            tasks: vec![first, second],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });

        reduce_app_state(
            &mut state,
            AppEvent::PatchTask {
                task_id: "first".to_string(),
                patch: TaskPatch::Tags(vec![Tag {
                    id: "backend-id".to_string(),
                    label: "backend".to_string(),
                }]),
            },
        );
        reduce_app_state(
            &mut state,
            AppEvent::PatchTask {
                task_id: "second".to_string(),
                patch: TaskPatch::Tags(vec![Tag {
                    id: "duplicate-id".to_string(),
                    label: "backend".to_string(),
                }]),
            },
        );

        assert_eq!(
            state.tags,
            vec![Tag {
                id: "backend-id".to_string(),
                label: "backend".to_string(),
            }]
        );
        assert_eq!(state.tasks[0].tag_ids, vec!["backend-id"]);
        assert_eq!(state.tasks[1].tag_ids, vec!["backend-id"]);
    }
}
