use std::collections::HashMap;

use time::PrimitiveDateTime;
use tuicore::{ChipColorRole, DispatchOutcome};

pub(crate) const DEFAULT_WORKSPACE_SETTING: &str = "tasks.default_workspace";

#[derive(Debug, Clone)]
pub struct WorkspaceSnapshot {
    pub tasks: Vec<Task>,
    pub people: Vec<Person>,
    pub workspaces: Vec<Workspace>,
    pub tags: Vec<Tag>,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub tasks: Vec<Task>,
    pub people: Vec<Person>,
    pub workspaces: Vec<Workspace>,
    pub tags: Vec<Tag>,
    pub selected_task_id: Option<String>,
    pub selected_person_id: Option<String>,
    pub selected_workspace_id: Option<String>,
    pub selected_tag_id: Option<String>,
    pub save_errors: HashMap<SaveTarget, String>,
    pub app_setting_errors: HashMap<String, String>,
    pub app_setting_values: HashMap<String, String>,
    pub app_setting_confirmed_values: HashMap<String, String>,
    pub app_setting_desired_values: HashMap<String, String>,
    pub app_setting_generations: HashMap<String, u64>,
    pub refresh_error: Option<String>,
    pub last_custom_snooze: Option<PrimitiveDateTime>,
    pub version: u64,
    pub external_refresh_version: u64,
    pub workspace_revision: u64,
    pub entity_revisions: HashMap<String, u64>,
}

impl AppState {
    pub fn from_snapshot(snapshot: WorkspaceSnapshot) -> Self {
        Self::from_snapshot_with_last_custom(snapshot, None)
    }

    pub fn from_snapshot_with_last_custom(
        snapshot: WorkspaceSnapshot,
        last_custom_snooze: Option<PrimitiveDateTime>,
    ) -> Self {
        Self {
            selected_task_id: snapshot.tasks.first().map(|task| task.id.clone()),
            selected_person_id: snapshot.people.first().map(|person| person.id.clone()),
            selected_workspace_id: snapshot
                .workspaces
                .first()
                .map(|workspace| workspace.id.clone()),
            selected_tag_id: snapshot.tags.first().map(|tag| tag.id.clone()),
            tasks: snapshot.tasks,
            people: snapshot.people,
            workspaces: snapshot.workspaces,
            tags: snapshot.tags,
            last_custom_snooze,
            save_errors: HashMap::new(),
            app_setting_errors: HashMap::new(),
            app_setting_values: HashMap::new(),
            app_setting_confirmed_values: HashMap::new(),
            app_setting_desired_values: HashMap::new(),
            app_setting_generations: HashMap::new(),
            refresh_error: None,
            version: 0,
            external_refresh_version: 0,
            workspace_revision: 0,
            entity_revisions: HashMap::new(),
        }
    }

    pub fn task_save_error(&self, task_id: &str) -> Option<&str> {
        self.save_error_for(task_id, |field| matches!(field, SaveEntityField::Task(_)))
    }

    pub fn task_status_error(&self, task_id: &str) -> Option<&str> {
        self.task_save_error(task_id)
            .or(self.refresh_error.as_deref())
    }

    pub fn person_save_error(&self, person_id: &str) -> Option<&str> {
        self.save_error_for(person_id, |field| {
            matches!(field, SaveEntityField::Person(_))
        })
    }

    pub fn workspace_save_error(&self, workspace_id: &str) -> Option<&str> {
        self.save_error_for(workspace_id, |field| {
            matches!(field, SaveEntityField::Workspace(_))
        })
    }

    pub fn tag_save_error(&self, tag_id: &str) -> Option<&str> {
        self.save_error_for(tag_id, |field| matches!(field, SaveEntityField::Tag(_)))
    }

    pub fn person_deletion(&self, person_id: &str) -> Option<PersonDeletion> {
        let person = self
            .people
            .iter()
            .find(|person| person.id == person_id)?
            .clone();
        Some(PersonDeletion {
            person,
            task_ids: self
                .tasks
                .iter()
                .filter(|task| task.people_ids.iter().any(|id| id == person_id))
                .map(|task| task.id.clone())
                .collect(),
            lead_workspace_ids: self
                .workspaces
                .iter()
                .filter(|workspace| workspace.lead_person_id.as_deref() == Some(person_id))
                .map(|workspace| workspace.id.clone())
                .collect(),
        })
    }

