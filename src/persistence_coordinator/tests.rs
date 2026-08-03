use super::*;
use crate::domain::{
    Person, Tag, TaskSize, TaskState, Workspace, WorkspaceSnapshot, reduce_app_state,
};
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
        workspaces: Vec::new(),
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
    let task = test_task("pending-1");
    let mut coordinator = test_coordinator(&runtime, &pool, test_store(vec![task.clone()]));

    coordinator.submit(PersistenceCommand::CreateTask(task));
    coordinator.submit(PersistenceCommand::PatchTask(
        "pending-1".to_string(),
        TaskPatch::Title("Patched".to_string()),
    ));

    assert!(coordinator.drain(Duration::from_secs(2)));
    assert_eq!(coordinator.store.borrow().state().tasks[0].id, "1");
    let title: String = runtime
        .block_on(sqlx::query("SELECT title FROM tasks WHERE id = 1").fetch_one(&pool))
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
                workspaces: Vec::new(),
                tags: Vec::new(),
            }),
            revision: 1,
            revisions: HashMap::new(),
            expiry_error: None,
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
        .block_on(sqlx::query("DROP TABLE people").execute(&pool))
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
fn unchanged_revision_poll_reloads_when_a_local_snooze_expires() {
    let (runtime, pool) = test_database();
    let mut task = test_task("task-1");
    task.state = TaskState::Snoozed;
    task.snoozed_until = Some(time::macros::datetime!(2000-01-01 0:00));
    persist_task(&runtime, &pool, task.clone());
    let store = test_store(vec![task.clone()]);
    store.borrow_mut().dispatch(AppEvent::WorkspaceRefreshed {
        snapshot: WorkspaceSnapshot {
            tasks: vec![task],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        },
        revision: 2,
        entity_revisions: HashMap::from([("task:task-1".into(), 1)]),
    });
    let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));

    coordinator.start_refresh();
    settle(&mut coordinator);

    let store = store.borrow();
    let state = store.state();
    assert_eq!(state.workspace_revision, 3);
    assert_eq!(state.tasks[0].state, TaskState::Todo);
    assert_eq!(state.tasks[0].snoozed_until, None);
    assert_eq!(state.entity_revisions["task:1"], 2);
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
    assert_eq!(state.state().entity_revisions["task:1"], 2);
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
    assert_eq!(
        state.state().tasks[0].state,
        crate::domain::TaskState::Backlog
    );
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
                workspaces: Vec::new(),
                tags: Vec::new(),
            }),
            revision: 1,
            revisions: HashMap::new(),
            expiry_error: None,
        }))
        .unwrap();
    assert!(coordinator.poll());
    assert_eq!(store.borrow().state().refresh_error, None);
}

#[test]
fn failed_app_setting_save_restores_previous_value_and_exposes_error() {
    let (runtime, pool) = test_database();
    let store = test_store(Vec::new());
    store
        .borrow_mut()
        .dispatch(AppEvent::AppSettingChangeRequested {
            key: "calendar.show_weekends".into(),
            value: "false".into(),
            generation: 1,
        });
    store
        .borrow_mut()
        .dispatch(AppEvent::AppSettingSaveCompleted {
            key: "calendar.show_weekends".into(),
            value: "false".into(),
            generation: 1,
            error: None,
        });
    runtime
        .block_on(sqlx::query("DROP TABLE app_settings").execute(&pool))
        .unwrap();
    let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));

    coordinator.submit(PersistenceCommand::SetAppSetting {
        key: "calendar.show_weekends".into(),
        value: "true".into(),
        generation: 0,
    });
    assert!(coordinator.drain(Duration::from_secs(2)));

    let state = store.borrow();
    assert_eq!(
        state
            .state()
            .app_setting_values
            .get("calendar.show_weekends")
            .map(String::as_str),
        Some("false")
    );
    assert!(
        state
            .state()
            .app_setting_errors
            .contains_key("calendar.show_weekends")
    );
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
        workspaces: Vec::new(),
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
        TaskPatch::Description("Details".to_string()),
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
        TaskPatch::Description("Details".to_string()),
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
        PersistenceCommand::PatchTask(_, TaskPatch::Description(value)) if value == "Details"
    ));
    assert!(matches!(
        &queue[1],
        PersistenceCommand::PatchTask(_, TaskPatch::Title(value)) if value == "Latest"
    ));

    assert!(coordinator.drain(Duration::from_secs(2)));
    let row = runtime
        .block_on(sqlx::query("SELECT title, description FROM tasks WHERE id = 1").fetch_one(&pool))
        .expect("task reloads");
    assert_eq!(row.try_get::<String, _>("title").unwrap(), "Latest");
    assert_eq!(row.try_get::<String, _>("description").unwrap(), "Details");
}

