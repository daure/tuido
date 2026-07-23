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
        AppEvent, AppState, PersonField, PersonPatch, ProjectField, ProjectPatch, SaveTarget,
        TagField, TagPatch, Task, TaskPatch,
    },
    storage::{self, SqlDialect},
};

pub(crate) type AppStore =
    Rc<RefCell<Store<AppState, AppEvent, fn(&mut AppState, AppEvent) -> tuicore::DispatchOutcome>>>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CommandKey {
    Task(String),
    Person(String, PersonField),
    Project(String, ProjectField),
    Tag(String, TagField),
}

#[derive(Debug, Clone)]
pub(crate) enum PersistenceCommand {
    CreateTask(Task),
    DeleteTask(Task),
    PatchTask(String, TaskPatch),
    PatchPerson(String, PersonPatch),
    PatchProject(String, ProjectPatch),
    PatchTag(String, TagPatch),
}

impl PersistenceCommand {
    fn key(&self) -> CommandKey {
        match self {
            Self::CreateTask(task) | Self::DeleteTask(task) => CommandKey::Task(task.id.clone()),
            Self::PatchTask(id, _) => CommandKey::Task(id.clone()),
            Self::PatchPerson(id, patch) => CommandKey::Person(id.clone(), patch.field()),
            Self::PatchProject(id, patch) => CommandKey::Project(id.clone(), patch.field()),
            Self::PatchTag(id, patch) => CommandKey::Tag(id.clone(), patch.field()),
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

    pub(crate) fn submit(&mut self, command: PersistenceCommand) {
        let key = command.key();
        if self.active.contains_key(&key) {
            let queue = self.queued.entry(key).or_default();
            let patch_field = match &command {
                PersistenceCommand::PatchTask(_, patch) => Some(patch.field()),
                _ => None,
            };
            let replace_index = patch_field.and_then(|field| {
                queue
                    .iter()
                    .enumerate()
                    .rev()
                    .take_while(|(_, queued)| matches!(queued, PersistenceCommand::PatchTask(_, _)))
                    .find_map(|(index, queued)| match queued {
                        PersistenceCommand::PatchTask(_, patch) if patch.field() == field => {
                            Some(index)
                        }
                        _ => None,
                    })
            });
            if let Some(index) = replace_index {
                queue[index] = command;
            } else {
                queue.push_back(command);
            }
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
        let task_patch_is_superseded = match &completion.command {
            PersistenceCommand::PatchTask(_, patch) => {
                self.queued.get(&completion.key).is_some_and(|queue| {
                    queue
                        .iter()
                        .take_while(|command| {
                            matches!(command, PersistenceCommand::PatchTask(_, _))
                        })
                        .any(|command| {
                            matches!(
                                command,
                                PersistenceCommand::PatchTask(_, queued_patch)
                                    if queued_patch.field() == patch.field()
                            )
                        })
                })
            }
            _ => false,
        };
        let mut changed = false;
        match completion.command {
            PersistenceCommand::CreateTask(task) => {
                if completion.error.is_some() {
                    changed |= self
                        .store
                        .borrow_mut()
                        .dispatch(AppEvent::TaskDeleted(task.id))
                        .changed;
                    self.queued.remove(&completion.key);
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
                            !matches!(command, PersistenceCommand::PatchTask(_, _))
                        });
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{TaskSize, WorkspaceSnapshot, reduce_app_state};
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
            .get(&CommandKey::Task("task-1".to_string()))
            .expect("task queue exists");
        assert!(matches!(
            &queue[0],
            PersistenceCommand::PatchTask(_, TaskPatch::Title(value)) if value == "Latest"
        ));
        assert!(matches!(
            &queue[1],
            PersistenceCommand::PatchTask(_, TaskPatch::Detail(value)) if value == "Details"
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
        let key = CommandKey::Task("task-1".to_string());
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
        let key = CommandKey::Task("missing-task".to_string());
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
}
