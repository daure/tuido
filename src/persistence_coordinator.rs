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
        ProjectPatch, SaveTarget, Tag, TagDeletion, TagPatch, Task, TaskField, TaskPatch, TaskRank,
    },
    service::{TaskRankUpdate, TuidoService},
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
    AppSetting(String),
}

#[derive(Debug, Clone)]
pub(crate) enum PersistenceCommand {
    CreateTask(Task),
    DeleteTask(Task),
    PatchTask(String, TaskPatch),
    ReorderTasks {
        before: Vec<TaskRank>,
        after: Vec<TaskRank>,
        expected_revisions: HashMap<String, u64>,
    },
    CreatePerson(Person),
    DeletePerson(PersonDeletion),
    PatchPerson(String, PersonPatch),
    CreateProject(Project),
    DeleteProject(ProjectDeletion),
    PatchProject(String, ProjectPatch),
    CreateTag(Tag),
    DeleteTag(TagDeletion),
    PatchTag(String, TagPatch),
    SetAppSetting {
        key: String,
        value: String,
        generation: u64,
    },
}

impl PersistenceCommand {
    fn key(&self) -> CommandKey {
        match self {
            Self::CreateTask(_)
            | Self::DeleteTask(_)
            | Self::PatchTask(_, _)
            | Self::ReorderTasks { .. } => CommandKey::Task,
            Self::CreatePerson(person) => CommandKey::Person(person.id.clone()),
            Self::DeletePerson(deletion) => CommandKey::Person(deletion.person.id.clone()),
            Self::PatchPerson(id, _) => CommandKey::Person(id.clone()),
            Self::CreateProject(project) => CommandKey::Project(project.id.clone()),
            Self::DeleteProject(deletion) => CommandKey::Project(deletion.project.id.clone()),
            Self::PatchProject(id, _) => CommandKey::Project(id.clone()),
            Self::CreateTag(tag) => CommandKey::Tag(tag.id.clone()),
            Self::DeleteTag(deletion) => CommandKey::Tag(deletion.tag.id.clone()),
            Self::PatchTag(id, _) => CommandKey::Tag(id.clone()),
            Self::SetAppSetting { key, .. } => CommandKey::AppSetting(key.clone()),
        }
    }
}

struct Completion {
    key: CommandKey,
    sequence: u64,
    command: PersistenceCommand,
    error: Option<String>,
    related_revisions: HashMap<String, u64>,
    created_task: Option<Task>,
}

struct ExecutionResult {
    related_revisions: HashMap<String, u64>,
    created_task: Option<Task>,
}