    pub fn workspace_deletion(&self, workspace_id: &str) -> Option<WorkspaceDeletion> {
        let workspace = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)?
            .clone();
        Some(WorkspaceDeletion {
            workspace,
            task_ids: self
                .tasks
                .iter()
                .filter(|task| task.workspace_id.as_deref() == Some(workspace_id))
                .map(|task| task.id.clone())
                .collect(),
            was_default: self
                .app_setting_values
                .get(DEFAULT_WORKSPACE_SETTING)
                .is_some_and(|value| value == workspace_id),
        })
    }

    pub fn tag_deletion(&self, tag_id: &str) -> Option<TagDeletion> {
        let tag = self.tags.iter().find(|tag| tag.id == tag_id)?.clone();
        Some(TagDeletion {
            tag,
            task_ids: self
                .tasks
                .iter()
                .filter(|task| task.tag_ids.iter().any(|id| id == tag_id))
                .map(|task| task.id.clone())
                .collect(),
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
    TaskCreated(Task),
    TaskDeleted(String),
    TaskRanksChanged(Vec<TaskRank>),
    SelectTask(String),
    PatchTask {
        task_id: String,
        patch: TaskPatch,
    },
    PersonCreated(Person),
    PersonDeleted(String),
    PersonRestored(PersonDeletion),
    SelectPerson(String),
    PatchPerson {
        person_id: String,
        patch: PersonPatch,
    },
    WorkspaceCreated(Workspace),
    WorkspaceDeleted(String),
    WorkspaceRestored(WorkspaceDeletion),
    SelectWorkspace(String),
    PatchWorkspace {
        workspace_id: String,
        patch: WorkspacePatch,
    },
    TagCreated(Tag),
    TagDeleted(String),
    TagRestored(TagDeletion),
    SelectTag(String),
    PatchTag {
        tag_id: String,
        patch: TagPatch,
    },
    SaveCompleted {
        target: SaveTarget,
        error: Option<String>,
    },
    AppSettingChangeRequested {
        key: String,
        value: String,
        generation: u64,
    },
    AppSettingSaveCompleted {
        key: String,
        value: String,
        generation: u64,
        error: Option<String>,
    },
    EntityRevisionCommitted {
        key: String,
        revision: Option<u64>,
    },
    WorkspaceRevisionCommitted,
    EntityRevisionsMerged(HashMap<String, u64>),
    WorkspaceRefreshed {
        snapshot: WorkspaceSnapshot,
        revision: u64,
        entity_revisions: HashMap<String, u64>,
    },
    RefreshFailed(String),
    RefreshSucceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SaveEntityField {
    Task(TaskField),
    Person(PersonField),
    Workspace(WorkspaceField),
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

    pub fn workspace(entity_id: String, field: WorkspaceField) -> Self {
        Self {
            entity_id,
            field: SaveEntityField::Workspace(field),
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
            for task in &mut state.tasks {
                task.relations
                    .retain(|relation| relation.task_id != task_id);
            }
            state
                .save_errors
                .retain(|target, _| target.entity_id != task_id);
            if state.selected_task_id.as_deref() == Some(&task_id) {
                state.selected_task_id = None;
            }
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::TaskRanksChanged(ranks) => {
            let mut changed = false;
            for rank in ranks {
                if let Some(task) = state.tasks.iter_mut().find(|task| task.id == rank.id)
                    && task.rank != rank.rank
                {
                    task.rank = rank.rank;
                    changed = true;
                }
            }
            if !changed {
                return DispatchOutcome::unchanged();
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
            let task_changed = match &patch {
                TaskPatch::Relations(relations) => {
                    apply_task_relations(&mut state.tasks, index, relations)
                }
                _ => apply_task_patch(&mut state.tasks[index], &mut state.tags, &patch),
            };
            let custom_changed = match patch {
                TaskPatch::Snooze {
                    remember_custom: Some(custom),
                    ..
                } if state.last_custom_snooze != Some(custom) => {
                    state.last_custom_snooze = Some(custom);
                    true
                }
                _ => false,
            };
            if !task_changed && !custom_changed {
                return DispatchOutcome::unchanged();
            }
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::PersonCreated(person) => {
            state.selected_person_id = Some(person.id.clone());
            state.people.push(person);
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::PersonDeleted(person_id) => {
            let Some(index) = state
                .people
                .iter()
                .position(|person| person.id == person_id)
            else {
                return DispatchOutcome::unchanged();
            };
            state.people.remove(index);
            state
                .save_errors
                .retain(|target, _| target.entity_id != person_id);
            if state.selected_person_id.as_deref() == Some(&person_id) {
                state.selected_person_id = state
                    .people
                    .get(index)
                    .or_else(|| state.people.last())
                    .map(|person| person.id.clone());
            }
            for task in &mut state.tasks {
                task.people_ids.retain(|id| id != &person_id);
            }
            for workspace in &mut state.workspaces {
                if workspace.lead_person_id.as_deref() == Some(&person_id) {
                    workspace.lead_person_id = None;
                }
            }
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::PersonRestored(deletion) => {
            let person_id = deletion.person.id.clone();
            state.selected_person_id = Some(person_id.clone());
            state.people.push(deletion.person);
            for task in &mut state.tasks {
                if deletion.task_ids.contains(&task.id) && !task.people_ids.contains(&person_id) {
                    task.people_ids.push(person_id.clone());
                }
            }
            for workspace in &mut state.workspaces {
                if deletion.lead_workspace_ids.contains(&workspace.id) {
                    workspace.lead_person_id = Some(person_id.clone());
                }
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
        AppEvent::WorkspaceCreated(workspace) => {
            state.selected_workspace_id = Some(workspace.id.clone());
            state.workspaces.push(workspace);
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::WorkspaceDeleted(workspace_id) => {
            let Some(index) = state
                .workspaces
                .iter()
                .position(|workspace| workspace.id == workspace_id)
            else {
                return DispatchOutcome::unchanged();
            };
            state.workspaces.remove(index);
            state
                .save_errors
                .retain(|target, _| target.entity_id != workspace_id);
            if state.selected_workspace_id.as_deref() == Some(&workspace_id) {
                state.selected_workspace_id = state
                    .workspaces
                    .get(index)
                    .or_else(|| state.workspaces.last())
                    .map(|workspace| workspace.id.clone());
            }
            for task in &mut state.tasks {
                if task.workspace_id.as_deref() == Some(&workspace_id) {
                    task.workspace_id = None;
                }
            }
            if state
                .app_setting_values
                .get(DEFAULT_WORKSPACE_SETTING)
                .is_some_and(|value| value == &workspace_id)
            {
                for values in [
                    &mut state.app_setting_values,
                    &mut state.app_setting_confirmed_values,
                    &mut state.app_setting_desired_values,
                ] {
                    values.insert(DEFAULT_WORKSPACE_SETTING.to_string(), String::new());
                }
            }
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::WorkspaceRestored(deletion) => {
            let workspace_id = deletion.workspace.id.clone();
            state.selected_workspace_id = Some(workspace_id.clone());
            state.workspaces.push(deletion.workspace);
            for task in &mut state.tasks {
                if deletion.task_ids.contains(&task.id) {
                    task.workspace_id = Some(workspace_id.clone());
                }
            }
            if deletion.was_default {
                for values in [
                    &mut state.app_setting_values,
                    &mut state.app_setting_confirmed_values,
                    &mut state.app_setting_desired_values,
                ] {
                    values.insert(DEFAULT_WORKSPACE_SETTING.to_string(), workspace_id.clone());
                }
            }
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::SelectWorkspace(workspace_id) => {
            if state.selected_workspace_id.as_deref() == Some(&workspace_id) {
                DispatchOutcome::unchanged()
            } else {
                state.selected_workspace_id = Some(workspace_id);
                DispatchOutcome::layout()
            }
        }
        AppEvent::PatchWorkspace {
            workspace_id,
            patch,
        } => {
            let Some(index) = state
                .workspaces
                .iter()
                .position(|workspace| workspace.id == workspace_id)
            else {
                return DispatchOutcome::unchanged();
            };
            if !apply_workspace_patch(&mut state.workspaces[index], &patch) {
                return DispatchOutcome::unchanged();
            }
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::TagCreated(tag) => {
            state.selected_tag_id = Some(tag.id.clone());
            state.tags.push(tag);
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::TagDeleted(tag_id) => {
            let Some(index) = state.tags.iter().position(|tag| tag.id == tag_id) else {
                return DispatchOutcome::unchanged();
            };
            state.tags.remove(index);
            state
                .save_errors
                .retain(|target, _| target.entity_id != tag_id);
            if state.selected_tag_id.as_deref() == Some(&tag_id) {
                state.selected_tag_id = state
                    .tags
                    .get(index)
                    .or_else(|| state.tags.last())
                    .map(|tag| tag.id.clone());
            }
            for task in &mut state.tasks {
                task.tag_ids.retain(|id| id != &tag_id);
            }
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::TagRestored(deletion) => {
            let tag_id = deletion.tag.id.clone();
            state.selected_tag_id = Some(tag_id.clone());
            state.tags.push(deletion.tag);
            for task in &mut state.tasks {
                if deletion.task_ids.contains(&task.id) && !task.tag_ids.contains(&tag_id) {
                    task.tag_ids.push(tag_id.clone());
                }
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
        AppEvent::AppSettingChangeRequested {
            key,
            value,
            generation,
        } => {
            if state
                .app_setting_generations
                .get(&key)
                .is_some_and(|current| *current >= generation)
            {
                return DispatchOutcome::unchanged();
            }
            if !state.app_setting_confirmed_values.contains_key(&key)
                && let Some(confirmed) = state.app_setting_values.get(&key).cloned()
            {
                state
                    .app_setting_confirmed_values
                    .insert(key.clone(), confirmed);
            }
            state
                .app_setting_generations
                .insert(key.clone(), generation);
            state
                .app_setting_desired_values
                .insert(key.clone(), value.clone());
            let value_changed = state.app_setting_values.get(&key) != Some(&value);
            state.app_setting_values.insert(key.clone(), value);
            let error_changed = state.app_setting_errors.remove(&key).is_some();
            if value_changed || error_changed {
                state.version += 1;
                DispatchOutcome::layout()
            } else {
                DispatchOutcome::unchanged()
            }
        }
        AppEvent::AppSettingSaveCompleted {
            key,
            value,
            generation,
            error,
        } => {
            let is_latest = state.app_setting_generations.get(&key) == Some(&generation);
            if error.is_none() {
                state
                    .app_setting_confirmed_values
                    .insert(key.clone(), value.clone());
            }
            if !is_latest {
                return DispatchOutcome::unchanged();
            }
            let changed = if let Some(error) = error {
                let message = format!("Setting save failed for {key}: {error}");
                let confirmed = state
                    .app_setting_confirmed_values
                    .get(&key)
                    .cloned()
                    .unwrap_or_default();
                let value_changed = state.app_setting_values.get(&key) != Some(&confirmed);
                state
                    .app_setting_desired_values
                    .insert(key.clone(), confirmed.clone());
                state.app_setting_values.insert(key.clone(), confirmed);
                let error_changed = state.app_setting_errors.get(&key) != Some(&message);
                state.app_setting_errors.insert(key, message);
                value_changed || error_changed
            } else {
                state
                    .app_setting_desired_values
                    .insert(key.clone(), value.clone());
                let value_changed = state.app_setting_values.get(&key) != Some(&value);
                state.app_setting_values.insert(key.clone(), value);
                value_changed || state.app_setting_errors.remove(&key).is_some()
            };
            if changed {
                state.version += 1;
                DispatchOutcome::layout()
            } else {
                DispatchOutcome::unchanged()
            }
        }
        AppEvent::EntityRevisionCommitted { key, revision } => {
            if let Some(revision) = revision {
                state.entity_revisions.insert(key, revision);
            } else {
                state.entity_revisions.remove(&key);
            }
            state.workspace_revision += 1;
            DispatchOutcome::unchanged()
        }
        AppEvent::WorkspaceRevisionCommitted => {
            state.workspace_revision += 1;
            DispatchOutcome::unchanged()
        }
        AppEvent::EntityRevisionsMerged(revisions) => {
            state.entity_revisions.extend(revisions);
            DispatchOutcome::unchanged()
        }
        AppEvent::WorkspaceRefreshed {
            snapshot,
            revision,
            entity_revisions,
        } => {
            let selected_task = state.selected_task_id.clone();
            let selected_person = state.selected_person_id.clone();
            let selected_workspace = state.selected_workspace_id.clone();
            let selected_tag = state.selected_tag_id.clone();
            state.tasks = snapshot.tasks;
            state.people = snapshot.people;
            state.workspaces = snapshot.workspaces;
            state.tags = snapshot.tags;
            state.selected_task_id = retained_selection(selected_task, &state.tasks, |v| &v.id);
            state.selected_person_id =
                retained_selection(selected_person, &state.people, |v| &v.id);
            state.selected_workspace_id =
                retained_selection(selected_workspace, &state.workspaces, |v| &v.id);
            state.selected_tag_id = retained_selection(selected_tag, &state.tags, |v| &v.id);
            state.workspace_revision = revision;
            state.entity_revisions = entity_revisions;
            state.refresh_error = None;
            state.external_refresh_version += 1;
            state.version += 1;
            DispatchOutcome::layout()
        }
        AppEvent::RefreshFailed(error) => {
            let message = format!("Workspace refresh failed: {error}");
            if state.refresh_error.as_deref() == Some(&message) {
                DispatchOutcome::unchanged()
            } else {
                state.refresh_error = Some(message);
                state.version += 1;
                DispatchOutcome::changed()
            }
        }
        AppEvent::RefreshSucceeded => {
            if state.refresh_error.take().is_some() {
                state.version += 1;
                DispatchOutcome::changed()
            } else {
                DispatchOutcome::unchanged()
            }
        }
    }
}

fn retained_selection<T>(
    selected: Option<String>,
    values: &[T],
    id: impl Fn(&T) -> &String,
) -> Option<String> {
    selected
        .filter(|selected| values.iter().any(|value| id(value) == selected))
        .or_else(|| values.first().map(id).cloned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub rank: i64,
    pub created_at: String,
    pub updated_at: String,
    pub title: String,
    pub state: TaskState,
    pub size: TaskSize,
    pub priority: TaskPriority,
    pub snoozed_until: Option<PrimitiveDateTime>,
    pub people_ids: Vec<String>,
    pub workspace_id: Option<String>,
    pub tag_ids: Vec<String>,
    pub checklist: Vec<ChecklistItem>,
    pub links: Vec<String>,
    pub relations: Vec<TaskRelation>,
    pub description: String,
}

impl Task {
    pub fn quick_capture(id: String, title: String, description: String, size: TaskSize) -> Self {
        Self {
            id,
            rank: 0,
            created_at: String::new(),
            updated_at: String::new(),
            title: title.trim().to_string(),
            state: TaskState::Backlog,
            size,
            priority: TaskPriority::Medium,
            snoozed_until: None,
            people_ids: Vec::new(),
            workspace_id: None,
            tag_ids: Vec::new(),
            checklist: Vec::new(),
            links: Vec::new(),
            relations: Vec::new(),
            description,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRelation {
    pub task_id: String,
    pub kind: TaskRelationKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskRelationKind {
    Blocks,
    IsBlockedBy,
    RelatesTo,
    Duplicates,
    IsDuplicatedBy,
}

impl TaskRelationKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::IsBlockedBy => "is_blocked_by",
            Self::RelatesTo => "relates_to",
            Self::Duplicates => "duplicates",
            Self::IsDuplicatedBy => "is_duplicated_by",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::IsBlockedBy => "is blocked by",
            Self::RelatesTo => "relates to",
            Self::Duplicates => "duplicates",
            Self::IsDuplicatedBy => "is duplicated by",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "blocks" => Some(Self::Blocks),
            "is_blocked_by" => Some(Self::IsBlockedBy),
            "relates_to" => Some(Self::RelatesTo),
            "duplicates" => Some(Self::Duplicates),
            "is_duplicated_by" => Some(Self::IsDuplicatedBy),
            _ => None,
        }
    }

    pub fn inverse(self) -> Self {
        match self {
            Self::Blocks => Self::IsBlockedBy,
            Self::IsBlockedBy => Self::Blocks,
            Self::RelatesTo => Self::RelatesTo,
            Self::Duplicates => Self::IsDuplicatedBy,
            Self::IsDuplicatedBy => Self::Duplicates,
        }
    }
}

pub(crate) fn task_identifier(number: i64, workspace_key: Option<&str>) -> String {
    workspace_key
        .filter(|key| !key.is_empty())
        .map_or_else(|| number.to_string(), |key| format!("{key}-{number}"))
}

pub(crate) fn task_number(identifier: &str) -> Option<i64> {
    if identifier.starts_with("pending-") {
        return None;
    }
    identifier
        .rsplit_once('-')
        .map_or(identifier, |(_, number)| number)
        .parse::<i64>()
        .ok()
        .filter(|number| *number > 0)
}

pub(crate) fn task_display_id(task: &Task, workspace: Option<&Workspace>) -> String {
    let number = task_number(&task.id)
        .map(|number| number.to_string())
        .unwrap_or_else(|| task.id.clone());
    workspace.map_or(number.clone(), |workspace| {
        format!("{}-{number}", workspace.key)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecklistItem {
    pub id: String,
    pub parent_id: Option<String>,
    pub text: String,
    pub checked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRank {
    pub id: String,
    pub rank: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub email: String,
    pub about: String,
    pub active: bool,
}

impl Person {
    pub fn new(id: String, name: String, email: String) -> Self {
        Self {
            id,
            name: name.trim().to_string(),
            email: email.trim().to_string(),
            about: String::new(),
            active: true,
        }
    }

    pub fn with_about(id: String, name: String, email: String, about: String) -> Self {
        let mut person = Self::new(id, name, email);
        person.about = about;
        person
    }
}

#[derive(Debug, Clone)]
pub struct PersonDeletion {
    pub person: Person,
    pub task_ids: Vec<String>,
    pub lead_workspace_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub id: String,
    pub key: String,
    pub name: String,
    pub description: String,
    pub lead_person_id: Option<String>,
}

impl Workspace {
    pub fn new(id: String, key: String, name: String, description: String) -> Self {
        Self {
            id,
            key: Self::normalize_key(&key),
            name: name.trim().to_string(),
            description,
            lead_person_id: None,
        }
    }

    pub(crate) fn normalize_key(key: &str) -> String {
        key.to_uppercase()
    }

    pub(crate) fn is_valid_key(key: &str) -> bool {
        let length = key.chars().count();
        (2..=5).contains(&length) && !key.chars().any(char::is_whitespace)
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceDeletion {
    pub workspace: Workspace,
    pub task_ids: Vec<String>,
    pub was_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub id: String,
    pub label: String,
}

impl Tag {
    pub fn new(id: String, label: String) -> Self {
        Self {
            id,
            label: label.trim().to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TagDeletion {
    pub tag: Tag,
    pub task_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskField {
    Title,
    Description,
    State,
    Size,
    Priority,
    People,
    Workspaces,
    Tags,
    Checklist,
    Links,
    Relations,
    Snooze,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PersonField {
    Name,
    Email,
    About,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkspaceField {
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
    Description(String),
    State(TaskState),
    Size(TaskSize),
    Priority(TaskPriority),
    People(Vec<String>),
    Workspace(Option<String>),
    Tags(Vec<Tag>),
    Checklist(Vec<ChecklistItem>),
    Links(Vec<String>),
    Relations(Vec<TaskRelation>),
    Snooze {
        until: PrimitiveDateTime,
        remember_custom: Option<PrimitiveDateTime>,
    },
    Unsnooze,
}

impl TaskPatch {
    pub fn field(&self) -> TaskField {
        match self {
            Self::Title(_) => TaskField::Title,
            Self::Description(_) => TaskField::Description,
            Self::State(_) => TaskField::State,
            Self::Size(_) => TaskField::Size,
            Self::Priority(_) => TaskField::Priority,
            Self::People(_) => TaskField::People,
            Self::Workspace(_) => TaskField::Workspaces,
            Self::Tags(_) => TaskField::Tags,
            Self::Checklist(_) => TaskField::Checklist,
            Self::Links(_) => TaskField::Links,
            Self::Relations(_) => TaskField::Relations,
            Self::Snooze { .. } | Self::Unsnooze => TaskField::Snooze,
        }
    }
}

#[derive(Debug, Clone)]
pub enum PersonPatch {
    Name(String),
    Email(String),
    About(String),
    Active(bool),
}

impl PersonPatch {
    pub fn field(&self) -> PersonField {
        match self {
            Self::Name(_) => PersonField::Name,
            Self::Email(_) => PersonField::Email,
            Self::About(_) => PersonField::About,
            Self::Active(_) => PersonField::Active,
        }
    }
}

#[derive(Debug, Clone)]
pub enum WorkspacePatch {
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

impl WorkspacePatch {
    pub fn field(&self) -> WorkspaceField {
        match self {
            Self::Key(_) => WorkspaceField::Key,
            Self::Name(_) => WorkspaceField::Name,
            Self::Description(_) => WorkspaceField::Description,
            Self::LeadPerson(_) => WorkspaceField::LeadPerson,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Backlog,
    Todo,
    InProgress,
    Done,
    Snoozed,
    Rejected,
}

impl TaskState {
    pub fn id(self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Snoozed => "snoozed",
            Self::Rejected => "rejected",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Backlog => "BACKLOG",
            Self::Todo => "TODO",
            Self::InProgress => "IN-PROGRESS",
            Self::Done => "DONE",
            Self::Snoozed => "SNOOZED",
            Self::Rejected => "REJECTED",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "backlog" => Some(Self::Backlog),
            "todo" => Some(Self::Todo),
            "in_progress" => Some(Self::InProgress),
            "done" => Some(Self::Done),
            "snoozed" => Some(Self::Snoozed),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }

    pub(crate) fn parse_persisted(value: &str) -> Option<Self> {
        match value {
            "clarify" | "next" => Some(Self::Todo),
            "waiting" => Some(Self::Backlog),
            "doing" => Some(Self::InProgress),
            value => Self::parse(value),
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
            Self::Small => "SMA",
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
        TaskPatch::Description(description) if task.description != *description => {
            task.description = description.clone();
            true
        }
        TaskPatch::State(value) if task.state != *value => {
            task.state = *value;
            if *value != TaskState::Snoozed {
                task.snoozed_until = None;
            }
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
        TaskPatch::People(ids) if task.people_ids != *ids => {
            task.people_ids = ids.clone();
            true
        }
        TaskPatch::Workspace(id) if task.workspace_id != *id => {
            task.workspace_id = id.clone();
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
        TaskPatch::Checklist(checklist) if task.checklist != *checklist => {
            task.checklist = checklist.clone();
            true
        }
        TaskPatch::Links(links) => {
            let mut links = links.clone();
            links.sort();
            links.dedup();
            if task.links == links {
                false
            } else {
                task.links = links;
                true
            }
        }
        TaskPatch::Relations(relations) if task.relations != *relations => {
            task.relations = relations.clone();
            true
        }
        TaskPatch::Snooze { until, .. }
            if task.state != TaskState::Snoozed || task.snoozed_until != Some(*until) =>
        {
            task.state = TaskState::Snoozed;
            task.snoozed_until = Some(*until);
            true
        }
        TaskPatch::Unsnooze if task.state != TaskState::Todo || task.snoozed_until.is_some() => {
            task.state = TaskState::Todo;
            task.snoozed_until = None;
            true
        }
        _ => false,
    }
}

fn apply_task_relations(tasks: &mut [Task], task_index: usize, relations: &[TaskRelation]) -> bool {
    let task_id = tasks[task_index].id.clone();
    let before = tasks
        .iter()
        .map(|task| task.relations.clone())
        .collect::<Vec<_>>();
    for task in tasks.iter_mut() {
        if task.id == task_id {
            task.relations.clear();
        } else {
            task.relations
                .retain(|relation| relation.task_id != task_id);
        }
    }
    for relation in relations {
        if relation.task_id == task_id || !tasks.iter().any(|task| task.id == relation.task_id) {
            continue;
        }
        if !tasks[task_index].relations.contains(relation) {
            tasks[task_index].relations.push(relation.clone());
        }
        let inverse = TaskRelation {
            task_id: task_id.clone(),
            kind: relation.kind.inverse(),
        };
        if let Some(other) = tasks.iter_mut().find(|task| task.id == relation.task_id)
            && !other.relations.contains(&inverse)
        {
            other.relations.push(inverse);
        }
    }
    before
        .iter()
        .zip(tasks.iter())
        .any(|(before, task)| before != &task.relations)
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
        PersonPatch::About(about) if person.about != *about => {
            person.about = about.clone();
            true
        }
        PersonPatch::Active(active) if person.active != *active => {
            person.active = *active;
            true
        }
        _ => false,
    }
}

fn apply_workspace_patch(workspace: &mut Workspace, patch: &WorkspacePatch) -> bool {
    match patch {
        WorkspacePatch::Key(key)
            if Workspace::is_valid_key(key) && workspace.key != Workspace::normalize_key(key) =>
        {
            workspace.key = Workspace::normalize_key(key);
            true
        }
        WorkspacePatch::Name(name) if workspace.name != name.trim() && !name.trim().is_empty() => {
            workspace.name = name.trim().to_string();
            true
        }
        WorkspacePatch::Description(description) if workspace.description != *description => {
            workspace.description = description.clone();
            true
        }
        WorkspacePatch::LeadPerson(lead_person_id)
            if workspace.lead_person_id.as_ref() != lead_person_id.as_ref() =>
        {
            workspace.lead_person_id = lead_person_id.clone();
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

    fn setting_state(value: &str) -> AppState {
        let mut state = AppState::from_snapshot(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        state
            .app_setting_values
            .insert("setting".into(), value.into());
        state
            .app_setting_confirmed_values
            .insert("setting".into(), value.into());
        state
            .app_setting_desired_values
            .insert("setting".into(), value.into());
        state
    }

    fn request_setting(state: &mut AppState, value: &str, generation: u64) {
        reduce_app_state(
            state,
            AppEvent::AppSettingChangeRequested {
                key: "setting".into(),
                value: value.into(),
                generation,
            },
        );
    }

    fn complete_setting(state: &mut AppState, value: &str, generation: u64, error: Option<&str>) {
        reduce_app_state(
            state,
            AppEvent::AppSettingSaveCompleted {
                key: "setting".into(),
                value: value.into(),
                generation,
                error: error.map(str::to_string),
            },
        );
    }

    #[test]
    fn stale_setting_failure_cannot_replace_newer_successful_toggle() {
        let mut state = setting_state("true");
        request_setting(&mut state, "false", 1);
        request_setting(&mut state, "true", 2);

        complete_setting(&mut state, "false", 1, Some("first failed"));
        assert_eq!(state.app_setting_values["setting"], "true");
        assert!(!state.app_setting_errors.contains_key("setting"));

        complete_setting(&mut state, "true", 2, None);
        assert_eq!(state.app_setting_values["setting"], "true");
        assert_eq!(state.app_setting_confirmed_values["setting"], "true");
    }

    #[test]
    fn latest_setting_failure_rolls_back_to_last_confirmed_toggle() {
        let mut state = setting_state("true");
        request_setting(&mut state, "false", 1);
        request_setting(&mut state, "true", 2);

        complete_setting(&mut state, "false", 1, None);
        assert_eq!(state.app_setting_values["setting"], "true");
        assert_eq!(state.app_setting_confirmed_values["setting"], "false");

        complete_setting(&mut state, "true", 2, Some("second failed"));
        assert_eq!(state.app_setting_values["setting"], "false");
        assert_eq!(state.app_setting_desired_values["setting"], "false");
        assert!(state.app_setting_errors.contains_key("setting"));
    }

    #[test]
    fn save_success_only_clears_matching_failed_field() {
        let mut state = AppState::from_snapshot(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
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
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let target = SaveTarget::task("T-1".to_string(), TaskField::Description);

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
    fn deleting_selected_task_clears_selection_for_the_active_view_to_replace() {
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
            workspaces: Vec::new(),
            tags: Vec::new(),
        });

        let outcome = reduce_app_state(&mut state, AppEvent::TaskDeleted("first".to_string()));

        assert!(outcome.changed);
        assert_eq!(state.tasks.len(), 1);
        assert_eq!(state.selected_task_id, None);
    }

    #[test]
    fn deleting_task_removes_reverse_relations_from_remaining_tasks() {
        let mut first = Task::quick_capture(
            "first".into(),
            "First".into(),
            String::new(),
            TaskSize::Small,
        );
        first.relations.push(TaskRelation {
            task_id: "second".into(),
            kind: TaskRelationKind::Blocks,
        });
        let mut second = Task::quick_capture(
            "second".into(),
            "Second".into(),
            String::new(),
            TaskSize::Small,
        );
        second.relations.push(TaskRelation {
            task_id: "first".into(),
            kind: TaskRelationKind::IsBlockedBy,
        });
        let mut state = AppState::from_snapshot(WorkspaceSnapshot {
            tasks: vec![first, second],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });

        reduce_app_state(&mut state, AppEvent::TaskDeleted("first".into()));

        assert!(state.tasks[0].relations.is_empty());
    }

    #[test]
    fn snooze_patch_updates_workflow_datetime_and_optional_global_last_together() {
        let task =
            Task::quick_capture("task".into(), "Task".into(), String::new(), TaskSize::Small);
        let mut state = AppState::from_snapshot(WorkspaceSnapshot {
            tasks: vec![task],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let until = time::macros::datetime!(2026-07-24 8:00);

        let outcome = reduce_app_state(
            &mut state,
            AppEvent::PatchTask {
                task_id: "task".into(),
                patch: TaskPatch::Snooze {
                    until,
                    remember_custom: Some(until),
                },
            },
        );

        assert!(outcome.changed);
        assert_eq!(state.tasks[0].state, TaskState::Snoozed);
        assert_eq!(state.tasks[0].snoozed_until, Some(until));
        assert_eq!(state.last_custom_snooze, Some(until));
    }

    #[test]
    fn unsnooze_patch_returns_task_to_todo_and_clears_datetime() {
        let until = time::macros::datetime!(2026-07-24 8:00);
        let mut task =
            Task::quick_capture("task".into(), "Task".into(), String::new(), TaskSize::Small);
        task.state = TaskState::Snoozed;
        task.snoozed_until = Some(until);
        let mut state = AppState::from_snapshot(WorkspaceSnapshot {
            tasks: vec![task],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        state.last_custom_snooze = Some(until);

        let outcome = reduce_app_state(
            &mut state,
            AppEvent::PatchTask {
                task_id: "task".into(),
                patch: TaskPatch::Unsnooze,
            },
        );

        assert!(outcome.changed);
        assert_eq!(state.tasks[0].state, TaskState::Todo);
        assert_eq!(state.tasks[0].snoozed_until, None);
        assert_eq!(state.last_custom_snooze, Some(until));
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
            workspaces: Vec::new(),
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

    #[test]
    fn management_entities_create_select_and_delete_like_tasks() {
        let mut state = AppState::from_snapshot(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: vec![Person::new("person-1".into(), "Ada".into(), String::new())],
            workspaces: vec![Workspace::new(
                "workspace-1".into(),
                "CORE".into(),
                "Core".into(),
                String::new(),
            )],
            tags: vec![Tag::new("tag-1".into(), "api".into())],
        });

        reduce_app_state(
            &mut state,
            AppEvent::PersonCreated(Person::new(
                "person-2".into(),
                "Grace".into(),
                String::new(),
            )),
        );
        reduce_app_state(
            &mut state,
            AppEvent::WorkspaceCreated(Workspace::new(
                "workspace-2".into(),
                "APP".into(),
                "App".into(),
                String::new(),
            )),
        );
        reduce_app_state(
            &mut state,
            AppEvent::TagCreated(Tag::new("tag-2".into(), "frontend".into())),
        );

        assert_eq!(state.selected_person_id.as_deref(), Some("person-2"));
        assert_eq!(state.selected_workspace_id.as_deref(), Some("workspace-2"));
        assert_eq!(state.selected_tag_id.as_deref(), Some("tag-2"));

        reduce_app_state(&mut state, AppEvent::PersonDeleted("person-2".into()));
        reduce_app_state(&mut state, AppEvent::WorkspaceDeleted("workspace-2".into()));
        reduce_app_state(&mut state, AppEvent::TagDeleted("tag-2".into()));

        assert_eq!(state.selected_person_id.as_deref(), Some("person-1"));
        assert_eq!(state.selected_workspace_id.as_deref(), Some("workspace-1"));
        assert_eq!(state.selected_tag_id.as_deref(), Some("tag-1"));
    }

    #[test]
    fn workspace_keys_are_uppercase_after_creation() {
        let workspace = Workspace::new(
            "workspace-1".into(),
            "core".into(),
            "Core".into(),
            String::new(),
        );
        assert_eq!(workspace.key, "CORE");
        assert!(Workspace::is_valid_key("AB"));
        assert!(Workspace::is_valid_key("ABCDE"));
        for invalid in ["A", "ABCDEF", "A B", " AB", "AB "] {
            assert!(!Workspace::is_valid_key(invalid), "accepted {invalid:?}");
        }
    }

    #[test]
    fn person_delete_clears_and_failed_delete_restores_references() {
        let mut workspace = Workspace::new(
            "workspace-1".into(),
            "CORE".into(),
            "Core".into(),
            String::new(),
        );
        workspace.lead_person_id = Some("person-1".into());
        let mut state = AppState::from_snapshot(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: vec![Person::new("person-1".into(), "Ada".into(), String::new())],
            workspaces: vec![workspace],
            tags: Vec::new(),
        });

        let deletion = state.person_deletion("person-1").unwrap();
        reduce_app_state(&mut state, AppEvent::PersonDeleted("person-1".into()));

        assert_eq!(state.workspaces[0].lead_person_id, None);

        reduce_app_state(&mut state, AppEvent::PersonRestored(deletion));

        assert_eq!(
            state.workspaces[0].lead_person_id.as_deref(),
            Some("person-1")
        );
    }
}
#[test]
fn completing_snoozed_task_clears_snooze_timestamp() {
    let mut task =
        Task::quick_capture("task".into(), "Task".into(), String::new(), TaskSize::Small);
    task.state = TaskState::Snoozed;
    task.snoozed_until = Some(time::macros::datetime!(2026-07-25 08:00));
    let mut state = AppState::from_snapshot(WorkspaceSnapshot {
        tasks: vec![task],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });

    let outcome = reduce_app_state(
        &mut state,
        AppEvent::PatchTask {
            task_id: "task".into(),
            patch: TaskPatch::State(TaskState::Done),
        },
    );

    assert!(outcome.changed);
    assert_eq!(state.tasks[0].state, TaskState::Done);
    assert_eq!(state.tasks[0].snoozed_until, None);
}