#[test]
fn failed_active_patch_defers_completion_to_successful_queued_patch() {
    let (runtime, pool) = test_database();
    let task = test_task("task-1");
    persist_task(&runtime, &pool, task.clone());
    let store = test_store(vec![task]);
    store.borrow_mut().dispatch(AppEvent::SaveCompleted {
        target: SaveTarget::task("task-1".to_string(), crate::domain::TaskField::Description),
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
        TaskPatch::Description("Latest detail".to_string()),
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
        created_task: None,
    }));
    assert!(coordinator.drain(Duration::from_secs(2)));

    let state = store.borrow();
    assert!(state.state().save_errors.is_empty());
    assert_eq!(state.state().version, initial_version + 1);
}

#[test]
fn custom_snooze_then_state_then_quick_keeps_latest_workflow() {
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

    coordinator.finish(Completion {
        key,
        sequence: 41,
        command: PersistenceCommand::PatchTask("task-1".into(), TaskPatch::Title("active".into())),
        error: None,
        related_revisions: HashMap::new(),
        created_task: None,
    });
    assert!(coordinator.drain(Duration::from_secs(2)));

    let row = runtime
        .block_on(
            sqlx::query("SELECT workflow_state, snoozed_until FROM tasks WHERE id = 1")
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

    assert!(matches!(
        coordinator.queued.get(&key),
        Some(queue) if queue.len() == 1 && matches!(
            queue.front(),
            Some(PersistenceCommand::PatchTask(
                _,
                TaskPatch::Snooze {
                    until,
                    remember_custom: Some(remembered),
                },
            )) if *until == quick && *remembered == custom
        )
    ));

    coordinator.finish(Completion {
        key,
        sequence: 42,
        command: PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::Description("active".into()),
        ),
        error: None,
        related_revisions: HashMap::new(),
        created_task: None,
    });
    assert!(coordinator.drain(Duration::from_secs(2)));
    let task_row = runtime
        .block_on(sqlx::query("SELECT snoozed_until FROM tasks WHERE id = 1").fetch_one(&pool))
        .unwrap();
    assert_eq!(
        task_row.try_get::<String, _>("snoozed_until").unwrap(),
        crate::snooze::format_datetime(quick)
    );
    let last = runtime
        .block_on(
            sqlx::query("SELECT value FROM settings WHERE key = 'last_custom_snooze'")
                .fetch_optional(&pool),
        )
        .unwrap();
    assert!(last.is_none());
}

#[test]
fn other_task_custom_blocks_same_task_quick_from_reordering_global_last() {
    let (runtime, pool) = test_database();
    let task_a = test_task("1");
    let task_b = test_task("2");
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
        "1".into(),
        TaskPatch::Snooze {
            until: custom_a,
            remember_custom: Some(custom_a),
        },
    ));
    coordinator.submit(PersistenceCommand::PatchTask(
        "2".into(),
        TaskPatch::Snooze {
            until: custom_b,
            remember_custom: Some(custom_b),
        },
    ));
    coordinator.submit(PersistenceCommand::PatchTask(
        "1".into(),
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
        ) if id == "1" && until == custom_a && remembered == custom_a
    ));
    assert!(matches!(
        queue[1],
        PersistenceCommand::PatchTask(
            ref id,
            TaskPatch::Snooze {
                until,
                remember_custom: Some(remembered)
            }
        ) if id == "2" && until == custom_b && remembered == custom_b
    ));
    assert!(matches!(
        queue[2],
        PersistenceCommand::PatchTask(
            ref id,
            TaskPatch::Snooze {
                until,
                remember_custom: None
            }
        ) if id == "1" && until == quick_a
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
        created_task: None,
    });
    assert!(coordinator.drain(Duration::from_secs(2)));

    let task_a_until: String = runtime
        .block_on(sqlx::query("SELECT snoozed_until FROM tasks WHERE id = 1").fetch_one(&pool))
        .unwrap()
        .try_get("snoozed_until")
        .unwrap();
    assert_eq!(task_a_until, crate::snooze::format_datetime(quick_a));
    let last = runtime
        .block_on(
            sqlx::query("SELECT value FROM settings WHERE key = 'last_custom_snooze'")
                .fetch_optional(&pool),
        )
        .unwrap();
    assert!(last.is_none());
}

