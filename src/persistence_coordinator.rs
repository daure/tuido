use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

use sqlx::{AnyPool, postgres::PgListener};
use tokio::runtime::Handle;
use tuicore::Store;

use crate::{
    domain::{
        AppEvent, AppState, Person, PersonDeletion, PersonPatch, Project, ProjectDeletion,
        ProjectPatch, SaveTarget, Tag, TagDeletion, TagPatch, Task, TaskField, TaskPatch,
    },
    service::TuidoService,
    storage::SqlDialect,
};

pub(crate) type AppStore =
    Rc<RefCell<Store<AppState, AppEvent, fn(&mut AppState, AppEvent) -> tuicore::DispatchOutcome>>>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CommandKey {
    Task,
    Person(String),
    Project(String),
    Tag(String),
}

#[derive(Debug, Clone)]
pub(crate) enum PersistenceCommand {
    CreateTask(Task),
    DeleteTask(Task),
    PatchTask(String, TaskPatch),
    CreatePerson(Person),
    DeletePerson(PersonDeletion),
    PatchPerson(String, PersonPatch),
    CreateProject(Project),
    DeleteProject(ProjectDeletion),
    PatchProject(String, ProjectPatch),
    CreateTag(Tag),
    DeleteTag(TagDeletion),
    PatchTag(String, TagPatch),
}

impl PersistenceCommand {
    fn key(&self) -> CommandKey {
        match self {
            Self::CreateTask(_) | Self::DeleteTask(_) | Self::PatchTask(_, _) => CommandKey::Task,
            Self::CreatePerson(person) => CommandKey::Person(person.id.clone()),
            Self::DeletePerson(deletion) => CommandKey::Person(deletion.person.id.clone()),
            Self::PatchPerson(id, _) => CommandKey::Person(id.clone()),
            Self::CreateProject(project) => CommandKey::Project(project.id.clone()),
            Self::DeleteProject(deletion) => CommandKey::Project(deletion.project.id.clone()),
            Self::PatchProject(id, _) => CommandKey::Project(id.clone()),
            Self::CreateTag(tag) => CommandKey::Tag(tag.id.clone()),
            Self::DeleteTag(deletion) => CommandKey::Tag(deletion.tag.id.clone()),
            Self::PatchTag(id, _) => CommandKey::Tag(id.clone()),
        }
    }
}

struct Completion {
    key: CommandKey,
    sequence: u64,
    command: PersistenceCommand,
    error: Option<String>,
    related_revisions: HashMap<String, u64>,
}

struct RefreshCompletion {
    revision: u64,
    snapshot: Option<crate::domain::WorkspaceSnapshot>,
    revisions: HashMap<String, u64>,
}

pub(crate) struct PersistenceCoordinator {
    store: AppStore,
    service: TuidoService,
    runtime: Handle,
    completion_tx: mpsc::Sender<Completion>,
    completion_rx: mpsc::Receiver<Completion>,
    active: HashMap<CommandKey, u64>,
    active_expected: HashMap<u64, Option<u64>>,
    queued: HashMap<CommandKey, VecDeque<PersistenceCommand>>,
    next_sequence: u64,
    refresh_tx: mpsc::Sender<Result<RefreshCompletion, String>>,
    refresh_rx: mpsc::Receiver<Result<RefreshCompletion, String>>,
    notification_rx: mpsc::Receiver<()>,
    change_notified: bool,
    refresh_inflight: bool,
    reconcile_required: bool,
    last_refresh_check: Instant,
}

impl PersistenceCoordinator {
    pub(crate) fn new(
        store: AppStore,
        pool: AnyPool,
        dialect: SqlDialect,
        runtime: Handle,
        notification_url: Option<String>,
    ) -> Self {
        let (completion_tx, completion_rx) = mpsc::channel();
        let (refresh_tx, refresh_rx) = mpsc::channel();
        let (notification_tx, notification_rx) = mpsc::channel();
        if let Some(database_url) = notification_url {
            spawn_postgres_listener(&runtime, database_url, notification_tx);
        }
        Self {
            store,
            service: TuidoService::from_parts(pool, dialect),
            runtime,
            completion_tx,
            completion_rx,
            active: HashMap::new(),
            active_expected: HashMap::new(),
            queued: HashMap::new(),
            next_sequence: 0,
            refresh_tx,
            refresh_rx,
            notification_rx,
            change_notified: false,
            refresh_inflight: false,
            reconcile_required: false,
            last_refresh_check: Instant::now(),
        }
    }

    pub(crate) fn submit(&mut self, mut command: PersistenceCommand) {
        let key = command.key();
        if self.active.contains_key(&key) {
            let queue = self.queued.entry(key).or_default();
            let patch_field = match &command {
                PersistenceCommand::PatchTask(id, patch) => {
                    Some((id.as_str(), coalesce_task_field(patch)))
                }
                _ => None,
            };
            let replace_index = patch_field.and_then(|(task_id, field)| {
                queue
                    .iter()
                    .enumerate()
                    .rev()
                    .take_while(|(_, queued)| {
                        matches!(queued, PersistenceCommand::PatchTask(_, _))
                            && !remembered_custom_for_other_task(queued, task_id)
                    })
                    .find_map(|(index, queued)| match queued {
                        PersistenceCommand::PatchTask(queued_id, patch)
                            if queued_id == task_id
                                && coalesce_task_field(patch) == field
                                && can_supersede_task_patch(patch, &command) =>
                        {
                            Some(index)
                        }
                        _ => None,
                    })
            });
            if let Some(index) = replace_index {
                let removed = queue.remove(index).expect("queued command index is valid");
                merge_remembered_custom(&removed, &mut command);
            }
            queue.push_back(command);
        } else {
            self.start(command);
        }
    }

    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        while self.notification_rx.try_recv().is_ok() {
            self.change_notified = true;
        }
        while let Ok(completion) = self.completion_rx.try_recv() {
            changed |= self.finish(completion);
        }
        while let Ok(result) = self.refresh_rx.try_recv() {
            self.refresh_inflight = false;
            match result {
                Ok(refresh) if !self.has_pending() => {
                    let current = self.store.borrow().state().workspace_revision;
                    match refresh.snapshot {
                        None if refresh.revision < current => {
                            self.reconcile_required = true;
                        }
                        None => {
                            changed |= self
                                .store
                                .borrow_mut()
                                .dispatch(AppEvent::RefreshSucceeded)
                                .changed;
                        }
                        Some(_) if refresh.revision < current => {
                            self.reconcile_required = true;
                        }
                        Some(snapshot) => {
                            self.reconcile_required = false;
                            changed |= self
                                .store
                                .borrow_mut()
                                .dispatch(AppEvent::WorkspaceRefreshed {
                                    snapshot,
                                    revision: refresh.revision,
                                    entity_revisions: refresh.revisions,
                                })
                                .changed;
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    changed |= self
                        .store
                        .borrow_mut()
                        .dispatch(AppEvent::RefreshFailed(error))
                        .changed;
                }
            }
        }
        if !self.refresh_inflight
            && !self.has_pending()
            && (self.change_notified
                || self.reconcile_required
                || self.last_refresh_check.elapsed() >= Duration::from_millis(500))
        {
            self.start_refresh();
        }
        changed
    }

    fn start_refresh(&mut self) {
        self.refresh_inflight = true;
        self.change_notified = false;
        self.last_refresh_check = Instant::now();
        let service = self.service.clone();
        let current = self.store.borrow().state().workspace_revision;
        let force_snapshot = self.reconcile_required;
        let tx = self.refresh_tx.clone();
        self.runtime.spawn(async move {
            let result = async {
                let observed = service.workspace_revision().await?;
                if !force_snapshot && observed <= current {
                    return Ok(RefreshCompletion {
                        revision: observed,
                        snapshot: None,
                        revisions: HashMap::new(),
                    });
                }
                let workspace = service.consistent_workspace().await?;
                Ok(RefreshCompletion {
                    revision: workspace.revision,
                    snapshot: Some(workspace.snapshot),
                    revisions: workspace.entity_revisions,
                })
            }
            .await
            .map_err(|e: crate::service::ServiceError| e.to_string());
            let _ = tx.send(result);
        });
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self.active.is_empty() || !self.queued.is_empty()
    }

    pub(crate) fn drain(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.has_pending() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match self.completion_rx.recv_timeout(remaining) {
                Ok(completion) => {
                    self.finish(completion);
                }
                Err(_) => return false,
            }
        }
        true
    }

