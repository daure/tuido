use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

use sqlx::AnyPool;
use tokio::runtime::Handle;
use tuicore::Store;

use crate::{
    domain::{
        AppEvent, AppState, Person, PersonDeletion, PersonPatch, Project, ProjectDeletion,
        ProjectPatch, SaveTarget, Tag, TagDeletion, TagPatch, Task, TaskField, TaskPatch,
    },
    storage::{self, SqlDialect},
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
}

pub(crate) struct PersistenceCoordinator {
    store: AppStore,
    pool: AnyPool,
    dialect: SqlDialect,
    runtime: Handle,
    completion_tx: mpsc::Sender<Completion>,
    completion_rx: mpsc::Receiver<Completion>,
    active: HashMap<CommandKey, u64>,
    queued: HashMap<CommandKey, VecDeque<PersistenceCommand>>,
    next_sequence: u64,
}

impl PersistenceCoordinator {
    pub(crate) fn new(
        store: AppStore,
        pool: AnyPool,
        dialect: SqlDialect,
        runtime: Handle,
    ) -> Self {
        let (completion_tx, completion_rx) = mpsc::channel();
        Self {
            store,
            pool,
            dialect,
            runtime,
            completion_tx,
            completion_rx,
            active: HashMap::new(),
            queued: HashMap::new(),
            next_sequence: 0,
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
        while let Ok(completion) = self.completion_rx.try_recv() {
            changed |= self.finish(completion);
        }
        changed
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
        let pool = self.pool.clone();
        let dialect = self.dialect;
        let tx = self.completion_tx.clone();
        self.runtime.spawn(async move {
            let result = execute(pool, dialect, command.clone()).await;
            let _ = tx.send(Completion {
                key,
                sequence,
                command,
                error: result.err().map(|error| error.to_string()),
            });
        });
    }

    fn finish(&mut self, completion: Completion) -> bool {
        if self.active.get(&completion.key).copied() != Some(completion.sequence) {
            return false;
        }
        self.active.remove(&completion.key);
        if completion.error.is_some() {
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

async fn execute(
    pool: AnyPool,
    dialect: SqlDialect,
    command: PersistenceCommand,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match command {
        PersistenceCommand::CreateTask(task) => storage::create_task(pool, dialect, task).await,
        PersistenceCommand::DeleteTask(task) => storage::delete_task(pool, dialect, task.id).await,
        PersistenceCommand::CreatePerson(person) => {
            storage::create_person(pool, dialect, person).await
        }
        PersistenceCommand::DeletePerson(deletion) => {
            storage::delete_person(pool, dialect, deletion.person.id).await
        }
        PersistenceCommand::CreateProject(project) => {
            storage::create_project(pool, dialect, project).await
        }
        PersistenceCommand::DeleteProject(deletion) => {
            storage::delete_project(pool, dialect, deletion.project.id).await
        }
        PersistenceCommand::CreateTag(tag) => storage::create_tag(pool, dialect, tag).await,
        PersistenceCommand::DeleteTag(deletion) => {
            storage::delete_tag(pool, dialect, deletion.tag.id).await
        }
        PersistenceCommand::PatchTask(id, patch) => {
            storage::save_patch(pool, dialect, id, patch).await
        }
        PersistenceCommand::PatchPerson(id, patch) => {
            storage::save_person_patch(pool, dialect, id, patch).await
        }
        PersistenceCommand::PatchProject(id, patch) => {
            storage::save_project_patch(pool, dialect, id, patch).await
        }
        PersistenceCommand::PatchTag(id, patch) => {
            storage::save_tag_patch(pool, dialect, id, patch).await
        }
    }
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
        Rc::new(RefCell::new(Store::new(
            AppState::from_snapshot(WorkspaceSnapshot {
                tasks,
                people: Vec::new(),
                projects: Vec::new(),
                tags: Vec::new(),
            }),
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
        )
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
        runtime
            .block_on(storage::create_task(
                pool.clone(),
                SqlDialect::Sqlite,
                task.clone(),
            ))
            .expect("task creates");
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
        runtime
            .block_on(storage::create_task(
                pool.clone(),
                SqlDialect::Sqlite,
                task.clone(),
            ))
            .unwrap();
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
        runtime
            .block_on(storage::create_task(
                pool.clone(),
                SqlDialect::Sqlite,
                task.clone(),
            ))
            .unwrap();
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
            runtime
                .block_on(storage::create_task(pool.clone(), SqlDialect::Sqlite, task))
                .unwrap();
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
        runtime
            .block_on(storage::create_task(
                pool.clone(),
                SqlDialect::Sqlite,
                task.clone(),
            ))
            .unwrap();
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
        runtime
            .block_on(storage::create_task(
                pool.clone(),
                SqlDialect::Sqlite,
                task.clone(),
            ))
            .unwrap();
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
        runtime
            .block_on(storage::create_task(
                pool.clone(),
                SqlDialect::Sqlite,
                task.clone(),
            ))
            .unwrap();
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
            runtime
                .block_on(storage::create_task(pool.clone(), SqlDialect::Sqlite, task))
                .unwrap();
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
}