struct RefreshCompletion {
    revision: u64,
    snapshot: Option<crate::domain::WorkspaceSnapshot>,
    revisions: HashMap<String, u64>,
    expiry_error: Option<String>,
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
    next_setting_generation: u64,
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
            next_setting_generation: 1,
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
        if let PersistenceCommand::SetAppSetting {
            key,
            value,
            generation,
        } = &mut command
        {
            *generation = self.next_setting_generation;
            self.next_setting_generation += 1;
            self.store
                .borrow_mut()
                .dispatch(AppEvent::AppSettingChangeRequested {
                    key: key.clone(),
                    value: value.clone(),
                    generation: *generation,
                });
        }
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
                            if let Some(error) = refresh.expiry_error {
                                changed |= self
                                    .store
                                    .borrow_mut()
                                    .dispatch(AppEvent::RefreshFailed(error))
                                    .changed;
                            }
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
        let current = {
            let store = self.store.borrow();
            let state = store.state();
            state.workspace_revision
        };
        let force_snapshot = self.reconcile_required;
        let tx = self.refresh_tx.clone();
        self.runtime.spawn(async move {
            let result = async {
                let expiry_error = service
                    .process_snooze_expirations()
                    .await
                    .err()
                    .map(|error| format!("Snooze expiry processing failed: {error}"));
                let observed = service.workspace_revision().await?;
                if expiry_error.is_none() && !force_snapshot && observed <= current {
                    return Ok(RefreshCompletion {
                        revision: observed,
                        snapshot: None,
                        revisions: HashMap::new(),
                        expiry_error: None,
                    });
                }
                let workspace = service.consistent_workspace().await?;
                Ok(RefreshCompletion {
                    revision: workspace.revision,
                    snapshot: Some(workspace.snapshot),
                    revisions: workspace.entity_revisions,
                    expiry_error,
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

    fn start(&mut self, mut command: PersistenceCommand) {
        if let PersistenceCommand::ReorderTasks {
            after,
            expected_revisions,
            ..
        } = &mut command
        {
            let state = self.store.borrow();
            expected_revisions.clear();
            expected_revisions.extend(after.iter().filter_map(|rank| {
                state
                    .state()
                    .entity_revisions
                    .get(&format!("task:{}", rank.id))
                    .copied()
                    .map(|revision| (rank.id.clone(), revision))
            }));
        }
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
            let (error, related_revisions, created_task) = match result {
                Ok(result) => (None, result.related_revisions, result.created_task),
                Err(error) => (Some(error.to_string()), HashMap::new(), None),
            };
            let _ = tx.send(Completion {
                key,
                sequence,
                command,
                error,
                related_revisions,
                created_task,
            });
        });
    }

    fn finish(&mut self, completion: Completion) -> bool {
        if self.active.get(&completion.key).copied() != Some(completion.sequence) {
            return false;
        }
        self.active.remove(&completion.key);
        let committed_expected = self.active_expected.remove(&completion.sequence).flatten();
        if completion.error.is_none() {
            let revision = completion
                .created_task
                .as_ref()
                .map(|task| (format!("task:{}", task.id), Some(1)))
                .or_else(|| revision_update(&completion.command, committed_expected));
            if let Some((key, revision)) = revision {
                self.store
                    .borrow_mut()
                    .dispatch(AppEvent::EntityRevisionCommitted { key, revision });
            }
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
                } else if let Some(created_task) = completion.created_task {
                    remap_queued_task_id(
                        self.queued.get_mut(&completion.key),
                        &task.id,
                        &created_task.id,
                    );
                    changed |= self
                        .store
                        .borrow_mut()
                        .dispatch(AppEvent::TaskDeleted(task.id))
                        .changed;
                    changed |= self
                        .store
                        .borrow_mut()
                        .dispatch(AppEvent::TaskCreated(created_task))
                        .changed;
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
            PersistenceCommand::ReorderTasks { before, .. } => {
                if let Some(error) = completion.error {
                    changed |= self
                        .store
                        .borrow_mut()
                        .dispatch(AppEvent::TaskRanksChanged(before))
                        .changed;
                    changed |= self
                        .store
                        .borrow_mut()
                        .dispatch(AppEvent::RefreshFailed(format!(
                            "Task reorder failed: {error}"
                        )))
                        .changed;
                } else {
                    self.store
                        .borrow_mut()
                        .dispatch(AppEvent::WorkspaceRevisionCommitted);
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
            PersistenceCommand::SetAppSetting {
                key,
                value,
                generation,
            } => {
                changed |= self
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::AppSettingSaveCompleted {
                        key,
                        value,
                        generation,
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
) -> Result<ExecutionResult, Box<dyn std::error::Error + Send + Sync>> {
    let expected = || -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
        expected_revision.ok_or_else(|| "missing entity revision; refresh required".into())
    };
    let mut created_task = None;
    let related_revisions = match command {
        PersistenceCommand::CreateTask(task) => {
            let created = service
                .create_task_entity(task)
                .await
                .map_err(boxed_service_error)?;
            created_task = service
                .domain_snapshot()
                .await
                .map_err(boxed_service_error)?
                .tasks
                .into_iter()
                .find(|task| task.id == created.value.id);
            Ok(HashMap::new())
        }
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
        PersistenceCommand::ReorderTasks {
            after,
            expected_revisions,
            ..
        } => {
            let updates = after
                .into_iter()
                .map(|rank| {
                    let expected_revision =
                        expected_revisions.get(&rank.id).copied().ok_or_else(|| {
                            format!("missing task revision for {}; refresh required", rank.id)
                        })?;
                    Ok(TaskRankUpdate {
                        rank,
                        expected_revision,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            service
                .reorder_tasks(updates)
                .await
                .map_err(boxed_service_error)
        }
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
        PersistenceCommand::SetAppSetting { key, value, .. } => service
            .set_app_setting(&key, &value)
            .await
            .map(|_| HashMap::new())
            .map_err(boxed_service_error),
    }?;
    Ok(ExecutionResult {
        related_revisions,
        created_task,
    })
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
        PersistenceCommand::ReorderTasks { .. } => None,
        PersistenceCommand::DeletePerson(v) => Some(("person", &v.person.id)),
        PersistenceCommand::PatchPerson(id, _) => Some(("person", id)),
        PersistenceCommand::DeleteProject(v) => Some(("project", &v.project.id)),
        PersistenceCommand::PatchProject(id, _) => Some(("project", id)),
        PersistenceCommand::DeleteTag(v) => Some(("tag", &v.tag.id)),
        PersistenceCommand::PatchTag(id, _) => Some(("tag", id)),
        PersistenceCommand::SetAppSetting { .. } => None,
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

fn remap_queued_task_id(
    queue: Option<&mut VecDeque<PersistenceCommand>>,
    old_id: &str,
    new_id: &str,
) {
    let Some(queue) = queue else { return };
    for command in queue {
        match command {
            PersistenceCommand::DeleteTask(task) if task.id == old_id => {
                task.id = new_id.to_string();
            }
            PersistenceCommand::PatchTask(id, _) if id == old_id => {
                *id = new_id.to_string();
            }
            PersistenceCommand::ReorderTasks {
                before,
                after,
                expected_revisions,
            } => {
                for rank in before.iter_mut().chain(after.iter_mut()) {
                    if rank.id == old_id {
                        rank.id = new_id.to_string();
                    }
                }
                if let Some(revision) = expected_revisions.remove(old_id) {
                    expected_revisions.insert(new_id.to_string(), revision);
                }
            }
            _ => {}
        }
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
#[path = "persistence_coordinator/tests.rs"]
mod tests;