    fn start(&mut self, command: PersistenceCommand) {
        let key = command.key();
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.active.insert(key.clone(), sequence);
        let service = self.service.clone();
        let expected_revision = command_entity(&command).and_then(|(kind, id)| {
            self.store
                .borrow()
                .state()
                .entity_revisions
                .get(&format!("{kind}:{id}"))
                .copied()
        });
        self.active_expected.insert(sequence, expected_revision);
        let tx = self.completion_tx.clone();
        self.runtime.spawn(async move {
            let result = execute(service, command.clone(), expected_revision).await;
            let (error, related_revisions) = match result {
                Ok(revisions) => (None, revisions),
                Err(error) => (Some(error.to_string()), HashMap::new()),
            };
            let _ = tx.send(Completion {
                key,
                sequence,
                command,
                error,
                related_revisions,
            });
        });
    }

    fn finish(&mut self, completion: Completion) -> bool {
        if self.active.get(&completion.key).copied() != Some(completion.sequence) {
            return false;
        }
        self.active.remove(&completion.key);
        let committed_expected = self.active_expected.remove(&completion.sequence).flatten();
        if completion.error.is_none()
            && let Some((key, revision)) = revision_update(&completion.command, committed_expected)
        {
            self.store
                .borrow_mut()
                .dispatch(AppEvent::EntityRevisionCommitted { key, revision });
        }
        if completion.error.is_none() {
            let cascade_revisions = cascade_revision_updates(
                &completion.command,
                &self.store.borrow().state().entity_revisions,
            );
            if !cascade_revisions.is_empty() {
                self.store
                    .borrow_mut()
                    .dispatch(AppEvent::EntityRevisionsMerged(cascade_revisions));
            }
        }
        if !completion.related_revisions.is_empty() {
            self.store
                .borrow_mut()
                .dispatch(AppEvent::EntityRevisionsMerged(
                    completion.related_revisions.clone(),
                ));
        }
        if completion.error.is_some() {
            self.reconcile_required = true;
            preserve_failed_active_custom(
                &completion.command,
                self.queued.get_mut(&completion.key),
            );
        }
        let task_patch_is_superseded = match &completion.command {
            PersistenceCommand::PatchTask(id, patch) => {
                self.queued.get(&completion.key).is_some_and(|queue| {
                    queue
                        .iter()
                        .take_while(|command| {
                            matches!(command, PersistenceCommand::PatchTask(_, _))
                                && !remembered_custom_for_other_task(command, id)
                        })
                        .any(|command| {
                            let PersistenceCommand::PatchTask(queued_id, queued_patch) = command
                            else {
                                return false;
                            };
                            queued_id == id
                                && coalesce_task_field(queued_patch) == coalesce_task_field(patch)
                                && can_supersede_task_patch(patch, command)
                        })
                })
            }
            _ => false,
        };
        let mut changed = false;
        match completion.command {
            PersistenceCommand::CreateTask(task) => {
                if completion.error.is_some() {
                    let task_id = task.id.clone();
                    changed |= self
                        .store
                        .borrow_mut()
                        .dispatch(AppEvent::TaskDeleted(task.id))
                        .changed;
                    remove_queued_task_commands(self.queued.get_mut(&completion.key), &task_id);
                }
            }
            PersistenceCommand::DeleteTask(task) => match completion.error {
                Some(_) => {
                    changed |= self
                        .store
                        .borrow_mut()
                        .dispatch(AppEvent::TaskCreated(task))
                        .changed;
                }
                None => {
                    if let Some(queue) = self.queued.get_mut(&completion.key) {
                        queue.retain(|command| {
                            !matches!(command, PersistenceCommand::PatchTask(id, _) if id == &task.id)
                        });
                    }
                }
            },
            PersistenceCommand::CreatePerson(person) => {
                if completion.error.is_some() {
                    changed |= self
                        .store
                        .borrow_mut()
                        .dispatch(AppEvent::PersonDeleted(person.id))
                        .changed;
                    self.queued.remove(&completion.key);
                }
            }
            PersistenceCommand::DeletePerson(deletion) => match completion.error {
                Some(_) => {
                    changed |= self
                        .store
                        .borrow_mut()
                        .dispatch(AppEvent::PersonRestored(deletion))
                        .changed;
                }
                None => {
                    self.queued.remove(&completion.key);
                }
            },
            PersistenceCommand::CreateProject(project) => {
                if completion.error.is_some() {
                    changed |= self
                        .store
                        .borrow_mut()
                        .dispatch(AppEvent::ProjectDeleted(project.id))
                        .changed;
                    self.queued.remove(&completion.key);
                }
            }
            PersistenceCommand::DeleteProject(deletion) => match completion.error {
                Some(_) => {
                    changed |= self
                        .store
                        .borrow_mut()
                        .dispatch(AppEvent::ProjectRestored(deletion))
                        .changed;
                }
                None => {
                    self.queued.remove(&completion.key);
                }
            },
            PersistenceCommand::CreateTag(tag) => {
                if completion.error.is_some() {
                    changed |= self
                        .store
                        .borrow_mut()
                        .dispatch(AppEvent::TagDeleted(tag.id))
                        .changed;
                    self.queued.remove(&completion.key);
                }
            }
            PersistenceCommand::DeleteTag(deletion) => match completion.error {
                Some(_) => {
                    changed |= self
                        .store
                        .borrow_mut()
                        .dispatch(AppEvent::TagRestored(deletion))
                        .changed;
                }
                None => {
                    self.queued.remove(&completion.key);
                }
            },
            PersistenceCommand::PatchTask(id, patch) => {
                if !task_patch_is_superseded {
                    changed |= self
                        .store
                        .borrow_mut()
                        .dispatch(AppEvent::SaveCompleted {
                            target: SaveTarget::task(id, patch.field()),
                            error: completion.error,
                        })
                        .changed;
                }
            }
            PersistenceCommand::PatchPerson(id, patch) => {
                changed |= self
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::SaveCompleted {
                        target: SaveTarget::person(id, patch.field()),
                        error: completion.error,
                    })
                    .changed;
            }
            PersistenceCommand::PatchProject(id, patch) => {
                changed |= self
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::SaveCompleted {
                        target: SaveTarget::project(id, patch.field()),
                        error: completion.error,
                    })
                    .changed;
            }
            PersistenceCommand::PatchTag(id, patch) => {
                changed |= self
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::SaveCompleted {
                        target: SaveTarget::tag(id, patch.field()),
                        error: completion.error,
                    })
                    .changed;
            }
        }

        let next = self
            .queued
            .get_mut(&completion.key)
            .and_then(VecDeque::pop_front);
        if self
            .queued
            .get(&completion.key)
            .is_some_and(VecDeque::is_empty)
        {
            self.queued.remove(&completion.key);
        }
        if let Some(command) = next {
            self.start(command);
        }
        changed
    }
}