#[test]
fn custom_snooze_then_unsnooze_keeps_workflow_without_persisting_last() {
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

    coordinator.finish(Completion {
        key,
        sequence: 45,
        command: PersistenceCommand::PatchTask(
            "task-1".into(),
            TaskPatch::Description("active".into()),
        ),
        error: None,
        related_revisions: HashMap::new(),
        created_task: None,
    });
    assert!(coordinator.drain(Duration::from_secs(2)));

    let row = runtime
        .block_on(
            sqlx::query("SELECT workflow_state, snoozed_until FROM tasks WHERE id = 1")
                .fetch_one(&pool),
        )
        .unwrap();
    assert_eq!(row.try_get::<String, _>("workflow_state").unwrap(), "todo");
    assert_eq!(
        row.try_get::<Option<String>, _>("snoozed_until").unwrap(),
        None
    );
    let last = runtime
        .block_on(
            sqlx::query("SELECT value FROM settings WHERE key = 'last_custom_snooze'")
                .fetch_optional(&pool),
        )
        .unwrap();
    assert!(last.is_none());
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
        created_task: None,
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
        created_task: None,
    }));
    assert!(coordinator.drain(Duration::from_secs(2)));

    let row = runtime
        .block_on(sqlx::query("SELECT snoozed_until FROM tasks WHERE id = 1").fetch_one(&pool))
        .unwrap();
    assert_eq!(
        row.try_get::<String, _>("snoozed_until").unwrap(),
        crate::snooze::format_datetime(quick)
    );
    let last = runtime
        .block_on(
            sqlx::query("SELECT value FROM settings WHERE key = 'last_custom_snooze'")
                .fetch_optional(&pool),
        )
        .unwrap();
    assert!(last.is_none());
    assert!(store.borrow().state().save_errors.is_empty());
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
        created_task: None,
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
    let workspace = Workspace::new(
        "workspace-1".into(),
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
        lead_workspace_ids: Vec::new(),
    }));
    coordinator.submit(PersistenceCommand::CreateWorkspace(workspace.clone()));
    coordinator.submit(PersistenceCommand::DeleteWorkspace(WorkspaceDeletion {
        workspace,
        task_ids: Vec::new(),
        was_default: false,
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
    let workspaces: i64 = runtime
        .block_on(sqlx::query("SELECT COUNT(*) AS count FROM workspaces").fetch_one(&pool))
        .unwrap()
        .try_get("count")
        .unwrap();
    let tags: i64 = runtime
        .block_on(sqlx::query("SELECT COUNT(*) AS count FROM tags").fetch_one(&pool))
        .unwrap()
        .try_get("count")
        .unwrap();
    assert_eq!((people, workspaces, tags), (0, 0, 0));
}

#[test]
fn failed_management_entity_deletes_restore_optimistic_state() {
    let (runtime, pool) = test_database();
    let store = test_store(Vec::new());
    let mut coordinator = test_coordinator(&runtime, &pool, Rc::clone(&store));

    coordinator.submit(PersistenceCommand::DeletePerson(PersonDeletion {
        person: Person::new("person-1".into(), "Ada".into(), String::new()),
        task_ids: Vec::new(),
        lead_workspace_ids: Vec::new(),
    }));
    coordinator.submit(PersistenceCommand::DeleteWorkspace(WorkspaceDeletion {
        workspace: Workspace::new(
            "workspace-1".into(),
            "CORE".into(),
            "Core".into(),
            String::new(),
        ),
        task_ids: Vec::new(),
        was_default: false,
    }));
    coordinator.submit(PersistenceCommand::DeleteTag(TagDeletion {
        tag: Tag::new("tag-1".into(), "api".into()),
        task_ids: Vec::new(),
    }));

    assert!(coordinator.drain(Duration::from_secs(2)));
    let state = store.borrow();
    assert_eq!(state.state().people[0].id, "person-1");
    assert_eq!(state.state().workspaces[0].id, "workspace-1");
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
