use std::collections::HashMap;

use tuicore::{ChipColorRole, DispatchOutcome};

#[derive(Debug, Clone)]
pub struct WorkspaceSnapshot {
    pub tasks: Vec<Task>,
    pub people: Vec<Person>,
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub tasks: Vec<Task>,
    pub people: Vec<Person>,
    pub projects: Vec<Project>,
    pub selected_task_id: Option<String>,
    pub selected_person_id: Option<String>,
    pub selected_project_id: Option<String>,
    pub save_errors: HashMap<SaveTarget, String>,
    pub version: u64,
}

impl AppState {
    pub fn from_snapshot(snapshot: WorkspaceSnapshot) -> Self {
        Self {
            selected_task_id: snapshot.tasks.first().map(|task| task.id.clone()),
            selected_person_id: snapshot.people.first().map(|person| person.id.clone()),
            selected_project_id: snapshot.projects.first().map(|project| project.id.clone()),
            tasks: snapshot.tasks,
            people: snapshot.people,
            projects: snapshot.projects,
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
}

pub fn reduce_app_state(state: &mut AppState, event: AppEvent) -> DispatchOutcome {
    match event {
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
            if !apply_task_patch(
                &mut state.tasks[index],
                &state.people,
                &state.projects,
                &patch,
            ) {
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
            refresh_task_context_labels(&mut state.tasks, &state.people, &state.projects);
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
            refresh_task_context_labels(&mut state.tasks, &state.people, &state.projects);
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
    pub task_type: TaskType,
    pub subtype: TaskSubtype,
    pub state: TaskState,
    pub size: TaskSize,
    pub start_date: Option<String>,
    pub due_date: Option<String>,
    pub people_ids: Vec<String>,
    pub project_ids: Vec<String>,
    pub entity_labels: Vec<String>,
    pub focus_today: bool,
    pub frog_candidate: bool,
    pub detail: String,
    pub ai_rationale: String,
    pub swap_note: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskField {
    Title,
    Detail,
    Type,
    Subtype,
    State,
    Size,
    StartDate,
    EndDate,
    People,
    Projects,
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

#[derive(Debug, Clone)]
pub enum TaskPatch {
    Title(String),
    Detail(String),
    Type(TaskType),
    Subtype(TaskSubtype),
    State(TaskState),
    Size(TaskSize),
    StartDate(Option<String>),
    EndDate(Option<String>),
    People(Vec<String>),
    Projects(Vec<String>),
}

impl TaskPatch {
    pub fn field(&self) -> TaskField {
        match self {
            Self::Title(_) => TaskField::Title,
            Self::Detail(_) => TaskField::Detail,
            Self::Type(_) => TaskField::Type,
            Self::Subtype(_) => TaskField::Subtype,
            Self::State(_) => TaskField::State,
            Self::Size(_) => TaskField::Size,
            Self::StartDate(_) => TaskField::StartDate,
            Self::EndDate(_) => TaskField::EndDate,
            Self::People(_) => TaskField::People,
            Self::Projects(_) => TaskField::Projects,
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
pub enum TaskType {
    Action,
    Note,
}

impl TaskType {
    pub fn id(self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::Note => "note",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "action" => Some(Self::Action),
            "note" => Some(Self::Note),
            "waiting" | "follow_up" | "task" | "artifact_update" => Some(Self::Action),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskSubtype {
    Task,
    Waiting,
    FollowUp,
    ArtifactUpdate,
}

impl TaskSubtype {
    pub fn id(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Waiting => "waiting",
            Self::FollowUp => "follow_up",
            Self::ArtifactUpdate => "artifact_update",
        }
    }

    pub fn workflow_kind(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::FollowUp => "follow_up",
            Self::Task | Self::ArtifactUpdate => "action",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "task" | "action" => Some(Self::Task),
            "waiting" => Some(Self::Waiting),
            "follow_up" => Some(Self::FollowUp),
            "artifact_update" => Some(Self::ArtifactUpdate),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Todo,
    InProgress,
    Done,
    Snoozed,
}

impl TaskState {
    pub fn id(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Snoozed => "snoozed",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Todo => "TODO",
            Self::InProgress => "IN-PROGRESS",
            Self::Done => "DONE",
            Self::Snoozed => "SNOOZED",
        }
    }

    pub fn role(self) -> ChipColorRole {
        match self {
            Self::Todo => ChipColorRole::Accent,
            Self::InProgress | Self::Done => ChipColorRole::Success,
            Self::Snoozed => ChipColorRole::Muted,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "todo" | "clarify" | "next" | "waiting" => Some(Self::Todo),
            "in_progress" | "doing" => Some(Self::InProgress),
            "done" => Some(Self::Done),
            "snoozed" => Some(Self::Snoozed),
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

fn apply_task_patch(
    task: &mut Task,
    people: &[Person],
    projects: &[Project],
    patch: &TaskPatch,
) -> bool {
    match patch {
        TaskPatch::Title(title) if task.title != title.trim() && !title.trim().is_empty() => {
            task.title = title.trim().to_string();
            true
        }
        TaskPatch::Detail(detail) if task.detail != *detail => {
            task.detail = detail.clone();
            true
        }
        TaskPatch::Type(value) if task.task_type != *value => {
            task.task_type = *value;
            true
        }
        TaskPatch::Subtype(value) if task.subtype != *value => {
            task.subtype = *value;
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
            task.entity_labels = task_context_labels(task, people, projects);
            true
        }
        TaskPatch::Projects(ids) if task.project_ids != *ids => {
            task.project_ids = ids.clone();
            task.entity_labels = task_context_labels(task, people, projects);
            true
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

fn refresh_task_context_labels(tasks: &mut [Task], people: &[Person], projects: &[Project]) {
    for task in tasks {
        task.entity_labels = task_context_labels(task, people, projects);
    }
}

pub fn task_context_labels(task: &Task, people: &[Person], projects: &[Project]) -> Vec<String> {
    task.people_ids
        .iter()
        .filter_map(|id| people.iter().find(|person| &person.id == id))
        .map(|person| person.name.clone())
        .chain(
            task.project_ids
                .iter()
                .filter_map(|id| projects.iter().find(|project| &project.id == id))
                .map(|project| project.name.clone()),
        )
        .collect()
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
}