fn spawn_postgres_listener(
    runtime: &Handle,
    database_url: String,
    notification_tx: mpsc::Sender<()>,
) {
    runtime.spawn(async move {
        loop {
            let result = async {
                let mut listener = PgListener::connect(&database_url).await?;
                listener.listen("tuido_changes").await?;
                loop {
                    listener.recv().await?;
                    if notification_tx.send(()).is_err() {
                        return Ok::<(), sqlx::Error>(());
                    }
                }
            }
            .await;
            if result.is_ok() || notification_tx.send(()).is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}

async fn execute(
    service: TuidoService,
    command: PersistenceCommand,
    expected_revision: Option<u64>,
) -> Result<HashMap<String, u64>, Box<dyn std::error::Error + Send + Sync>> {
    let expected = || -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        expected_revision.ok_or_else(|| "missing entity revision; refresh required".into())
    };
    let related_revisions = match command {
        PersistenceCommand::CreateTask(task) => service
            .create_task_entity(task)
            .await
            .map(|_| HashMap::new())
            .map_err(boxed_service_error),
        PersistenceCommand::DeleteTask(task) => service
            .delete_task(&task.id, expected()?)
            .await
            .map(|_| HashMap::new())
            .map_err(boxed_service_error),
        PersistenceCommand::CreatePerson(person) => service
            .create_person_entity(person)
            .await
            .map(|_| HashMap::new())
            .map_err(boxed_service_error),
        PersistenceCommand::DeletePerson(deletion) => service
            .delete_person(&deletion.person.id, expected()?)
            .await
            .map(|_| HashMap::new())
            .map_err(boxed_service_error),
        PersistenceCommand::CreateProject(project) => service
            .create_project_entity(project)
            .await
            .map(|_| HashMap::new())
            .map_err(boxed_service_error),
        PersistenceCommand::DeleteProject(deletion) => service
            .delete_project(&deletion.project.id, expected()?)
            .await
            .map(|_| HashMap::new())
            .map_err(boxed_service_error),
        PersistenceCommand::CreateTag(tag) => service
            .create_tag_entity(tag)
            .await
            .map(|_| HashMap::new())
            .map_err(boxed_service_error),
        PersistenceCommand::DeleteTag(deletion) => service
            .delete_tag(&deletion.tag.id, expected()?)
            .await
            .map(|_| HashMap::new())
            .map_err(boxed_service_error),
        PersistenceCommand::PatchTask(id, patch) => service
            .patch_task(id, expected()?, patch)
            .await
            .map(|result| result.related_revisions)
            .map_err(boxed_service_error),
        PersistenceCommand::PatchPerson(id, patch) => service
            .patch_person(id, expected()?, patch)
            .await
            .map(|_| HashMap::new())
            .map_err(boxed_service_error),
        PersistenceCommand::PatchProject(id, patch) => service
            .patch_project(id, expected()?, patch)
            .await
            .map(|_| HashMap::new())
            .map_err(boxed_service_error),
        PersistenceCommand::PatchTag(id, patch) => service
            .patch_tag(id, expected()?, patch)
            .await
            .map(|_| HashMap::new())
            .map_err(boxed_service_error),
    }?;
    Ok(related_revisions)
}

fn boxed_service_error(
    error: crate::service::ServiceError,
) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(error)
}

fn command_entity(command: &PersistenceCommand) -> Option<(&'static str, &str)> {
    match command {
        PersistenceCommand::CreateTask(_)
        | PersistenceCommand::CreatePerson(_)
        | PersistenceCommand::CreateProject(_)
        | PersistenceCommand::CreateTag(_) => None,
        PersistenceCommand::DeleteTask(v) => Some(("task", &v.id)),
        PersistenceCommand::PatchTask(id, _) => Some(("task", id)),
        PersistenceCommand::DeletePerson(v) => Some(("person", &v.person.id)),
        PersistenceCommand::PatchPerson(id, _) => Some(("person", id)),
        PersistenceCommand::DeleteProject(v) => Some(("project", &v.project.id)),
        PersistenceCommand::PatchProject(id, _) => Some(("project", id)),
        PersistenceCommand::DeleteTag(v) => Some(("tag", &v.tag.id)),
        PersistenceCommand::PatchTag(id, _) => Some(("tag", id)),
    }
}

fn revision_update(
    command: &PersistenceCommand,
    expected: Option<u64>,
) -> Option<(String, Option<u64>)> {
    if let Some((kind, id)) = command_entity(command) {
        let key = format!("{kind}:{id}");
        if matches!(
            command,
            PersistenceCommand::DeleteTask(_)
                | PersistenceCommand::DeletePerson(_)
                | PersistenceCommand::DeleteProject(_)
                | PersistenceCommand::DeleteTag(_)
        ) {
            Some((key, None))
        } else {
            expected.map(|expected| (key, Some(expected + 1)))
        }
    } else {
        let (kind, id) = match command {
            PersistenceCommand::CreateTask(v) => ("task", v.id.as_str()),
            PersistenceCommand::CreatePerson(v) => ("person", v.id.as_str()),
            PersistenceCommand::CreateProject(v) => ("project", v.id.as_str()),
            PersistenceCommand::CreateTag(v) => ("tag", v.id.as_str()),
            _ => return None,
        };
        Some((format!("{kind}:{id}"), Some(1)))
    }
}

fn cascade_revision_updates(
    command: &PersistenceCommand,
    current: &HashMap<String, u64>,
) -> HashMap<String, u64> {
    let mut keys = Vec::new();
    match command {
        PersistenceCommand::DeletePerson(deletion) => {
            keys.extend(deletion.task_ids.iter().map(|id| format!("task:{id}")));
            keys.extend(
                deletion
                    .lead_project_ids
                    .iter()
                    .map(|id| format!("project:{id}")),
            );
        }
        PersistenceCommand::DeleteProject(deletion) => {
            keys.extend(deletion.task_ids.iter().map(|id| format!("task:{id}")));
        }
        PersistenceCommand::DeleteTag(deletion) => {
            keys.extend(deletion.task_ids.iter().map(|id| format!("task:{id}")));
        }
        _ => {}
    }
    keys.into_iter()
        .filter_map(|key| current.get(&key).map(|revision| (key, revision + 1)))
        .collect()
}

fn coalesce_task_field(patch: &TaskPatch) -> TaskField {
    match patch.field() {
        TaskField::Snooze => TaskField::State,
        field => field,
    }
}

fn can_supersede_task_patch(queued: &TaskPatch, replacement: &PersistenceCommand) -> bool {
    !matches!(
        (queued, replacement),
        (
            TaskPatch::Snooze {
                remember_custom: Some(_),
                ..
            },
            PersistenceCommand::PatchTask(_, TaskPatch::State(_) | TaskPatch::Unsnooze)
        )
    )
}

fn merge_remembered_custom(removed: &PersistenceCommand, replacement: &mut PersistenceCommand) {
    let PersistenceCommand::PatchTask(
        _,
        TaskPatch::Snooze {
            remember_custom: Some(custom),
            ..
        },
    ) = removed
    else {
        return;
    };
    if let PersistenceCommand::PatchTask(
        _,
        TaskPatch::Snooze {
            remember_custom, ..
        },
    ) = replacement
        && remember_custom.is_none()
    {
        *remember_custom = Some(*custom);
    }
}

fn remembered_custom_for_other_task(command: &PersistenceCommand, task_id: &str) -> bool {
    matches!(
        command,
        PersistenceCommand::PatchTask(
            queued_id,
            TaskPatch::Snooze {
                remember_custom: Some(_),
                ..
            }
        ) if queued_id != task_id
    )
}

fn preserve_failed_active_custom(
    active: &PersistenceCommand,
    queue: Option<&mut VecDeque<PersistenceCommand>>,
) {
    let PersistenceCommand::PatchTask(
        task_id,
        TaskPatch::Snooze {
            remember_custom: Some(custom),
            ..
        },
    ) = active
    else {
        return;
    };
    let Some(queue) = queue else { return };
    let candidate = queue
        .iter()
        .enumerate()
        .take_while(|(_, command)| !remembered_custom_for_other_task(command, task_id))
        .filter_map(|(index, command)| {
            matches!(command, PersistenceCommand::PatchTask(id, TaskPatch::Snooze { .. }) if id == task_id)
                .then_some(index)
        })
        .last();
    if let Some(PersistenceCommand::PatchTask(
        _,
        TaskPatch::Snooze {
            remember_custom, ..
        },
    )) = candidate.and_then(|index| queue.get_mut(index))
        && remember_custom.is_none()
    {
        *remember_custom = Some(*custom);
    }
}

fn remove_queued_task_commands(queue: Option<&mut VecDeque<PersistenceCommand>>, task_id: &str) {
    if let Some(queue) = queue {
        queue.retain(|command| command_task_id(command) != Some(task_id));
    }
}

fn command_task_id(command: &PersistenceCommand) -> Option<&str> {
    match command {
        PersistenceCommand::CreateTask(task) | PersistenceCommand::DeleteTask(task) => {
            Some(&task.id)
        }
        PersistenceCommand::PatchTask(id, _) => Some(id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Person, Project, Tag, TaskSize, WorkspaceSnapshot, reduce_app_state};
    use sqlx::{Row, any::AnyPoolOptions};

    fn test_task(id: &str) -> Task {
        Task::quick_capture(
            id.to_string(),
            "Original".to_string(),
            String::new(),
            TaskSize::Small,
        )
    }

    fn test_store(tasks: Vec<Task>) -> AppStore {
        let revisions = tasks
            .iter()
            .map(|task| (format!("task:{}", task.id), 1))
            .collect();
        let mut state = AppState::from_snapshot(WorkspaceSnapshot {
            tasks,
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        state.entity_revisions = revisions;
        Rc::new(RefCell::new(Store::new(
            state,
            reduce_app_state as fn(&mut AppState, AppEvent) -> tuicore::DispatchOutcome,
        )))
    }

    fn test_database() -> (tokio::runtime::Runtime, AnyPool) {
        sqlx::any::install_default_drivers();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime builds");
        let pool = runtime
            .block_on(
                AnyPoolOptions::new()
                    .max_connections(1)
                    .connect("sqlite::memory:"),
            )
            .expect("database connects");
        runtime
            .block_on(sqlx::migrate!().run(&pool))
            .expect("migrations run");
        (runtime, pool)
    }

    fn test_coordinator(
        runtime: &tokio::runtime::Runtime,
        pool: &AnyPool,
        store: AppStore,
    ) -> PersistenceCoordinator {
        PersistenceCoordinator::new(
            store,
            pool.clone(),
            SqlDialect::Sqlite,
            runtime.handle().clone(),
            None,
        )
    }

    fn persist_task(runtime: &tokio::runtime::Runtime, pool: &AnyPool, task: Task) {
        runtime
            .block_on(
                TuidoService::from_parts(pool.clone(), SqlDialect::Sqlite).create_task_entity(task),
            )
            .expect("task creates through service");
    }

    fn settle(coordinator: &mut PersistenceCoordinator) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while coordinator.has_pending()
            || coordinator.reconcile_required
            || coordinator.refresh_inflight
        {
            assert!(Instant::now() < deadline, "coordinator settles");
            coordinator.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn create_finishes_before_queued_patch() {
        let (runtime, pool) = test_database();
        let task = test_task("task-1");
        let mut coordinator = test_coordinator(&runtime, &pool, test_store(vec![task.clone()]));

        coordinator.submit(PersistenceCommand::CreateTask(task));
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".to_string(),
            TaskPatch::Title("Patched".to_string()),
        ));

        assert!(coordinator.drain(Duration::from_secs(2)));
        let title: String = runtime
            .block_on(sqlx::query("SELECT title FROM tasks WHERE id = 'task-1'").fetch_one(&pool))
            .expect("task reloads")
            .try_get("title")
            .expect("title decodes");
        assert_eq!(title, "Patched");
    }

    #[test]
    fn stale_refresh_cannot_replace_newer_local_workspace() {
        let (runtime, pool) = test_database();
        let latest = test_task("latest");
        let store = test_store(vec![latest.clone()]);
        for revision in 1..=2 {
            store
                .borrow_mut()
                .dispatch(AppEvent::EntityRevisionCommitted {
                    key: latest.id.clone(),
                    revision: Some(revision),
                });
        }
        let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));
        coordinator
            .refresh_tx
            .send(Ok(RefreshCompletion {
                snapshot: Some(WorkspaceSnapshot {
                    tasks: vec![test_task("stale")],
                    people: Vec::new(),
                    projects: Vec::new(),
                    tags: Vec::new(),
                }),
                revision: 1,
                revisions: HashMap::new(),
            }))
            .unwrap();

        coordinator.poll();

        let state = store.borrow();
        assert_eq!(state.state().workspace_revision, 2);
        assert_eq!(state.state().tasks[0].id, "latest");
        assert_eq!(state.state().selected_task_id.as_deref(), Some("latest"));
    }

    #[test]
    fn unchanged_revision_poll_does_not_load_workspace_snapshot() {
        let (runtime, pool) = test_database();
        let store = test_store(Vec::new());
        store
            .borrow_mut()
            .dispatch(AppEvent::EntityRevisionCommitted {
                key: "workspace".into(),
                revision: None,
            });
        runtime
            .block_on(sqlx::query("DROP TABLE tasks").execute(&pool))
            .unwrap();
        let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));

        coordinator.start_refresh();
        let deadline = Instant::now() + Duration::from_secs(2);
        while coordinator.refresh_inflight {
            assert!(Instant::now() < deadline, "revision check completes");
            coordinator.poll();
            std::thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(store.borrow().state().refresh_error, None);
    }

    #[test]
    fn database_notification_starts_revision_check_without_waiting_for_interval() {
        let (runtime, pool) = test_database();
        let store = test_store(Vec::new());
        let mut coordinator = test_coordinator(&runtime, &pool, store);
        coordinator.change_notified = true;
        coordinator.last_refresh_check = Instant::now();

        coordinator.poll();

        assert!(coordinator.refresh_inflight);
        assert!(!coordinator.change_notified);
    }

    #[test]
    fn stale_patch_reloads_authoritative_task_after_pending_write_drains() {
        let (runtime, pool) = test_database();
        let task = test_task("task-1");
        persist_task(&runtime, &pool, task.clone());
        let store = test_store(vec![task]);
        let service = TuidoService::from_parts(pool.clone(), SqlDialect::Sqlite);
        runtime
            .block_on(service.patch_task(
                "task-1".into(),
                1,
                TaskPatch::Title("External winner".into()),
            ))
            .unwrap();
        store.borrow_mut().dispatch(AppEvent::PatchTask {
            task_id: "task-1".into(),
            patch: TaskPatch::Title("Optimistic loser".into()),
        });
        let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));

        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::Title("Optimistic loser".into()),
        ));
        settle(&mut coordinator);

        let state = store.borrow();
        assert_eq!(state.state().tasks[0].title, "External winner");
        assert!(state.state().task_save_error("task-1").is_some());
        assert_eq!(state.state().entity_revisions["task:task-1"], 2);
    }

    #[test]
    fn invalid_snoozed_state_patch_reloads_authoritative_task() {
        let (runtime, pool) = test_database();
        let task = test_task("task-1");
        persist_task(&runtime, &pool, task.clone());
        let store = test_store(vec![task]);
        store.borrow_mut().dispatch(AppEvent::PatchTask {
            task_id: "task-1".into(),
            patch: TaskPatch::State(crate::domain::TaskState::Snoozed),
        });
        let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));

        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::State(crate::domain::TaskState::Snoozed),
        ));
        settle(&mut coordinator);

        let state = store.borrow();
        assert_eq!(state.state().tasks[0].state, crate::domain::TaskState::Todo);
        assert!(state.state().task_save_error("task-1").is_some());
    }

    #[test]
    fn snoozed_to_done_success_clears_optimistic_and_persisted_timestamp() {
        let (runtime, pool) = test_database();
        let mut task = test_task("task-1");
        task.state = crate::domain::TaskState::Snoozed;
        task.snoozed_until = Some(time::macros::datetime!(2026-07-25 08:00));
        persist_task(&runtime, &pool, task.clone());
        let store = test_store(vec![task]);
        store.borrow_mut().dispatch(AppEvent::PatchTask {
            task_id: "task-1".into(),
            patch: TaskPatch::State(crate::domain::TaskState::Done),
        });
        let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));

        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::State(crate::domain::TaskState::Done),
        ));
        assert!(coordinator.drain(Duration::from_secs(2)));

        {
            let state = store.borrow();
            let optimistic = &state.state().tasks[0];
            assert_eq!(optimistic.state, crate::domain::TaskState::Done);
            assert_eq!(optimistic.snoozed_until, None);
        }
        let persisted = runtime
            .block_on(TuidoService::from_parts(pool, SqlDialect::Sqlite).get_task("task-1"))
            .unwrap();
        assert_eq!(persisted.value.state, "done");
        assert_eq!(persisted.value.snoozed_until, None);
    }

    #[test]
    fn refresh_failure_is_visible_and_success_clears_it() {
        let (runtime, pool) = test_database();
        let store = test_store(Vec::new());
        let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));
        coordinator
            .refresh_tx
            .send(Err("database unavailable".into()))
            .unwrap();

        assert!(coordinator.poll());
        assert_eq!(
            store.borrow().state().refresh_error.as_deref(),
            Some("Workspace refresh failed: database unavailable")
        );

        coordinator
            .refresh_tx
            .send(Ok(RefreshCompletion {
                snapshot: Some(WorkspaceSnapshot {
                    tasks: Vec::new(),
                    people: Vec::new(),
                    projects: Vec::new(),
                    tags: Vec::new(),
                }),
                revision: 1,
                revisions: HashMap::new(),
            }))
            .unwrap();
        assert!(coordinator.poll());
        assert_eq!(store.borrow().state().refresh_error, None);
    }

    #[test]
    fn duplicate_tag_patch_reloads_authoritative_label() {
        let (runtime, pool) = test_database();
        let service = TuidoService::from_parts(pool.clone(), SqlDialect::Sqlite);
        let first = Tag::new("tag-1".into(), "api".into());
        let second = Tag::new("tag-2".into(), "backend".into());
        runtime
            .block_on(service.create_tag_entity(first.clone()))
            .unwrap();
        runtime
            .block_on(service.create_tag_entity(second.clone()))
            .unwrap();
        let mut state = AppState::from_snapshot(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: vec![first, second],
        });
        state.entity_revisions = HashMap::from([("tag:tag-1".into(), 1), ("tag:tag-2".into(), 1)]);
        let store = Rc::new(RefCell::new(Store::new(
            state,
            reduce_app_state as fn(&mut AppState, AppEvent) -> tuicore::DispatchOutcome,
        )));
        store.borrow_mut().dispatch(AppEvent::PatchTag {
            tag_id: "tag-2".into(),
            patch: TagPatch::Label("api".into()),
        });
        let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));

        coordinator.submit(PersistenceCommand::PatchTag(
            "tag-2".into(),
            TagPatch::Label("api".into()),
        ));
        settle(&mut coordinator);

        let state = store.borrow();
        assert_eq!(state.state().tags[1].label, "backend");
        assert!(state.state().tag_save_error("tag-2").is_some());
    }

    #[test]
    fn create_multiple_patches_then_delete_drains_to_absent() {
        let (runtime, pool) = test_database();
        let task = test_task("task-1");
        let mut coordinator = test_coordinator(&runtime, &pool, test_store(vec![task.clone()]));

        coordinator.submit(PersistenceCommand::CreateTask(task.clone()));
        coordinator.submit(PersistenceCommand::PatchTask(
            task.id.clone(),
            TaskPatch::Title("Patched".to_string()),
        ));
        coordinator.submit(PersistenceCommand::PatchTask(
            task.id.clone(),
            TaskPatch::Detail("Details".to_string()),
        ));
        coordinator.submit(PersistenceCommand::DeleteTask(task));

        assert!(coordinator.drain(Duration::from_secs(2)));
        assert!(!coordinator.has_pending());
        let count: i64 = runtime
            .block_on(sqlx::query("SELECT COUNT(*) AS count FROM tasks").fetch_one(&pool))
            .expect("task count loads")
            .try_get("count")
            .expect("task count decodes");
        assert_eq!(count, 0);
    }

    #[test]
    fn latest_queued_same_field_patch_keeps_other_field_order() {
        let (runtime, pool) = test_database();
        let task = test_task("task-1");
        let mut coordinator = test_coordinator(&runtime, &pool, test_store(vec![task.clone()]));

        coordinator.submit(PersistenceCommand::CreateTask(task));
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".to_string(),
            TaskPatch::Title("First".to_string()),
        ));
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".to_string(),
            TaskPatch::Detail("Details".to_string()),
        ));
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".to_string(),
            TaskPatch::Title("Latest".to_string()),
        ));

        let queue = coordinator
            .queued
            .get(&CommandKey::Task)
            .expect("task queue exists");
        assert!(matches!(
            &queue[0],
            PersistenceCommand::PatchTask(_, TaskPatch::Detail(value)) if value == "Details"
        ));
        assert!(matches!(
            &queue[1],
            PersistenceCommand::PatchTask(_, TaskPatch::Title(value)) if value == "Latest"
        ));

        assert!(coordinator.drain(Duration::from_secs(2)));
        let row = runtime
            .block_on(
                sqlx::query("SELECT title, detail FROM tasks WHERE id = 'task-1'").fetch_one(&pool),
            )
            .expect("task reloads");
        assert_eq!(row.try_get::<String, _>("title").unwrap(), "Latest");
        assert_eq!(row.try_get::<String, _>("detail").unwrap(), "Details");
    }

    #[test]
    fn failed_active_patch_defers_completion_to_successful_queued_patch() {
        let (runtime, pool) = test_database();
        let task = test_task("task-1");
        persist_task(&runtime, &pool, task.clone());
        let store = test_store(vec![task]);
        store.borrow_mut().dispatch(AppEvent::SaveCompleted {
            target: SaveTarget::task("task-1".to_string(), crate::domain::TaskField::Detail),
            error: Some("old detail failure".to_string()),
        });
        let initial_version = store.borrow().state().version;
        let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));
        let key = CommandKey::Task;
        coordinator.active.insert(key.clone(), 7);
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".to_string(),
            TaskPatch::Title("Latest".to_string()),
        ));
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".to_string(),
            TaskPatch::Detail("Latest detail".to_string()),
        ));

        assert!(!coordinator.finish(Completion {
            key,
            sequence: 7,
            command: PersistenceCommand::PatchTask(
                "task-1".to_string(),
                TaskPatch::Title("Superseded".to_string()),
            ),
            error: Some("active title failure".to_string()),
            related_revisions: HashMap::new(),
        }));
        assert!(coordinator.drain(Duration::from_secs(2)));

        let state = store.borrow();
        assert!(state.state().save_errors.is_empty());
        assert_eq!(state.state().version, initial_version + 1);
    }

    #[test]
    fn custom_snooze_then_state_then_quick_keeps_last_custom_and_latest_workflow_order() {
        let (runtime, pool) = test_database();
        let task = test_task("task-1");
        persist_task(&runtime, &pool, task.clone());
        let mut coordinator = test_coordinator(&runtime, &pool, test_store(vec![task]));
        let key = CommandKey::Task;
        coordinator.active.insert(key.clone(), 41);
        let custom = time::macros::datetime!(2026-08-05 14:30);
        let quick = time::macros::datetime!(2026-08-06 8:00);

        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::Snooze {
                until: custom,
                remember_custom: Some(custom),
            },
        ));
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::State(crate::domain::TaskState::Done),
        ));
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::Snooze {
                until: quick,
                remember_custom: None,
            },
        ));

        let queue = coordinator.queued.get(&key).unwrap();
        assert!(matches!(
            queue[0],
            PersistenceCommand::PatchTask(
                _,
                TaskPatch::Snooze {
                    until,
                    remember_custom: Some(remembered)
                }
            ) if until == custom && remembered == custom
        ));
        assert!(matches!(
            queue[1],
            PersistenceCommand::PatchTask(
                _,
                TaskPatch::Snooze {
                    until,
                    remember_custom: None
                }
            ) if until == quick
        ));
        coordinator.finish(Completion {
            key,
            sequence: 41,
            command: PersistenceCommand::PatchTask(
                "task-1".into(),
                TaskPatch::Title("active".into()),
            ),
            error: None,
            related_revisions: HashMap::new(),
        });
        assert!(coordinator.drain(Duration::from_secs(2)));

        let row = runtime
            .block_on(
                sqlx::query("SELECT workflow_state, snoozed_until FROM tasks WHERE id = 'task-1'")
                    .fetch_one(&pool),
            )
            .unwrap();
        assert_eq!(
            row.try_get::<String, _>("workflow_state").unwrap(),
            "snoozed"
        );
        assert_eq!(
            row.try_get::<String, _>("snoozed_until").unwrap(),
            crate::snooze::format_datetime(quick)
        );
        let last: String = runtime
            .block_on(
                sqlx::query("SELECT value FROM settings WHERE key = 'last_custom_snooze'")
                    .fetch_one(&pool),
            )
            .unwrap()
            .try_get("value")
            .unwrap();
        assert_eq!(last, crate::snooze::format_datetime(custom));
    }

    #[test]
    fn custom_snooze_replaced_before_execution_preserves_remembered_value() {
        let (runtime, pool) = test_database();
        let task = test_task("task-1");
        persist_task(&runtime, &pool, task.clone());
        let mut coordinator = test_coordinator(&runtime, &pool, test_store(vec![task]));
        let key = CommandKey::Task;
        coordinator.active.insert(key.clone(), 42);
        let custom = time::macros::datetime!(2026-09-01 16:45);
        let quick = time::macros::datetime!(2026-09-02 8:00);
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::Snooze {
                until: custom,
                remember_custom: Some(custom),
            },
        ));
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::Snooze {
                until: quick,
                remember_custom: None,
            },
        ));

        let queue = coordinator.queued.get(&key).unwrap();
        assert_eq!(queue.len(), 1);
        assert!(matches!(
            queue[0],
            PersistenceCommand::PatchTask(
                _,
                TaskPatch::Snooze {
                    until,
                    remember_custom: Some(remembered)
                }
            ) if until == quick && remembered == custom
        ));

        coordinator.finish(Completion {
            key,
            sequence: 42,
            command: PersistenceCommand::PatchTask(
                "task-1".into(),
                TaskPatch::Detail("active".into()),
            ),
            error: None,
            related_revisions: HashMap::new(),
        });
        assert!(coordinator.drain(Duration::from_secs(2)));
        let task_row = runtime
            .block_on(
                sqlx::query("SELECT snoozed_until FROM tasks WHERE id = 'task-1'").fetch_one(&pool),
            )
            .unwrap();
        assert_eq!(
            task_row.try_get::<String, _>("snoozed_until").unwrap(),
            crate::snooze::format_datetime(quick)
        );
        let last: String = runtime
            .block_on(
                sqlx::query("SELECT value FROM settings WHERE key = 'last_custom_snooze'")
                    .fetch_one(&pool),
            )
            .unwrap()
            .try_get("value")
            .unwrap();
        assert_eq!(last, crate::snooze::format_datetime(custom));
    }

    #[test]
    fn other_task_custom_blocks_same_task_quick_from_reordering_global_last() {
        let (runtime, pool) = test_database();
        let task_a = test_task("task-a");
        let task_b = test_task("task-b");
        for task in [task_a.clone(), task_b.clone()] {
            persist_task(&runtime, &pool, task);
        }
        let mut coordinator = test_coordinator(&runtime, &pool, test_store(vec![task_a, task_b]));
        let key = CommandKey::Task;
        coordinator.active.insert(key.clone(), 46);
        let custom_a = time::macros::datetime!(2026-09-08 14:00);
        let custom_b = time::macros::datetime!(2026-09-09 17:45);
        let quick_a = time::macros::datetime!(2026-09-10 8:00);

        coordinator.submit(PersistenceCommand::PatchTask(
            "task-a".into(),
            TaskPatch::Snooze {
                until: custom_a,
                remember_custom: Some(custom_a),
            },
        ));
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-b".into(),
            TaskPatch::Snooze {
                until: custom_b,
                remember_custom: Some(custom_b),
            },
        ));
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-a".into(),
            TaskPatch::Snooze {
                until: quick_a,
                remember_custom: None,
            },
        ));

        let queue = coordinator.queued.get(&key).unwrap();
        assert_eq!(queue.len(), 3);
        assert!(matches!(
            queue[0],
            PersistenceCommand::PatchTask(
                ref id,
                TaskPatch::Snooze {
                    until,
                    remember_custom: Some(remembered)
                }
            ) if id == "task-a" && until == custom_a && remembered == custom_a
        ));
        assert!(matches!(
            queue[1],
            PersistenceCommand::PatchTask(
                ref id,
                TaskPatch::Snooze {
                    until,
                    remember_custom: Some(remembered)
                }
            ) if id == "task-b" && until == custom_b && remembered == custom_b
        ));
        assert!(matches!(
            queue[2],
            PersistenceCommand::PatchTask(
                ref id,
                TaskPatch::Snooze {
                    until,
                    remember_custom: None
                }
            ) if id == "task-a" && until == quick_a
        ));

        coordinator.finish(Completion {
            key,
            sequence: 46,
            command: PersistenceCommand::PatchTask(
                "active-task".into(),
                TaskPatch::Title("active".into()),
            ),
            error: None,
            related_revisions: HashMap::new(),
        });
        assert!(coordinator.drain(Duration::from_secs(2)));

        let task_a_until: String = runtime
            .block_on(
                sqlx::query("SELECT snoozed_until FROM tasks WHERE id = 'task-a'").fetch_one(&pool),
            )
            .unwrap()
            .try_get("snoozed_until")
            .unwrap();
        assert_eq!(task_a_until, crate::snooze::format_datetime(quick_a));
        let last: String = runtime
            .block_on(
                sqlx::query("SELECT value FROM settings WHERE key = 'last_custom_snooze'")
                    .fetch_one(&pool),
            )
            .unwrap()
            .try_get("value")
            .unwrap();
        assert_eq!(last, crate::snooze::format_datetime(custom_b));
    }

    #[test]
    fn custom_snooze_then_unsnooze_preserves_setting_and_latest_workflow_order() {
        let (runtime, pool) = test_database();
        let task = test_task("task-1");
        persist_task(&runtime, &pool, task.clone());
        let mut coordinator = test_coordinator(&runtime, &pool, test_store(vec![task]));
        let key = CommandKey::Task;
        coordinator.active.insert(key.clone(), 45);
        let custom = time::macros::datetime!(2026-09-02 16:45);

        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::Snooze {
                until: custom,
                remember_custom: Some(custom),
            },
        ));
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::Unsnooze,
        ));

        let queue = coordinator.queued.get(&key).unwrap();
        assert_eq!(queue.len(), 2);
        assert!(matches!(
            queue[0],
            PersistenceCommand::PatchTask(_, TaskPatch::Snooze { .. })
        ));
        assert!(matches!(
            queue[1],
            PersistenceCommand::PatchTask(_, TaskPatch::Unsnooze)
        ));
        coordinator.finish(Completion {
            key,
            sequence: 45,
            command: PersistenceCommand::PatchTask(
                "task-1".into(),
                TaskPatch::Detail("active".into()),
            ),
            error: None,
            related_revisions: HashMap::new(),
        });
        assert!(coordinator.drain(Duration::from_secs(2)));

        let row = runtime
            .block_on(
                sqlx::query("SELECT workflow_state, snoozed_until FROM tasks WHERE id = 'task-1'")
                    .fetch_one(&pool),
            )
            .unwrap();
        assert_eq!(row.try_get::<String, _>("workflow_state").unwrap(), "todo");
        assert_eq!(
            row.try_get::<Option<String>, _>("snoozed_until").unwrap(),
            None
        );
        let last: String = runtime
            .block_on(
                sqlx::query("SELECT value FROM settings WHERE key = 'last_custom_snooze'")
                    .fetch_one(&pool),
            )
            .unwrap()
            .try_get("value")
            .unwrap();
        assert_eq!(last, crate::snooze::format_datetime(custom));
    }

    #[test]
    fn failed_active_custom_snooze_is_not_suppressed_by_queued_state() {
        let (runtime, pool) = test_database();
        let task = test_task("task-1");
        persist_task(&runtime, &pool, task.clone());
        let store = test_store(vec![task]);
        let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));
        let key = CommandKey::Task;
        coordinator.active.insert(key.clone(), 43);
        let custom = time::macros::datetime!(2026-09-03 15:30);
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::State(crate::domain::TaskState::Done),
        ));

        assert!(coordinator.finish(Completion {
            key,
            sequence: 43,
            command: PersistenceCommand::PatchTask(
                "task-1".into(),
                TaskPatch::Snooze {
                    until: custom,
                    remember_custom: Some(custom),
                },
            ),
            error: Some("custom snooze failed".into()),
            related_revisions: HashMap::new(),
        }));
        assert!(coordinator.drain(Duration::from_secs(2)));

        assert!(
            store
                .borrow()
                .state()
                .save_errors
                .contains_key(&SaveTarget::task("task-1".into(), TaskField::Snooze))
        );
    }

    #[test]
    fn failed_active_custom_merges_into_queued_quick_compound_snooze() {
        let (runtime, pool) = test_database();
        let task = test_task("task-1");
        persist_task(&runtime, &pool, task.clone());
        let store = test_store(vec![task]);
        let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));
        let key = CommandKey::Task;
        coordinator.active.insert(key.clone(), 44);
        let custom = time::macros::datetime!(2026-09-04 16:15);
        let quick = time::macros::datetime!(2026-09-05 8:00);
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::Snooze {
                until: quick,
                remember_custom: None,
            },
        ));

        assert!(!coordinator.finish(Completion {
            key,
            sequence: 44,
            command: PersistenceCommand::PatchTask(
                "task-1".into(),
                TaskPatch::Snooze {
                    until: custom,
                    remember_custom: Some(custom),
                },
            ),
            error: Some("custom snooze failed".into()),
            related_revisions: HashMap::new(),
        }));
        assert!(coordinator.drain(Duration::from_secs(2)));

        let row = runtime
            .block_on(
                sqlx::query("SELECT snoozed_until FROM tasks WHERE id = 'task-1'").fetch_one(&pool),
            )
            .unwrap();
        assert_eq!(
            row.try_get::<String, _>("snoozed_until").unwrap(),
            crate::snooze::format_datetime(quick)
        );
        let last: String = runtime
            .block_on(
                sqlx::query("SELECT value FROM settings WHERE key = 'last_custom_snooze'")
                    .fetch_one(&pool),
            )
            .unwrap()
            .try_get("value")
            .unwrap();
        assert_eq!(last, crate::snooze::format_datetime(custom));
        assert!(store.borrow().state().save_errors.is_empty());
    }

    #[test]
    fn latest_custom_snooze_wins_across_tasks_in_submission_order() {
        let (runtime, pool) = test_database();
        let first = test_task("task-1");
        let second = test_task("task-2");
        for task in [first.clone(), second.clone()] {
            persist_task(&runtime, &pool, task);
        }
        let mut coordinator = test_coordinator(&runtime, &pool, test_store(vec![first, second]));
        let first_custom = time::macros::datetime!(2026-09-06 14:00);
        let latest_custom = time::macros::datetime!(2026-09-07 17:45);

        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::Snooze {
                until: first_custom,
                remember_custom: Some(first_custom),
            },
        ));
        coordinator.submit(PersistenceCommand::PatchTask(
            "task-2".into(),
            TaskPatch::Snooze {
                until: latest_custom,
                remember_custom: Some(latest_custom),
            },
        ));
        assert!(coordinator.drain(Duration::from_secs(2)));

        let last: String = runtime
            .block_on(
                sqlx::query("SELECT value FROM settings WHERE key = 'last_custom_snooze'")
                    .fetch_one(&pool),
            )
            .unwrap()
            .try_get("value")
            .unwrap();
        assert_eq!(last, crate::snooze::format_datetime(latest_custom));
    }

    #[test]
    fn successful_active_patch_defers_completion_to_failed_queued_patch() {
        let (runtime, pool) = test_database();
        let store = test_store(vec![test_task("missing-task")]);
        let target = SaveTarget::task("missing-task".to_string(), crate::domain::TaskField::Title);
        store.borrow_mut().dispatch(AppEvent::SaveCompleted {
            target: target.clone(),
            error: Some("old title failure".to_string()),
        });
        let initial_version = store.borrow().state().version;
        let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));
        let key = CommandKey::Task;
        coordinator.active.insert(key.clone(), 11);
        coordinator.submit(PersistenceCommand::PatchTask(
            "missing-task".to_string(),
            TaskPatch::Title("Latest".to_string()),
        ));

        assert!(!coordinator.finish(Completion {
            key,
            sequence: 11,
            command: PersistenceCommand::PatchTask(
                "missing-task".to_string(),
                TaskPatch::Title("Superseded".to_string()),
            ),
            error: None,
            related_revisions: HashMap::new(),
        }));
        assert!(store.borrow().state().save_errors.contains_key(&target));
        assert_eq!(store.borrow().state().version, initial_version);
        assert!(coordinator.drain(Duration::from_secs(2)));

        let state = store.borrow();
        assert!(state.state().save_errors.contains_key(&target));
        assert_eq!(state.state().version, initial_version + 1);
    }

    #[test]
    fn failed_create_discards_queued_delete_and_removes_optimistic_task() {
        sqlx::any::install_default_drivers();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime builds");
        let pool = runtime
            .block_on(
                AnyPoolOptions::new()
                    .max_connections(1)
                    .connect("sqlite::memory:"),
            )
            .expect("database connects");
        let task = test_task("phantom");
        let store = test_store(vec![task.clone()]);
        runtime.block_on(pool.close());
        let mut coordinator = PersistenceCoordinator::new(
            Rc::clone(&store),
            pool,
            SqlDialect::Sqlite,
            runtime.handle().clone(),
            None,
        );

        coordinator.submit(PersistenceCommand::CreateTask(task.clone()));
        coordinator.submit(PersistenceCommand::DeleteTask(task));

        assert!(coordinator.drain(Duration::from_secs(2)));
        assert!(!coordinator.has_pending());
        assert!(store.borrow().state().tasks.is_empty());
    }

    #[test]
    fn failed_delete_restores_task_once() {
        let (runtime, pool) = test_database();
        let task = test_task("task-1");
        let store = test_store(Vec::new());
        let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));

        coordinator.submit(PersistenceCommand::DeleteTask(task));

        assert!(coordinator.drain(Duration::from_secs(2)));
        assert_eq!(store.borrow().state().tasks.len(), 1);
        assert_eq!(store.borrow().state().tasks[0].id, "task-1");
    }

    #[test]
    fn management_entity_creates_and_queued_deletes_drain_to_absent() {
        let (runtime, pool) = test_database();
        let person = Person::new("person-1".into(), "Ada".into(), String::new());
        let project = Project::new(
            "project-1".into(),
            "CORE".into(),
            "Core".into(),
            String::new(),
        );
        let tag = Tag::new("tag-1".into(), "api".into());
        let mut coordinator = test_coordinator(&runtime, &pool, test_store(Vec::new()));

        coordinator.submit(PersistenceCommand::CreatePerson(person.clone()));
        coordinator.submit(PersistenceCommand::DeletePerson(PersonDeletion {
            person,
            task_ids: Vec::new(),
            lead_project_ids: Vec::new(),
        }));
        coordinator.submit(PersistenceCommand::CreateProject(project.clone()));
        coordinator.submit(PersistenceCommand::DeleteProject(ProjectDeletion {
            project,
            task_ids: Vec::new(),
        }));
        coordinator.submit(PersistenceCommand::CreateTag(tag.clone()));
        coordinator.submit(PersistenceCommand::DeleteTag(TagDeletion {
            tag,
            task_ids: Vec::new(),
        }));

        assert!(coordinator.drain(Duration::from_secs(2)));
        let people: i64 = runtime
            .block_on(sqlx::query("SELECT COUNT(*) AS count FROM people").fetch_one(&pool))
            .unwrap()
            .try_get("count")
            .unwrap();
        let projects: i64 = runtime
            .block_on(sqlx::query("SELECT COUNT(*) AS count FROM projects").fetch_one(&pool))
            .unwrap()
            .try_get("count")
            .unwrap();
        let tags: i64 = runtime
            .block_on(sqlx::query("SELECT COUNT(*) AS count FROM tags").fetch_one(&pool))
            .unwrap()
            .try_get("count")
            .unwrap();
        assert_eq!((people, projects, tags), (0, 0, 0));
    }

    #[test]
    fn failed_management_entity_deletes_restore_optimistic_state() {
        let (runtime, pool) = test_database();
        let store = test_store(Vec::new());
        let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));

        coordinator.submit(PersistenceCommand::DeletePerson(PersonDeletion {
            person: Person::new("person-1".into(), "Ada".into(), String::new()),
            task_ids: Vec::new(),
            lead_project_ids: Vec::new(),
        }));
        coordinator.submit(PersistenceCommand::DeleteProject(ProjectDeletion {
            project: Project::new(
                "project-1".into(),
                "CORE".into(),
                "Core".into(),
                String::new(),
            ),
            task_ids: Vec::new(),
        }));
        coordinator.submit(PersistenceCommand::DeleteTag(TagDeletion {
            tag: Tag::new("tag-1".into(), "api".into()),
            task_ids: Vec::new(),
        }));

        assert!(coordinator.drain(Duration::from_secs(2)));
        let state = store.borrow();
        assert_eq!(state.state().people[0].id, "person-1");
        assert_eq!(state.state().projects[0].id, "project-1");
        assert_eq!(state.state().tags[0].id, "tag-1");
    }

    #[test]
    fn inline_task_tag_revision_supports_followup_edit_and_delete() {
        let (runtime, pool) = test_database();
        let task = test_task("task-1");
        persist_task(&runtime, &pool, task.clone());
        let tag = Tag::new("tag-inline".into(), "api".into());
        let store = test_store(vec![task]);
        store
            .borrow_mut()
            .dispatch(AppEvent::TagCreated(tag.clone()));
        let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));

        coordinator.submit(PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::Tags(vec![tag.clone()]),
        ));
        assert!(coordinator.drain(Duration::from_secs(2)));
        assert_eq!(
            store
                .borrow()
                .state()
                .entity_revisions
                .get("tag:tag-inline"),
            Some(&1)
        );

        coordinator.submit(PersistenceCommand::PatchTag(
            tag.id.clone(),
            TagPatch::Label("backend".into()),
        ));
        assert!(coordinator.drain(Duration::from_secs(2)));
        assert_eq!(
            store
                .borrow()
                .state()
                .entity_revisions
                .get("tag:tag-inline"),
            Some(&2)
        );

        coordinator.submit(PersistenceCommand::DeleteTag(TagDeletion {
            tag,
            task_ids: vec!["task-1".into()],
        }));
        assert!(coordinator.drain(Duration::from_secs(2)));
        let count: i64 = runtime
            .block_on(
                sqlx::query("SELECT COUNT(*) AS count FROM tags WHERE id = 'tag-inline'")
                    .fetch_one(&pool),
            )
            .unwrap()
            .try_get("count")
            .unwrap();
        assert_eq!(count, 0);
    }
}
