use super::*;
use crate::domain::{
    ChecklistItem, SaveTarget, TaskField, TaskLink, TaskRelation, TaskRelationKind,
    WorkspaceSnapshot,
};
use ratatui::{Terminal, backend::TestBackend};
use sqlx::any::AnyPoolOptions;
use tuicore::{
    FocusManager, HotkeyEvent, Key, KeyEvent, KeyModifiers, Propagation, TreeDispatcher,
};

fn test_task() -> Task {
    Task {
        id: "task-1".to_string(),
        rank: 1,
        created_at: String::new(),
        updated_at: String::new(),
        title: "Original".to_string(),
        state: TaskState::InProgress,
        size: TaskSize::Small,
        priority: TaskPriority::Medium,
        snoozed_until: None,
        people_ids: Vec::new(),
        workspace_id: None,
        tag_ids: Vec::new(),
        checklist: Vec::new(),
        links: Vec::new(),
        relations: Vec::new(),
        description: "Existing detail".to_string(),
    }
}

fn test_note(id: &str, position: i64) -> Versioned<NoteView> {
    Versioned {
        revision: 1,
        value: NoteView {
            id: id.into(),
            position,
            content: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
        },
    }
}

fn set_notes(app: &mut App, notes: Vec<Versioned<NoteView>>) {
    app.context
        .store
        .borrow_mut()
        .dispatch(AppEvent::NotesLoaded(notes));
}

#[test]
fn status_bar_enables_weather_and_exposes_forecast_menu() {
    assert!(weather_provider_config().is_enabled());
    assert!(STATUS_BAR_MENU_ITEMS.contains(&StatusBarMenuItem::WeatherForecast));
    assert_eq!(
        STATUS_BAR_MENU_ITEMS.first(),
        Some(&StatusBarMenuItem::Custom {
            id: SETTINGS_MENU_ID,
            label: " Settings",
        })
    );
}

#[test]
fn task_move_mode_uses_configured_control_m_and_emits_completed_order() {
    let mut table = task_table(
        vec![
            task_with_rank("first", "First", TaskState::Todo, 10),
            task_with_rank("second", "Second", TaskState::Todo, 20),
        ],
        Some("first"),
    );
    table.data_view_mut().highlight_id(&"first".to_string());
    let mut ctx = EventCtx::default();

    let plain_m = table.event(&TuiEvent::Key(KeyEvent::from(Key::Char('m'))), &mut ctx);
    assert!(!plain_m.handled());
    assert!(!table.is_reordering());

    table.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('m'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut ctx,
    );
    assert!(table.is_reordering());
    table.event(&TuiEvent::Key(KeyEvent::from(Key::Down)), &mut ctx);
    table.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);

    let reordered = table
        .take_events()
        .into_iter()
        .find_map(|event| match event {
            ListControlEvent::Reordered { row_ids } => Some(row_ids),
            _ => None,
        });
    assert_eq!(
        reordered,
        Some(vec!["second".to_string(), "first".to_string()])
    );
}

#[test]
fn settings_changes_update_app_state_before_persistence_completes() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let mut ctx = EventCtx::default();

    app.set_show_calendar_weekends(false);
    app.set_default_snooze_time(time::macros::time!(8:15));
    app.set_speed_reader_wpm("425".into(), &mut ctx);
    app.set_markdown_block_pause("1250".into(), &mut ctx);

    let state = store.borrow();
    assert_eq!(
        state.state().app_setting_values.get(SHOW_WEEKENDS_SETTING),
        Some(&"false".to_string())
    );
    assert_eq!(
        state
            .state()
            .app_setting_values
            .get(DEFAULT_SNOOZE_TIME_SETTING),
        Some(&"08:15".to_string())
    );
    assert_eq!(
        state
            .state()
            .app_setting_values
            .get(SPEED_READER_WPM_SETTING),
        Some(&"425".to_string())
    );
    assert_eq!(
        state
            .state()
            .app_setting_values
            .get(SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING),
        Some(&"1250".to_string())
    );
    assert!(ctx.notifications().is_empty());
}

#[test]
fn notes_zoom_saves_in_background_without_notification() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let coordinator = Rc::clone(&context.coordinator);
    let mut app = App::new(context.store, context.coordinator);
    let mut ctx = EventCtx::default();

    app.set_notes_zoom(crate::notes_config::NotesZoomLevels::default(), &mut ctx);

    assert!(coordinator.borrow().has_pending());
    assert!(ctx.notifications().is_empty());
}

#[test]
fn invalid_speed_reader_settings_warn_without_changing_state() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let mut valid_ctx = EventCtx::default();
    app.set_speed_reader_wpm("425".into(), &mut valid_ctx);
    app.set_markdown_block_pause("1250".into(), &mut valid_ctx);
    let mut invalid_ctx = EventCtx::default();

    app.set_speed_reader_wpm("99".into(), &mut invalid_ctx);
    app.set_markdown_block_pause("60001".into(), &mut invalid_ctx);

    let state = store.borrow();
    assert_eq!(
        state.state().app_setting_values[SPEED_READER_WPM_SETTING],
        "425"
    );
    assert_eq!(
        state.state().app_setting_values[SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING],
        "1250"
    );
    assert_eq!(
        invalid_ctx.notifications(),
        &[
            tuicore::Notification::warning(
                "Invalid speed reader WPM",
                "Enter a whole number from 100 to 1000.",
            ),
            tuicore::Notification::warning(
                "Invalid block delay",
                "Enter a whole number from 0 to 60000 ms.",
            ),
        ]
    );
}

#[test]
fn description_speed_reader_uses_current_dynamic_settings() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let mut settings_ctx = EventCtx::default();
    app.set_speed_reader_wpm("425".into(), &mut settings_ctx);
    app.set_markdown_block_pause("1250".into(), &mut settings_ctx);

    app.open_description_speed_reader("First word\n\nSecond".into(), &mut EventCtx::default());

    let AppDialog::SpeedReader(dialog) = app.primary_dialog().layer() else {
        panic!("speed reader dialog should be active");
    };
    let mut reader = dialog.child().clone();
    reader.play();
    <SpeedReader as TuiNode<AppMsg>>::tick(
        &mut reader,
        Duration::from_millis(140),
        AnimationSettings::default(),
    );
    assert_eq!(reader.current_word(), Some("First"));
    <SpeedReader as TuiNode<AppMsg>>::tick(
        &mut reader,
        Duration::from_millis(2),
        AnimationSettings::default(),
    );
    assert_eq!(reader.current_word(), Some("word"));
    <SpeedReader as TuiNode<AppMsg>>::tick(
        &mut reader,
        Duration::from_millis(1_390),
        AnimationSettings::default(),
    );
    assert_eq!(reader.current_word(), Some("word"));
    <SpeedReader as TuiNode<AppMsg>>::tick(
        &mut reader,
        Duration::from_millis(2),
        AnimationSettings::default(),
    );
    assert_eq!(reader.current_word(), Some("Second"));
}

#[test]
fn latest_speed_reader_save_failures_are_visible_and_restore_confirmed_values() {
    let (_runtime, _context, store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    for (key, confirmed, desired, generation) in [
        (SPEED_READER_WPM_SETTING, "300", "500", 2),
        (SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING, "250", "1250", 3),
    ] {
        store
            .borrow_mut()
            .dispatch(AppEvent::AppSettingChangeRequested {
                key: key.into(),
                value: confirmed.into(),
                generation: 1,
            });
        store
            .borrow_mut()
            .dispatch(AppEvent::AppSettingSaveCompleted {
                key: key.into(),
                value: confirmed.into(),
                generation: 1,
                error: None,
            });
        store
            .borrow_mut()
            .dispatch(AppEvent::AppSettingChangeRequested {
                key: key.into(),
                value: desired.into(),
                generation,
            });
    }
    let mut dialog = SettingsDialog::new(
        Rc::clone(&store),
        true,
        default_snooze_time(),
        &[],
        None,
        NoteEditingMode::Inline,
        500,
        Duration::from_millis(1_250),
    );
    store
        .borrow_mut()
        .dispatch(AppEvent::AppSettingSaveCompleted {
            key: SPEED_READER_WPM_SETTING.into(),
            value: "400".into(),
            generation: 1,
            error: Some("stale".into()),
        });
    for (key, value, generation, error) in [
        (SPEED_READER_WPM_SETTING, "500", 2, "WPM save failed"),
        (
            SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING,
            "1250",
            3,
            "delay save failed",
        ),
    ] {
        store
            .borrow_mut()
            .dispatch(AppEvent::AppSettingSaveCompleted {
                key: key.into(),
                value: value.into(),
                generation,
                error: Some(error.into()),
            });
    }

    dialog.layout(Rect::new(0, 0, 200, 16), &mut LayoutCtx::new());
    let text = rendered_text(&dialog, Rect::new(0, 0, 200, 16));
    let state = store.borrow();
    assert_eq!(
        state.state().app_setting_values[SPEED_READER_WPM_SETTING],
        "300"
    );
    assert_eq!(
        state.state().app_setting_values[SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING],
        "250"
    );
    assert!(text.contains("Setting save failed for speed_reader.wpm: WPM save failed"));
    assert!(text.contains(
        "Setting save failed for speed_reader.markdown_block_pause_ms: delay save failed"
    ));
    assert!(text.contains("300"));
    assert!(text.contains("250"));
    assert!(!text.contains("stale"));
}

#[test]
fn malformed_persisted_speed_reader_settings_fail_startup_loading() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    runtime.block_on(async {
        let service = TuidoService::connect_url("sqlite::memory:")
            .await
            .expect("service should connect");
        for (key, malformed) in [
            (SPEED_READER_WPM_SETTING, "99"),
            (SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING, "60001"),
        ] {
            service
                .set_app_setting(SPEED_READER_WPM_SETTING, "300")
                .await
                .unwrap();
            service
                .set_app_setting(SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING, "250")
                .await
                .unwrap();
            service.set_app_setting(key, malformed).await.unwrap();

            let error = load_speed_reader_settings(&service).await.unwrap_err();
            assert!(error.to_string().contains(key));
        }
    });
}

#[test]
fn task_tags_input_selects_existing_tags_and_creates_shared_candidates() {
    let api = Tag {
        id: "tag-api".to_string(),
        label: "api".to_string(),
    };
    let patches = Rc::new(RefCell::new(Vec::new()));
    let mut input = TaskTagsInput::new(
        &test_task(),
        std::slice::from_ref(&api),
        Rc::clone(&patches),
    );
    input.input.set_focused(true);
    let mut ctx = EventCtx::default();

    input.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
    for character in "api".chars() {
        input.event(
            &TuiEvent::Key(KeyEvent::from(Key::Char(character))),
            &mut ctx,
        );
    }
    input.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
    for character in "backend".chars() {
        input.event(
            &TuiEvent::Key(KeyEvent::from(Key::Char(character))),
            &mut ctx,
        );
    }
    input.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut ctx,
    );

    let patches = patches.borrow();
    let TaskPatch::Tags(tags) = patches.last().expect("tag changes should emit a patch") else {
        panic!("expected tags patch");
    };
    assert_eq!(tags.first(), Some(&api));
    assert_eq!(tags.get(1).map(|tag| tag.label.as_str()), Some("backend"));
    assert_ne!(tags[1].id, api.id);
    assert!(rendered_text(&input, Rect::new(0, 0, 40, 3)).contains("Add another tag"));
}

#[test]
fn task_tags_input_participates_in_control_focus_navigation() {
    let mut input = TaskTagsInput::new(&test_task(), &[], Rc::new(RefCell::new(Vec::new())));
    let mut layout = LayoutCtx::new();

    input.layout(Rect::new(0, 0, 40, 3), &mut layout);

    let target = layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "tag-input")
        .expect("tags input should register a focus target");
    assert!(target.enabled);
    assert!(target.control);
}

#[test]
fn task_tags_input_prompts_for_another_tag_when_one_is_selected() {
    let tag = Tag {
        id: "tag-study".to_string(),
        label: "study".to_string(),
    };
    let empty = TaskTagsInput::new(&test_task(), &[], Rc::new(RefCell::new(Vec::new())));
    let mut tagged_task = test_task();
    tagged_task.tag_ids.push(tag.id.clone());
    let tagged = TaskTagsInput::new(
        &tagged_task,
        std::slice::from_ref(&tag),
        Rc::new(RefCell::new(Vec::new())),
    );
    let area = Rect::new(0, 0, 40, 3);

    assert!(rendered_text(&empty, area).contains("No tags added"));
    assert!(rendered_text(&tagged, area).contains("Add another tag"));
}

fn task_with(id: &str, title: &str, state: TaskState) -> Task {
    let mut task = test_task();
    task.id = id.to_string();
    task.title = title.to_string();
    task.state = state;
    task
}

fn task_with_rank(id: &str, title: &str, state: TaskState, rank: i64) -> Task {
    let mut task = task_with(id, title, state);
    task.rank = rank;
    task
}

fn yank_task_table(table: &mut TaskTable) -> tuicore::DispatchEffects<AppMsg> {
    let mut ctx = EventCtx::default();
    let outcome = table.data_view_mut().event(&TuiEvent::Yank, &mut ctx);
    tuicore::DispatchEffects::from_event_ctx(outcome, ctx)
}

fn select_workspace_task(workspace: &mut TaskWorkspace, task_id: &str) {
    let task_id = task_id.to_string();
    workspace.table_mut().highlight_id(&task_id);
    workspace.table_mut().select_id(task_id.clone());
    workspace.table_mut().take_events();
    workspace.select_task(&task_id, &mut EventCtx::default());
}

pub(crate) fn test_context(
    snapshot: WorkspaceSnapshot,
) -> (tokio::runtime::Runtime, AppContext, AppStore) {
    sqlx::any::install_default_drivers();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    let pool = {
        let _runtime_guard = runtime.enter();
        AnyPoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .expect("lazy pool should build")
    };
    let store = Rc::new(RefCell::new(Store::new(
        AppState::from_snapshot(snapshot),
        reduce_app_state as fn(&mut AppState, AppEvent) -> tuicore::DispatchOutcome,
    )));
    let coordinator = Rc::new(RefCell::new(PersistenceCoordinator::new(
        Rc::clone(&store),
        pool,
        crate::storage::SqlDialect::Sqlite,
        runtime.handle().clone(),
        None,
    )));
    let context = AppContext::new(Rc::clone(&store), coordinator);
    (runtime, context, store)
}

pub(crate) fn rendered_text(node: &impl TuiNode<AppMsg>, area: Rect) -> String {
    let mut terminal =
        Terminal::new(TestBackend::new(area.width, area.height)).expect("terminal should build");
    terminal
        .draw(|frame| node.render(frame, area, &mut RenderCtx::new()))
        .expect("node should render");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn task_navigation_uses_the_most_specific_view_for_each_state() {
    for (state, expected_view) in [
        (TaskState::Backlog, TaskView::Backlog),
        (TaskState::Todo, TaskView::Active),
        (TaskState::InProgress, TaskView::Active),
        (TaskState::Snoozed, TaskView::Snoozed),
        (TaskState::Done, TaskView::Archived),
        (TaskState::Rejected, TaskView::Archived),
    ] {
        let mut target = test_task();
        target.id = format!("target-{}", state.id());
        target.state = state;
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![target.clone()],
            people: vec![],
            workspaces: vec![],
            tags: vec![],
        });
        let mut workspace = TaskWorkspace::new(context);
        *workspace.pending_navigation.borrow_mut() = Some(TaskNavigation {
            target_task_id: target.id.clone(),
            source_task_id: Some("source".into()),
            view: expected_view,
        });

        assert!(workspace.sync_navigation());
        assert_eq!(workspace.task_view, expected_view);
        assert_eq!(
            workspace.table().highlighted_id().as_deref(),
            Some(target.id.as_str())
        );
        assert_eq!(
            store.borrow().state().selected_task_id.as_deref(),
            Some(target.id.as_str())
        );
    }
}

#[test]
fn app_task_navigation_chooses_the_view_for_each_task_state() {
    for (state, expected_view) in [
        (TaskState::Backlog, TaskView::Backlog),
        (TaskState::Todo, TaskView::Active),
        (TaskState::InProgress, TaskView::Active),
        (TaskState::Snoozed, TaskView::Snoozed),
        (TaskState::Done, TaskView::Archived),
        (TaskState::Rejected, TaskView::Archived),
    ] {
        let mut target = test_task();
        target.state = state;
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![target.clone()],
            people: vec![],
            workspaces: vec![],
            tags: vec![],
        });
        let mut app = App::new(context.store, context.coordinator);
        app.active_tab.set(CALENDAR_TAB_INDEX);

        app.navigate_to_task("source".into(), target.id.clone(), &mut EventCtx::default());

        assert_eq!(app.active_tab.get(), TASKS_TAB_INDEX);
        assert_eq!(
            app.pending_task_navigation.borrow().as_ref(),
            Some(&TaskNavigation {
                target_task_id: target.id,
                source_task_id: Some("source".into()),
                view: expected_view,
            })
        );
    }
}

#[test]
fn global_h_returns_to_active_tasks_and_selects_the_first_row() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with_rank("backlog", "Backlog task", TaskState::Backlog, 0),
            task_with_rank("second", "Second active", TaskState::Todo, 20),
            task_with_rank("first", "First active", TaskState::InProgress, 10),
        ],
        people: vec![],
        workspaces: vec![],
        tags: vec![],
    });
    let mut app = App::new(context.store, context.coordinator);
    *app.pending_task_view.borrow_mut() = Some(TaskView::Backlog);
    let mut layout = LayoutCtx::new();
    app.layout(Rect::new(0, 0, 120, 30), &mut layout);
    let task_table_path = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "data-view"
                && !target
                    .path
                    .keys()
                    .iter()
                    .any(|part| matches!(part.as_str(), "checklist" | "links"))
        })
        .expect("task table should be focusable")
        .path
        .clone();
    let mut ctx = EventCtx::default();

    let outcome = app.dispatch_event(
        &EventRoute::new(task_table_path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('H'),
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut ctx,
    );
    app.layout(Rect::new(0, 0, 120, 30), &mut LayoutCtx::new());

    assert!(outcome.handled());
    assert_eq!(app.active_tab.get(), TASKS_TAB_INDEX);
    assert_eq!(
        store.borrow().state().selected_task_id.as_deref(),
        Some("first")
    );
    let text = rendered_text(&app, Rect::new(0, 0, 120, 30));
    assert!(text.contains("Active"));
    assert!(text.contains("First active"));
    assert!(!text.contains("Backlog task"));
}

#[test]
fn task_navigation_focuses_reverse_issue_link_on_destination_task() {
    let mut source = test_task();
    source.id = "source".into();
    source.relations.push(TaskRelation {
        task_id: "target".into(),
        kind: TaskRelationKind::Blocks,
    });
    let mut target = test_task();
    target.id = "target".into();
    target.relations.push(TaskRelation {
        task_id: "source".into(),
        kind: TaskRelationKind::IsBlockedBy,
    });
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![source, target],
        people: vec![],
        workspaces: vec![],
        tags: vec![],
    });
    let mut app = App::new(context.store, context.coordinator);
    app.navigate_to_task("source".into(), "target".into(), &mut EventCtx::default());
    let mut layout = LayoutCtx::new();
    app.layout(Rect::new(0, 0, 120, 80), &mut layout);
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(&issue_links_focus_request(), layout.focus_targets())
        .expect("Issue links control should accept navigation focus");
    let mut dispatcher = TreeDispatcher::new();
    dispatcher.dispatch_focus(&mut app, transition, AnimationSettings::default());
    let route = EventRoute::new(focus.current_path());

    let effects = dispatcher.dispatch_event(
        &mut app,
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        AnimationSettings::default(),
    );

    assert!(matches!(
        effects.messages.as_slice(),
        [AppMsg::NavigateToTask {
            source_task_id,
            target_task_id,
        }] if source_task_id == "target" && target_task_id == "source"
    ));
}

#[test]
fn tracked_tabs_apply_programmatic_selection_requests() {
    let selected = Rc::new(Cell::new(0));
    let tabs = Tabs::new(vec![
        Tab::text("First", "first"),
        Tab::text("Second", "second"),
    ]);
    let mut tracked = TrackedTabs::new(tabs, Rc::clone(&selected));
    selected.set(1);

    tracked.layout(Rect::new(0, 0, 40, 5), &mut LayoutCtx::new());

    assert_eq!(tracked.tabs.selected_index(), 1);
}

#[test]
fn empty_notes_tab_renders_seasonal_empty_state() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    app.active_tab.set(2);
    let area = Rect::new(0, 0, 120, 30);

    app.layout(area, &mut LayoutCtx::new());
    let text = rendered_text(&app, area);

    assert!(text.contains("Notes"));
    assert!(!text.contains("Note 1"));
    assert!(text.contains("Note |N|"));
    assert!(text.contains("No notes captured yet"));
    assert!(!text.contains("󰲋 Space"));
    assert!(!text.contains(" Tags"));
}

#[test]
fn notes_navigation_uses_the_full_app_focus_path() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    app.active_tab.set(2);
    set_notes(
        &mut app,
        vec![test_note("note-0", 0), test_note("note-1", 1)],
    );
    let mut layout = LayoutCtx::new();
    app.layout(Rect::new(0, 0, 120, 30), &mut layout);
    let panel = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target
                .path
                .keys()
                .last()
                .is_some_and(|key| key.as_str() == "panel-note-0")
        })
        .expect("first note panel should be focusable")
        .clone();
    let expected = panel
        .path
        .parent()
        .expect("note panel should have a parent path")
        .child(ChildKey::new("panel-note-1"));
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: panel.path.clone(),
                id: panel.id.clone(),
            },
            layout.focus_targets(),
        )
        .expect("first note panel should accept focus");
    let mut dispatcher = TreeDispatcher::new();
    dispatcher.dispatch_focus(&mut app, transition, AnimationSettings::default());

    let effects = dispatcher.dispatch_event(
        &mut app,
        &EventRoute::new(focus.current_path()),
        &TuiEvent::Key(KeyEvent::from(Key::Char('l'))),
        AnimationSettings::default(),
    );

    assert_eq!(effects.focus_request, Some(FocusRequest::Path(expected)));
}

#[test]
fn stale_note_draft_after_refresh_surfaces_conflict_without_overwriting_newer_note() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let mut newer = test_note("note-0", 0);
    newer.revision = 2;
    newer.value.content = "newer remote content".into();
    set_notes(&mut app, vec![newer]);

    app.patch_note(NoteChange {
        id: "note-0".into(),
        revision: 1,
        base_content: String::new(),
        content: "draft before quit".into(),
    });

    let state = app.context.store.borrow();
    assert_eq!(state.state().notes[0].value.content, "newer remote content");
    assert!(
        state
            .state()
            .note_error
            .as_deref()
            .is_some_and(|error| error.contains("conflict"))
    );
}

#[test]
fn new_note_from_tasks_switches_to_notes_and_focuses_notes_tab_content() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    app.layout(Rect::new(0, 0, 120, 30), &mut LayoutCtx::new());
    let mut ctx = EventCtx::default();

    app.create_note(&mut ctx);

    let state = app.context.store.borrow();
    assert_eq!(app.active_tab.get(), NOTES_TAB_INDEX);
    assert_eq!(state.state().notes.len(), 1);
    assert_eq!(state.state().notes[0].value.position, 0);
    assert!(Uuid::parse_str(&state.state().notes[0].value.id).is_ok());
    let note_id = state.state().notes[0].value.id.clone();
    assert_eq!(
        ctx.focus_request(),
        Some(&FocusRequest::Path(note_path(
            notes_workspace_focus_path(),
            &note_id,
        )))
    );
    drop(state);
    let mut layout = LayoutCtx::new();
    app.layout(Rect::new(0, 0, 120, 30), &mut layout);
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(
            ctx.focus_request().expect("new note should request focus"),
            layout.focus_targets(),
        )
        .expect("notes tab should have a focusable note");
    let panel = format!("panel-{note_id}");

    assert_eq!(
        transition
            .current
            .as_ref()
            .and_then(|target| target.path.keys().last())
            .map(ChildKey::as_str),
        Some(panel.as_str())
    );
}

#[test]
fn creating_a_note_enters_inline_editing() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let area = Rect::new(0, 0, 120, 30);
    app.layout(area, &mut LayoutCtx::new());
    let mut create = EventCtx::default();
    app.create_note(&mut create);
    let pending_id = app.context.store.borrow().state().notes[0].value.id.clone();
    let mut layout = LayoutCtx::new();
    app.layout(area, &mut layout);
    let target = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target
                .path
                .keys()
                .last()
                .is_some_and(|key| key.as_str() == format!("panel-{pending_id}"))
        })
        .expect("pending note should be focusable")
        .clone();
    assert!(target.focused_events_before_global_hotkeys);
    let route = EventRoute::new(target.path.clone());
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: target.path.clone(),
                id: target.id.clone(),
            },
            layout.focus_targets(),
        )
        .expect("pending note should accept focus");
    let mut dispatcher = TreeDispatcher::new();

    dispatcher.dispatch_focus(&mut app, transition, AnimationSettings::default());
    dispatcher.dispatch_event(
        &mut app,
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Char('x'))),
        AnimationSettings::default(),
    );
    let effects = dispatcher.dispatch_event(
        &mut app,
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Esc)),
        AnimationSettings::default(),
    );

    assert!(matches!(
        effects.messages.as_slice(),
        [AppMsg::PatchNote(NoteChange { id, content, .. })] if id == &pending_id && content == "x"
    ));
}

#[test]
fn creating_a_note_opens_the_external_editor_when_configured() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    app.context
        .store
        .borrow_mut()
        .dispatch(AppEvent::AppSettingChangeRequested {
            key: DEFAULT_NOTE_EDITING_SETTING.into(),
            value: NoteEditingMode::External.setting_value().into(),
            generation: 1,
        });
    let area = Rect::new(0, 0, 120, 30);
    app.layout(area, &mut LayoutCtx::new());
    app.create_note(&mut EventCtx::default());
    let pending_id = app.context.store.borrow().state().notes[0].value.id.clone();
    let mut layout = LayoutCtx::new();
    app.layout(area, &mut layout);
    let target = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target
                .path
                .keys()
                .last()
                .is_some_and(|key| key.as_str() == format!("panel-{pending_id}"))
        })
        .expect("pending note should be focusable")
        .clone();
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: target.path.clone(),
                id: target.id.clone(),
            },
            layout.focus_targets(),
        )
        .expect("pending note should accept focus");
    let mut dispatcher = TreeDispatcher::new();

    let effects = dispatcher.dispatch_focus(&mut app, transition, AnimationSettings::default());

    assert_eq!(
        effects
            .external_editor
            .as_ref()
            .and_then(|request| request.file_extension.as_deref()),
        Some("md")
    );
}

#[test]
fn note_editing_keeps_new_task_hotkey_as_text() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    app.active_tab.set(NOTES_TAB_INDEX);
    set_notes(&mut app, vec![test_note("note-0", 0)]);
    let area = Rect::new(0, 0, 120, 30);
    let mut layout = LayoutCtx::new();
    app.layout(area, &mut layout);
    let target = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target
                .path
                .keys()
                .last()
                .is_some_and(|key| key.as_str() == "panel-note-0")
        })
        .expect("note should be focusable")
        .clone();
    let route = EventRoute::new(target.path.clone());
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: target.path,
                id: target.id,
            },
            layout.focus_targets(),
        )
        .expect("note should accept focus");
    let mut dispatcher = TreeDispatcher::new();
    dispatcher.dispatch_focus(&mut app, transition, AnimationSettings::default());
    dispatcher.dispatch_event(
        &mut app,
        &route,
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        AnimationSettings::default(),
    );

    let effects = dispatcher.dispatch_event(
        &mut app,
        &route,
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('K'),
            modifiers: KeyModifiers::SHIFT,
        }),
        AnimationSettings::default(),
    );

    assert!(
        !effects
            .messages
            .iter()
            .any(|message| matches!(message, AppMsg::OpenCreateTask { .. }))
    );
}

#[test]
fn new_note_removes_empty_notes_before_background_create_completes() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let mut empty = test_note("empty", 0);
    empty.value.content = "\n\t ".into();
    let mut populated = test_note("populated", 1);
    populated.value.content = "Keep this".into();
    set_notes(&mut app, vec![empty, populated]);

    app.create_note(&mut EventCtx::default());

    let state = app.context.store.borrow();
    assert_eq!(state.state().notes.len(), 2);
    assert_eq!(state.state().notes_version, 2);
    assert!(Uuid::parse_str(&state.state().notes[0].value.id).is_ok());
    assert_eq!(state.state().notes[1].value.id, "populated");
    assert_eq!(state.state().notes[1].value.position, 1);
    assert_eq!(
        state
            .state()
            .note_placeholders
            .get(&state.state().notes[0].value.id),
        Some(&note_placeholder("empty").to_string())
    );
}

#[test]
fn new_note_inherits_the_selected_empty_note_placeholder() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    set_notes(
        &mut app,
        vec![test_note("first", 0), test_note("selected", 1)],
    );
    app.active_focus_path = Some(note_path(TreePath::new(), "selected"));

    app.create_note(&mut EventCtx::default());

    let state = app.context.store.borrow();
    assert_eq!(
        state
            .state()
            .note_placeholders
            .get(&state.state().notes[0].value.id),
        Some(&note_placeholder("selected").to_string())
    );
}

#[test]
fn task_creation_from_notes_restores_cancel_focus_and_submission_focuses_tasks() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    app.active_tab.set(NOTES_TAB_INDEX);
    set_notes(&mut app, vec![test_note("note-0", 0)]);
    let mut layout = LayoutCtx::new();
    app.layout(Rect::new(0, 0, 120, 30), &mut layout);
    let note = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target
                .path
                .keys()
                .last()
                .is_some_and(|key| key.as_str() == "panel-note-0")
        })
        .expect("note panel should be focusable")
        .clone();
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: note.path.clone(),
                id: note.id.clone(),
            },
            layout.focus_targets(),
        )
        .expect("note panel should accept focus");
    let mut dispatcher = TreeDispatcher::new();
    dispatcher.dispatch_focus(&mut app, transition, AnimationSettings::default());
    let route = EventRoute::new(focus.current_path());

    let open = dispatcher.dispatch_event(
        &mut app,
        &route,
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_QUICK_CREATE.hotkey())),
        AnimationSettings::default(),
    );
    assert!(open.outcome.handled());
    assert!(matches!(
        open.messages.as_slice(),
        [AppMsg::OpenCreateTask {
            calendar_date: None
        }]
    ));
    app.open_create_task_dialog(None, &mut EventCtx::default());
    let mut cancel = EventCtx::default();
    app.close_dialog(&mut cancel);

    assert_eq!(app.active_tab.get(), NOTES_TAB_INDEX);
    assert_eq!(
        cancel.focus_request(),
        Some(&FocusRequest::Path(note.path.clone()))
    );

    let reopen = dispatcher.dispatch_event(
        &mut app,
        &route,
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_QUICK_CREATE.hotkey())),
        AnimationSettings::default(),
    );
    assert!(reopen.outcome.handled());
    app.open_create_task_dialog(None, &mut EventCtx::default());
    let mut submit = EventCtx::default();
    app.submit_create_task(
        CreateTaskDraft {
            title: "Task from notes".into(),
        },
        &mut submit,
    );

    assert_eq!(app.active_tab.get(), TASKS_TAB_INDEX);
    let task = store
        .borrow()
        .state()
        .tasks
        .first()
        .expect("task should be created")
        .clone();
    assert_eq!(task.title, "Task from notes");
    assert_eq!(task.rank, -1);
    assert_eq!(
        app.pending_task_navigation.borrow().as_ref(),
        Some(&TaskNavigation {
            target_task_id: task.id.clone(),
            source_task_id: None,
            view: TaskView::Backlog,
        })
    );
    assert_eq!(
        submit.focus_request(),
        Some(&initial_task_table_focus_request())
    );

    app.layout(Rect::new(0, 0, 120, 30), &mut LayoutCtx::new());

    assert!(app.pending_task_navigation.borrow().is_none());
    assert_eq!(
        store.borrow().state().selected_task_id.as_deref(),
        Some(task.id.as_str())
    );
    assert!(rendered_text(&app, Rect::new(0, 0, 120, 30)).contains("Backlog"));
}

#[test]
fn deleting_a_note_opens_the_standard_confirmation_dialog() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    set_notes(&mut app, vec![test_note("note-0", 0)]);

    app.open_delete_note_dialog("note-0".into(), &mut EventCtx::default());

    assert!(app.primary_dialog().is_active());
    assert!(matches!(
        app.primary_dialog().layer(),
        AppDialog::DeleteNote(_)
    ));
    let text = rendered_text(app.primary_dialog().layer(), Rect::new(0, 0, 80, 10));
    assert!(text.contains("Delete note?"));
    assert!(text.contains("Delete this note? This cannot be undone."));
}

#[test]
fn missing_note_delete_target_closes_the_quick_menu() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    set_notes(&mut app, vec![test_note("note-0", 0)]);
    app.open_note_quick_menu("note-0".into(), &mut EventCtx::default());
    let mut ctx = EventCtx::default();

    app.open_delete_note_dialog("missing".into(), &mut ctx);

    assert!(!app.primary_dialog().is_active());
}

#[test]
fn deleting_a_note_focuses_next_or_falls_back_to_previous() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    app.active_tab.set(2);
    set_notes(
        &mut app,
        vec![
            test_note("note-0", 0),
            test_note("note-1", 1),
            test_note("note-2", 2),
        ],
    );
    app.layout(Rect::new(0, 0, 120, 30), &mut LayoutCtx::new());
    let mut next = EventCtx::default();

    app.delete_note("note-1".into(), &mut next);

    let state = app.context.store.borrow();
    assert_eq!(
        state
            .state()
            .notes
            .iter()
            .map(|note| note.value.id.as_str())
            .collect::<Vec<_>>(),
        ["note-0", "note-2"]
    );
    assert_eq!(
        next.focus_request(),
        Some(&FocusRequest::Path(note_path(
            app.note_focus_path.borrow().clone(),
            "note-2",
        )))
    );
    drop(state);
    let mut previous = EventCtx::default();

    app.delete_note("note-2".into(), &mut previous);

    assert_eq!(
        previous.focus_request(),
        Some(&FocusRequest::Path(note_path(
            app.note_focus_path.borrow().clone(),
            "note-0",
        )))
    );
}

#[test]
fn deleting_the_last_note_focuses_app_tabs() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    app.active_tab.set(NOTES_TAB_INDEX);
    set_notes(&mut app, vec![test_note("note-0", 0)]);
    let mut ctx = EventCtx::default();

    app.delete_note("note-0".into(), &mut ctx);

    assert!(app.context.store.borrow().state().notes.is_empty());
    assert_eq!(ctx.focus_request(), Some(&app_tabs_focus_request()));
}

#[test]
fn bulk_task_state_update_changes_every_selected_task_with_one_pending_write() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with("first", "First", TaskState::Todo),
            task_with("second", "Second", TaskState::Todo),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let coordinator = Rc::clone(&context.coordinator);
    let mut app = App::new(context.store, context.coordinator);

    app.complete_tasks(
        vec!["second".into(), "first".into()],
        TaskState::InProgress,
        None,
        &mut EventCtx::default(),
    );

    assert!(
        store
            .borrow()
            .state()
            .tasks
            .iter()
            .all(|task| task.state == TaskState::InProgress)
    );
    assert!(coordinator.borrow().has_pending());
}

#[test]
fn bulk_snooze_then_unsnooze_updates_the_selected_group() {
    let until = time::macros::datetime!(2026-08-24 8:00);
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with("first", "First", TaskState::Todo),
            task_with("second", "Second", TaskState::InProgress),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);

    app.snooze_tasks(
        vec!["second".into(), "first".into()],
        until,
        Some(until),
        None,
        &mut EventCtx::default(),
    );
    assert!(
        store
            .borrow()
            .state()
            .tasks
            .iter()
            .all(|task| { task.state == TaskState::Snoozed && task.snoozed_until == Some(until) })
    );

    app.unsnooze_tasks(
        vec!["first".into(), "second".into()],
        None,
        &mut EventCtx::default(),
    );
    assert!(
        store
            .borrow()
            .state()
            .tasks
            .iter()
            .all(|task| { task.state == TaskState::Todo && task.snoozed_until.is_none() })
    );
}

#[test]
fn selected_task_helpers_use_source_order_not_selection_order() {
    let state = AppState::from_snapshot(WorkspaceSnapshot {
        tasks: vec![
            task_with("first", "First", TaskState::Todo),
            task_with("second", "Second", TaskState::Todo),
            task_with("third", "Third", TaskState::Todo),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });

    let selected = tasks_for_ids(&state, &["third".into(), "first".into()]);

    assert_eq!(
        selected
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["first", "third"]
    );
    assert_eq!(
        task_block_edge_availability(
            &["first".into(), "second".into(), "third".into()],
            &["third".into(), "first".into()]
        ),
        (false, false)
    );
    assert_eq!(
        task_block_edge_availability(
            &["first".into(), "second".into(), "third".into()],
            &["second".into(), "first".into()]
        ),
        (false, true)
    );
    assert_eq!(
        task_block_edge_availability(
            &["first".into(), "second".into(), "third".into()],
            &["third".into(), "second".into()]
        ),
        (true, false)
    );
}

#[test]
fn selected_task_copy_payloads_use_rank_order_and_escape_titles() {
    let workspace = Workspace::new(
        "workspace".into(),
        "PER".into(),
        "Personal".into(),
        String::new(),
    );
    let mut first = task_with_rank("old-45", "First \"quoted\"", TaskState::Todo, 10);
    first.workspace_id = Some(workspace.id.clone());
    let mut second = task_with_rank("old-46", "Second \\ path", TaskState::Todo, 20);
    second.workspace_id = Some(workspace.id.clone());
    let state = AppState::from_snapshot(WorkspaceSnapshot {
        tasks: vec![second, first],
        people: Vec::new(),
        workspaces: vec![workspace],
        tags: Vec::new(),
    });
    let selected = tasks_for_ids(&state, &["old-46".into(), "old-45".into()]);

    assert_eq!(
        task_agent_commands_for(&state, &selected, "execute"),
        Some(r#"Tuido execute PER-45 "First \"quoted\""; PER-46 "Second \\ path""#.into())
    );
    assert_eq!(
        task_agent_commands_for(&state, &selected, "clarify"),
        Some(r#"Tuido clarify PER-45 "First \"quoted\""; PER-46 "Second \\ path""#.into())
    );
    assert_eq!(
        task_references_for(&state, &selected),
        r#"Tuido PER-45 "First \"quoted\""; PER-46 "Second \\ path""#
    );
}

#[test]
fn selection_invocation_matrix_clears_task_and_calendar_groups_once_for_accepted_copy() {
    for source in [
        TransientSelectionSource::TaskList,
        TransientSelectionSource::Calendar,
    ] {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![task_with("first", "First", TaskState::Todo)],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let invocation = context
            .begin_transient_selection(source, vec!["first".into()], Some("first".into()))
            .expect("group selection should create an invocation");

        context.accept_transient_clipboard(Some(invocation));

        assert!(context.take_selection_clear_request(invocation));
        assert!(!context.take_selection_clear_request(invocation));
    }
}

#[test]
fn quick_menu_copy_messages_clear_each_source_once_before_closing() {
    for source in [
        TransientSelectionSource::TaskList,
        TransientSelectionSource::Calendar,
    ] {
        for (event, expected) in [
            (TuiEvent::Yank, "reference"),
            (
                TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_AGENT_YANK.hotkey())),
                "execute",
            ),
            (
                TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_AGENT_YANK_CLARIFY.hotkey())),
                "clarify",
            ),
        ] {
            let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
                tasks: vec![task_with("first", "First", TaskState::Todo)],
                people: Vec::new(),
                workspaces: Vec::new(),
                tags: Vec::new(),
            });
            let mut app = App::new(Rc::clone(&context.store), Rc::clone(&context.coordinator));
            let invocation = app
                .context
                .begin_transient_selection(source, vec!["first".into()], Some("first".into()))
                .unwrap();
            let mut menu = TaskQuickMenu::new_multiple(
                vec!["first".into()],
                vec![TaskState::Todo],
                TaskQuickClipboard {
                    execute: Some("execute".into()),
                    clarify: Some("clarify".into()),
                    reference: Some("reference".into()),
                },
                false,
                false,
            );
            menu.set_selection_invocation(Some(invocation));
            let mut message_queue = EventCtx::default();

            assert!(menu.event(&event, &mut message_queue).handled());
            let (action_invocation, payload) = match message_queue.messages() {
                [
                    AppMsg::SelectionAction {
                        invocation: action_invocation,
                        action,
                    },
                ] if *action_invocation == invocation => match action.as_ref() {
                    AppMsg::CopyTaskClipboard(payload) => (invocation, payload.clone()),
                    message => panic!("unexpected quick-menu action: {message:?}"),
                },
                messages => panic!("unexpected quick-menu messages: {messages:?}"),
            };
            let mut delivery = EventCtx::default();
            app.dispatch_selection_action(
                action_invocation,
                AppMsg::CopyTaskClipboard(payload),
                &mut delivery,
            );

            assert_eq!(delivery.clipboard_request(), Some(expected));
            assert!(app.context.take_selection_clear_request(invocation));
            assert!(!app.context.take_selection_clear_request(invocation));
        }
    }
}

#[test]
fn selection_invocation_matrix_cancel_and_pre_submit_failure_preserve_or_clear_as_required() {
    for source in [
        TransientSelectionSource::TaskList,
        TransientSelectionSource::Calendar,
    ] {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![task_with("first", "First", TaskState::Todo)],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let cancelled = context
            .begin_transient_selection(source, vec!["first".into()], Some("first".into()))
            .unwrap();

        context.cancel_transient_selection(cancelled);

        assert!(!context.take_selection_clear_request(cancelled));

        let accepted = context
            .begin_transient_selection(source, vec!["first".into()], Some("first".into()))
            .unwrap();
        context.retire_transient_selection_without_persistence(Some(accepted));

        assert!(context.take_selection_clear_request(accepted));
        assert!(!context.take_selection_clear_request(accepted));
    }
}

#[test]
fn selection_invocation_matrix_async_rollback_restores_only_the_start_highlight() {
    for source in [
        TransientSelectionSource::TaskList,
        TransientSelectionSource::Calendar,
    ] {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![task_with("first", "First", TaskState::Todo)],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let invocation = context
            .begin_transient_selection(source, vec!["first".into()], Some("first".into()))
            .unwrap();
        context.accept_transient_mutation(Some(invocation));

        assert!(context.take_selection_clear_request(invocation));
        context
            .coordinator
            .borrow_mut()
            .record_selection_outcome_for_test(invocation, true);
        context.resolve_persistence_selection_outcomes();

        assert_eq!(
            context.rollback_transient_selection_highlight(invocation),
            Some("first".into())
        );
        assert!(!context.take_selection_clear_request(invocation));
    }
}

#[test]
fn selection_invocation_matrix_unrelated_refresh_does_not_clear_a_new_group() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with("first", "First", TaskState::Todo),
            task_with("second", "Second", TaskState::Todo),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let invocation_a = context
        .begin_transient_selection(
            TransientSelectionSource::TaskList,
            vec!["first".into()],
            Some("first".into()),
        )
        .unwrap();
    context.accept_transient_mutation(Some(invocation_a));
    assert!(context.take_selection_clear_request(invocation_a));
    store
        .borrow_mut()
        .dispatch(AppEvent::AppSettingChangeRequested {
            key: "unrelated".into(),
            value: "value".into(),
            generation: 1,
        });
    context.resolve_persistence_selection_outcomes();
    let invocation_b = context
        .begin_transient_selection(
            TransientSelectionSource::TaskList,
            vec!["second".into()],
            Some("second".into()),
        )
        .unwrap();

    assert!(!context.take_selection_clear_request(invocation_b));
}

#[test]
fn store_refresh_keeps_transient_task_selection_for_table_actions() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with("first", "First", TaskState::Todo),
            task_with("second", "Second", TaskState::Todo),
            task_with("third", "Third", TaskState::Todo),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.table_mut().highlight_id(&"first".to_string());
    workspace.task_list_mut().event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Down,
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut EventCtx::default(),
    );
    assert_eq!(
        workspace.task_list().transient_selected_ids(),
        ["first", "second"]
    );

    store
        .borrow_mut()
        .dispatch(AppEvent::AppSettingChangeRequested {
            key: "unrelated".into(),
            value: "value".into(),
            generation: 1,
        });
    workspace.sync_store_version();
    assert_eq!(
        workspace.task_list().transient_selected_ids(),
        ["first", "second"]
    );
    workspace.table_focused = true;
    let mut action_ctx = EventCtx::default();
    workspace.handle_workspace_event(
        EventOutcome::Ignored,
        &TuiEvent::Key(KeyEvent::from(Key::Char('.'))),
        &mut action_ctx,
    );

    assert!(matches!(
        action_ctx.messages(),
        [AppMsg::SelectionAction { action, .. }]
            if matches!(action.as_ref(), AppMsg::OpenTasksQuickMenu(task_ids) if task_ids == &["first", "second"])
    ));
}

#[test]
fn one_item_task_list_selection_routes_every_direct_action_to_group_handlers() {
    let actions = [
        (
            TuiEvent::Key(KeyEvent {
                code: Key::Char('c'),
                modifiers: KeyModifiers::CONTROL,
            }),
            "complete",
        ),
        (
            TuiEvent::Key(KeyEvent {
                code: Key::Char('t'),
                modifiers: KeyModifiers::CONTROL,
            }),
            "progress",
        ),
        (
            TuiEvent::Key(KeyEvent {
                code: Key::Char('x'),
                modifiers: KeyModifiers::CONTROL,
            }),
            "delete",
        ),
        (
            TuiEvent::Key(KeyEvent {
                code: Key::Char('z'),
                modifiers: KeyModifiers::CONTROL,
            }),
            "snooze",
        ),
    ];
    for (event, action) in actions {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![task_with("first", "First", TaskState::Todo)],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        workspace.table_mut().highlight_id(&"first".to_string());
        workspace.task_list_mut().event(
            &TuiEvent::Key(KeyEvent {
                code: Key::Char(' '),
                modifiers: KeyModifiers::CONTROL,
            }),
            &mut EventCtx::default(),
        );
        assert_eq!(workspace.task_list().transient_selected_ids(), ["first"]);
        workspace.table_focused = true;
        let mut ctx = EventCtx::default();

        workspace.handle_workspace_event(EventOutcome::Ignored, &event, &mut ctx);

        let [
            AppMsg::SelectionAction {
                action: message, ..
            },
        ] = ctx.messages()
        else {
            panic!(
                "unexpected task-list {action} message: {:?}",
                ctx.messages()
            );
        };
        assert!(match (action, message.as_ref()) {
            ("complete", AppMsg::OpenCompleteTasks(task_ids))
            | ("delete", AppMsg::OpenDeleteTasks(task_ids))
            | ("snooze", AppMsg::OpenTasksSnooze(task_ids)) => {
                task_ids == &vec![String::from("first")]
            }
            (
                "progress",
                AppMsg::CompleteTasks {
                    task_ids,
                    state: TaskState::InProgress,
                },
            ) => task_ids == &vec![String::from("first")],
            (_, message) => panic!("unexpected task-list {action} message: {message:?}"),
        });
    }
}

#[test]
fn partial_stale_menu_action_keeps_its_original_selection_guard() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with("first", "First", TaskState::Todo),
            task_with("second", "Second", TaskState::Todo),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(Rc::clone(&context.store), Rc::clone(&context.coordinator));
    let task_ids = vec!["first".into(), "second".into()];
    let invocation = app
        .context
        .begin_transient_selection(
            TransientSelectionSource::TaskList,
            task_ids.clone(),
            Some("first".into()),
        )
        .unwrap();
    store
        .borrow_mut()
        .dispatch(AppEvent::TaskDeleted("second".into()));

    app.complete_tasks(
        task_ids,
        TaskState::Done,
        Some(invocation),
        &mut EventCtx::default(),
    );

    assert!(app.context.take_selection_clear_request(invocation));
}

#[test]
fn selection_outcomes_ignore_mismatched_and_unrelated_invocations() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![task_with("first", "First", TaskState::Todo)],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let invocation = context
        .begin_transient_selection(
            TransientSelectionSource::TaskList,
            vec!["first".into()],
            Some("first".into()),
        )
        .unwrap();
    context.accept_transient_mutation(Some(invocation));
    let coordinator = Rc::clone(&context.coordinator);
    coordinator.borrow_mut().record_selection_outcome_for_test(
        PersistenceSelectionInvocation {
            token: invocation.token,
            source: PersistenceSelectionSource::Calendar,
        },
        true,
    );
    coordinator.borrow_mut().record_selection_outcome_for_test(
        PersistenceSelectionInvocation {
            token: invocation.token + 1,
            source: PersistenceSelectionSource::TaskList,
        },
        true,
    );

    context.resolve_persistence_selection_outcomes();

    assert_eq!(
        context.rollback_transient_selection_highlight(invocation),
        None
    );
}

#[test]
fn failed_outcome_for_a_restores_its_highlight_after_b_is_queued() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with("first", "First", TaskState::Todo),
            task_with("second", "Second", TaskState::Todo),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let invocation_a = context
        .begin_transient_selection(
            TransientSelectionSource::TaskList,
            vec!["first".into()],
            Some("first".into()),
        )
        .unwrap();
    context.accept_transient_mutation(Some(invocation_a));
    let _invocation_b = context
        .begin_transient_selection(
            TransientSelectionSource::TaskList,
            vec!["second".into()],
            Some("second".into()),
        )
        .unwrap();
    context
        .coordinator
        .borrow_mut()
        .record_selection_outcome_for_test(invocation_a, true);

    context.resolve_persistence_selection_outcomes();

    assert_eq!(
        context.rollback_transient_selection_highlight(invocation_a),
        Some("first".into())
    );
}

#[test]
fn task_list_quick_menu_keeps_multi_selection_after_focus_moves_to_dialog() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with("first", "First", TaskState::Todo),
            task_with("second", "Second", TaskState::Todo),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let area = Rect::new(0, 0, 120, 40);
    let mut layout = LayoutCtx::new();
    workspace.layout(area, &mut layout);
    let table = layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "data-view")
        .expect("task table should be focusable")
        .clone();
    let route = EventRoute::new(table.path.clone());
    workspace.dispatch_focus(&table, true, &mut FocusCtx::default());

    workspace.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent {
            code: Key::Down,
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut EventCtx::default(),
    );
    let mut quick_menu = EventCtx::default();
    assert!(
        workspace
            .dispatch_event(
                &route,
                &TuiEvent::Key(Key::Char('.').into()),
                &mut quick_menu,
            )
            .handled()
    );
    assert!(matches!(
        quick_menu.messages(),
        [AppMsg::SelectionAction { action, .. }]
            if matches!(action.as_ref(), AppMsg::OpenTasksQuickMenu(task_ids) if task_ids == &["first", "second"])
    ));

    workspace.dispatch_focus(&table, false, &mut FocusCtx::default());

    assert_eq!(
        workspace.task_list().transient_selected_ids(),
        ["first", "second"]
    );
}

#[test]
fn task_list_group_selection_clears_after_order_independent_completion() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with("first", "First", TaskState::Todo),
            task_with("second", "Second", TaskState::Todo),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context.clone());
    workspace.table_mut().highlight_id(&"first".to_string());
    workspace.task_list_mut().event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Down,
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut EventCtx::default(),
    );
    let action_start_highlight = workspace.table().highlighted_id();
    workspace.table_focused = true;
    workspace.handle_workspace_event(
        EventOutcome::Ignored,
        &TuiEvent::Key(Key::Char('.').into()),
        &mut EventCtx::default(),
    );

    workspace.sync_store_version();
    assert_eq!(
        workspace.task_list().transient_selected_ids(),
        ["first", "second"]
    );

    for task_id in ["first", "second"] {
        context.store.borrow_mut().dispatch(AppEvent::PatchTask {
            task_id: task_id.into(),
            patch: TaskPatch::State(TaskState::InProgress),
        });
    }
    context.accept_transient_mutation(workspace.active_selection_invocation);
    workspace.sync_store_version();

    assert!(workspace.task_list().transient_selected_ids().is_empty());
    assert_eq!(
        workspace.table().highlighted_id().as_deref(),
        action_start_highlight.as_deref()
    );
}

#[test]
fn unrelated_refresh_preserves_new_group_selection_after_completed_group_sync() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with("first", "First", TaskState::InProgress),
            task_with("second", "Second", TaskState::InProgress),
            task_with("third", "Third", TaskState::InProgress),
            task_with("fourth", "Fourth", TaskState::InProgress),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context.clone());
    workspace.table_mut().highlight_id(&"first".to_string());
    workspace.task_list_mut().event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Down,
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut EventCtx::default(),
    );
    workspace.table_focused = true;
    workspace.handle_workspace_event(
        EventOutcome::Ignored,
        &TuiEvent::Key(Key::Char('.').into()),
        &mut EventCtx::default(),
    );
    for task_id in ["first", "second"] {
        store.borrow_mut().dispatch(AppEvent::PatchTask {
            task_id: task_id.into(),
            patch: TaskPatch::State(TaskState::Done),
        });
    }
    context.accept_transient_mutation(workspace.active_selection_invocation);
    workspace.sync_store_version();
    assert!(workspace.task_list().transient_selected_ids().is_empty());

    workspace.table_mut().highlight_id(&"third".to_string());
    workspace.task_list_mut().event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Down,
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut EventCtx::default(),
    );
    let new_highlight = workspace.table().highlighted_id();
    assert_eq!(
        workspace.task_list().transient_selected_ids(),
        ["third", "fourth"]
    );

    let state = store.borrow().state().clone();
    store.borrow_mut().dispatch(AppEvent::WorkspaceRefreshed {
        snapshot: WorkspaceSnapshot {
            tasks: state.tasks,
            people: state.people,
            workspaces: state.workspaces,
            tags: state.tags,
        },
        revision: state.workspace_revision + 1,
        entity_revisions: state.entity_revisions,
    });
    workspace.sync_store_version();

    assert_eq!(
        workspace.task_list().transient_selected_ids(),
        ["third", "fourth"]
    );
    assert_eq!(workspace.table().highlighted_id(), new_highlight);
}

#[test]
fn archived_group_completion_clears_selection_after_rank_canonicalization() {
    let mut first = task_with_rank("first", "First", TaskState::Done, 1);
    first.updated_at = "1".into();
    let mut second = task_with_rank("second", "Second", TaskState::Done, 2);
    second.updated_at = "2".into();
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![first, second],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context.clone());
    *workspace.pending_task_view.borrow_mut() = Some(TaskView::Archived);
    assert!(workspace.sync_task_view_change());
    workspace.table_mut().highlight_id(&"second".to_string());
    workspace.task_list_mut().event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Down,
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut EventCtx::default(),
    );
    let action_start_highlight = workspace.table().highlighted_id();
    workspace.table_focused = true;
    workspace.handle_workspace_event(
        EventOutcome::Ignored,
        &TuiEvent::Key(KeyEvent::from(Key::Char('.'))),
        &mut EventCtx::default(),
    );
    assert_eq!(
        workspace.task_list().transient_selected_ids(),
        ["second", "first"]
    );

    for task_id in ["first", "second"] {
        context.store.borrow_mut().dispatch(AppEvent::PatchTask {
            task_id: task_id.into(),
            patch: TaskPatch::State(TaskState::Rejected),
        });
    }
    context.accept_transient_mutation(workspace.active_selection_invocation);
    workspace.sync_store_version();

    assert!(workspace.task_list().transient_selected_ids().is_empty());
    assert_eq!(
        workspace.table().highlighted_id().as_deref(),
        action_start_highlight.as_deref()
    );
}

fn assert_failed_bulk_action_restores_task_highlight(
    tasks: Vec<Task>,
    action: impl FnOnce(&mut App, PersistenceSelectionInvocation, &mut EventCtx<AppMsg>),
) {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: tasks.clone(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let mut workspace = TaskWorkspace::new(app.context.clone());
    workspace.table_mut().highlight_id(&"second".to_string());
    let invocation = app
        .context
        .begin_transient_selection(
            TransientSelectionSource::TaskList,
            vec!["first".into(), "second".into()],
            Some("second".into()),
        )
        .unwrap();
    workspace.active_selection_invocation = Some(invocation);
    action(&mut app, invocation, &mut EventCtx::default());
    workspace.sync_store_version();

    store.borrow_mut().dispatch(AppEvent::TasksRestored(tasks));
    app.context
        .coordinator
        .borrow_mut()
        .record_selection_outcome_for_test(invocation, true);
    workspace.sync_store_version();

    assert_eq!(
        workspace.table().highlighted_id().as_deref(),
        Some("second")
    );
}

#[test]
fn failed_bulk_delete_restores_action_start_highlight() {
    assert_failed_bulk_action_restores_task_highlight(
        vec![
            task_with("first", "First", TaskState::InProgress),
            task_with("second", "Second", TaskState::InProgress),
        ],
        |app, invocation, ctx| {
            app.delete_tasks(vec!["first".into(), "second".into()], Some(invocation), ctx)
        },
    );
}

#[test]
fn failed_bulk_state_change_restores_action_start_highlight() {
    assert_failed_bulk_action_restores_task_highlight(
        vec![
            task_with("first", "First", TaskState::InProgress),
            task_with("second", "Second", TaskState::InProgress),
        ],
        |app, invocation, ctx| {
            app.complete_tasks(
                vec!["first".into(), "second".into()],
                TaskState::Done,
                Some(invocation),
                ctx,
            )
        },
    );
}

#[test]
fn failed_bulk_snooze_restores_action_start_highlight() {
    assert_failed_bulk_action_restores_task_highlight(
        vec![
            task_with("first", "First", TaskState::InProgress),
            task_with("second", "Second", TaskState::InProgress),
        ],
        |app, invocation, ctx| {
            app.snooze_tasks(
                vec!["first".into(), "second".into()],
                time::macros::datetime!(2026-08-28 09:00),
                None,
                Some(invocation),
                ctx,
            )
        },
    );
}

#[test]
fn failed_calendar_bulk_snooze_restores_action_start_highlight() {
    let today = time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .date();
    let until = today.with_time(time::macros::time!(8:00));
    let tasks = ["first", "second"]
        .into_iter()
        .enumerate()
        .map(|(index, id)| {
            let mut task = task_with_rank(id, id, TaskState::Snoozed, index as i64 + 1);
            task.snoozed_until = Some(until);
            task
        })
        .collect::<Vec<_>>();
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: tasks.clone(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let mut workspace = CalendarWorkspace::new(app.context.clone(), true);
    for key in [
        KeyEvent::from(Key::Char('D')),
        KeyEvent {
            code: Key::Down,
            modifiers: KeyModifiers::SHIFT,
        },
    ] {
        workspace.event(&TuiEvent::Key(key), &mut EventCtx::default());
    }
    assert_eq!(workspace.highlighted_task_id().as_deref(), Some("second"));
    let invocation = app
        .context
        .begin_transient_selection(
            TransientSelectionSource::Calendar,
            vec!["first".into(), "second".into()],
            workspace.highlighted_task_id(),
        )
        .unwrap();
    workspace.set_active_selection_invocation_for_test(Some(invocation));

    app.snooze_tasks(
        vec!["first".into(), "second".into()],
        (today + time::Duration::days(1)).with_time(time::macros::time!(8:00)),
        None,
        Some(invocation),
        &mut EventCtx::default(),
    );
    workspace.sync_store_version();
    assert_eq!(workspace.highlighted_task_id(), None);

    store.borrow_mut().dispatch(AppEvent::TasksRestored(tasks));
    app.context
        .coordinator
        .borrow_mut()
        .record_selection_outcome_for_test(invocation, true);
    workspace.sync_store_version();

    assert_eq!(workspace.highlighted_task_id().as_deref(), Some("second"));
}

#[test]
fn calendar_quick_menu_snapshots_transient_day_selection_and_keeps_it_after_focus_moves() {
    let today = time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .date();
    let until = today.with_time(time::macros::time!(8:00));
    let tasks = ["first", "second", "third"]
        .into_iter()
        .enumerate()
        .map(|(index, id)| {
            let mut task = task_with_rank(id, id, TaskState::Snoozed, index as i64 + 1);
            task.snoozed_until = Some(until);
            task
        })
        .collect();
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks,
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = CalendarWorkspace::new(context, true);
    let area = Rect::new(0, 0, 120, 40);
    let mut layout = LayoutCtx::new();
    workspace.layout(area, &mut layout);
    let calendar = layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "calendar")
        .expect("calendar should be focusable")
        .clone();
    let route = EventRoute::new(calendar.path.clone());
    workspace.dispatch_focus(&calendar, true, &mut FocusCtx::default());
    workspace.dispatch_event(
        &route,
        &TuiEvent::Key(Key::Char('D').into()),
        &mut EventCtx::default(),
    );
    workspace.dispatch_event(
        &route,
        &TuiEvent::Key(KeyEvent {
            code: Key::Down,
            modifiers: KeyModifiers::SHIFT,
        }),
        &mut EventCtx::default(),
    );
    let mut quick_menu = EventCtx::default();
    assert!(
        workspace
            .dispatch_event(
                &route,
                &TuiEvent::Key(Key::Char('.').into()),
                &mut quick_menu,
            )
            .handled()
    );
    assert!(matches!(
        quick_menu.messages(),
        [AppMsg::SelectionAction { action, .. }]
            if matches!(action.as_ref(), AppMsg::OpenCalendarTasksQuickMenu {
                task_ids,
                time,
                selection_active: true,
            } if task_ids == &vec!["first".to_string(), "second".to_string()]
                && *time == Some(until)
            )
    ));

    workspace.dispatch_focus(&calendar, false, &mut FocusCtx::default());
    workspace.dispatch_focus(&calendar, true, &mut FocusCtx::default());
    for key in [
        KeyEvent {
            code: Key::Char('m'),
            modifiers: KeyModifiers::CONTROL,
        },
        KeyEvent::from(Key::End),
        KeyEvent::from(Key::Enter),
    ] {
        workspace.dispatch_event(&route, &TuiEvent::Key(key), &mut EventCtx::default());
    }

    let state = store.borrow();
    let ranks = state
        .state()
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task.rank))
        .collect::<Vec<_>>();
    assert_eq!(ranks, [("first", 2), ("second", 3), ("third", 1)]);
}

#[test]
fn calendar_group_selection_clears_after_order_independent_completion() {
    let today = time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .date();
    let until = today.with_time(time::macros::time!(8:00));
    let tasks = ["first", "second"]
        .into_iter()
        .enumerate()
        .map(|(index, id)| {
            let mut task = task_with_rank(id, id, TaskState::Snoozed, index as i64 + 1);
            task.snoozed_until = Some(until);
            task
        })
        .collect();
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks,
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = CalendarWorkspace::new(context.clone(), true);
    for key in [
        KeyEvent::from(Key::Char('D')),
        KeyEvent {
            code: Key::Down,
            modifiers: KeyModifiers::SHIFT,
        },
    ] {
        workspace.event(&TuiEvent::Key(key), &mut EventCtx::default());
    }
    workspace.event(
        &TuiEvent::Key(Key::Char('.').into()),
        &mut EventCtx::default(),
    );

    workspace.sync_store_version();
    assert_eq!(workspace.transient_selected_task_ids(), ["first", "second"]);

    for task_id in ["first", "second"] {
        context.store.borrow_mut().dispatch(AppEvent::PatchTask {
            task_id: task_id.into(),
            patch: TaskPatch::Snooze {
                until: today.with_time(time::macros::time!(9:00)),
                remember_custom: None,
            },
        });
    }
    context.accept_transient_mutation(workspace.active_selection_invocation_for_test());
    workspace.sync_store_version();

    assert!(workspace.transient_selected_task_ids().is_empty());
    assert_eq!(workspace.highlighted_task_id().as_deref(), Some("second"));
}

#[test]
fn calendar_quick_menu_uses_highlighted_time_after_navigation_clears_selection() {
    let today = time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .date();
    let eight = today.with_time(time::macros::time!(8:00));
    let nine = today.with_time(time::macros::time!(9:00));
    let tasks = [
        ("first", eight),
        ("second", eight),
        ("third", eight),
        ("later", nine),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (id, until))| {
        let mut task = task_with_rank(id, id, TaskState::Snoozed, index as i64 + 1);
        task.snoozed_until = Some(until);
        task
    })
    .collect();
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks,
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = CalendarWorkspace::new(context, true);
    for key in [
        KeyEvent::from(Key::Char('D')),
        KeyEvent {
            code: Key::Down,
            modifiers: KeyModifiers::CONTROL,
        },
        KeyEvent {
            code: Key::Down,
            modifiers: KeyModifiers::CONTROL,
        },
        KeyEvent {
            code: Key::Char(' '),
            modifiers: KeyModifiers::CONTROL,
        },
        KeyEvent::from(Key::Down),
    ] {
        workspace.event(&TuiEvent::Key(key), &mut EventCtx::default());
    }
    let mut ctx = EventCtx::default();

    workspace.event(&TuiEvent::Key(Key::Char('.').into()), &mut ctx);

    assert!(matches!(
        ctx.messages(),
        [AppMsg::OpenCalendarTasksQuickMenu {
            task_ids,
            time,
            selection_active: false,
        }] if task_ids == &vec!["later".to_string()] && *time == Some(nine)
    ));
}

#[test]
fn calendar_transient_single_selection_routes_progress_as_a_group_action() {
    let today = time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .date();
    let mut task = task_with("snoozed", "Snoozed", TaskState::Snoozed);
    task.snoozed_until = Some(today.with_time(time::macros::time!(8:00)));
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![task],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = CalendarWorkspace::new(context, true);
    for key in [
        KeyEvent::from(Key::Char('D')),
        KeyEvent {
            code: Key::Char(' '),
            modifiers: KeyModifiers::CONTROL,
        },
    ] {
        workspace.event(&TuiEvent::Key(key), &mut EventCtx::default());
    }
    let mut ctx = EventCtx::default();

    workspace.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('t'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut ctx,
    );

    assert!(matches!(
        ctx.messages(),
        [AppMsg::SelectionAction { action, .. }]
            if matches!(action.as_ref(), AppMsg::CompleteCalendarTasks {
                task_ids,
                state: TaskState::Todo,
            } if task_ids == &vec!["snoozed".to_string()])
    ));
}

#[test]
fn external_refresh_repopulates_selected_task_detail_once_draft_is_safe() {
    let task = test_task();
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![task.clone()],
        people: vec![],
        workspaces: vec![],
        tags: vec![],
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.detail_draft_protected = true;
    let mut refreshed = task;
    refreshed.title = "Externally changed".into();
    store.borrow_mut().dispatch(AppEvent::WorkspaceRefreshed {
        snapshot: WorkspaceSnapshot {
            tasks: vec![refreshed],
            people: vec![],
            workspaces: vec![],
            tags: vec![],
        },
        revision: 1,
        entity_revisions: std::collections::HashMap::new(),
    });
    let area = Rect::new(0, 0, 100, 30);

    workspace.layout(area, &mut LayoutCtx::new());
    assert!(rendered_text(workspace.detail(), area).contains("Original"));

    workspace.detail_draft_protected = false;
    workspace.layout(area, &mut LayoutCtx::new());
    let detail = rendered_text(workspace.detail(), area);
    assert!(detail.contains("Externally changed"));
    assert_eq!(
        workspace.table().highlighted_id().as_deref(),
        Some("task-1")
    );
}

#[test]
fn external_refresh_does_not_rebuild_focused_detail_after_checklist_patch() {
    let mut task = test_task();
    let optimistic_checklist = vec![ChecklistItem {
        id: "item-1".into(),
        parent_id: None,
        text: "Keep focus".into(),
        checked: true,
    }];
    task.checklist = vec![ChecklistItem {
        checked: false,
        ..optimistic_checklist[0].clone()
    }];
    let (runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![task],
        people: vec![],
        workspaces: vec![],
        tags: vec![],
    });
    let mut workspace = TaskWorkspace::new(context);
    let area = Rect::new(0, 0, 100, 30);
    let mut layout = LayoutCtx::new();
    workspace.layout(area, &mut layout);
    let detail_focus = layout
        .focus_targets()
        .iter()
        .find(|target| target.path.keys().first() == Some(&ChildKey::second()))
        .expect("detail should be focusable")
        .clone();
    workspace.dispatch_focus(&detail_focus, true, &mut FocusCtx::default());
    let original_patches = Rc::clone(&workspace.detail().patches);

    workspace
        .detail_mut()
        .patches
        .borrow_mut()
        .push(TaskPatch::Checklist(optimistic_checklist.clone()));
    assert!(workspace.sync_detail_changes(None).changed);
    assert!(workspace.detail_draft_protected);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let runtime_thread = std::thread::spawn(move || {
        runtime.block_on(async {
            let _ = shutdown_rx.await;
        });
    });
    let drained = workspace
        .context
        .coordinator
        .borrow_mut()
        .drain(Duration::from_secs(1));
    shutdown_tx
        .send(())
        .expect("runtime should still be running");
    runtime_thread.join().expect("runtime thread should join");
    assert!(drained);
    assert!(!workspace.context.coordinator.borrow().has_pending());
    store.borrow_mut().dispatch(AppEvent::WorkspaceRefreshed {
        snapshot: WorkspaceSnapshot {
            tasks: vec![Task {
                title: "Externally changed".into(),
                checklist: optimistic_checklist,
                ..test_task()
            }],
            people: vec![],
            workspaces: vec![],
            tags: vec![],
        },
        revision: 1,
        entity_revisions: std::collections::HashMap::new(),
    });

    workspace.layout(area, &mut LayoutCtx::new());

    assert!(Rc::ptr_eq(&original_patches, &workspace.detail().patches));
}

#[test]
fn identical_workspace_refresh_does_not_rebuild_selected_task_detail() {
    let task = test_task();
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![task.clone()],
        people: vec![],
        workspaces: vec![],
        tags: vec![],
    });
    let mut task_workspace = TaskWorkspace::new(context);
    let original_patches = Rc::clone(&task_workspace.detail().patches);
    store.borrow_mut().dispatch(AppEvent::WorkspaceRefreshed {
        snapshot: WorkspaceSnapshot {
            tasks: vec![task],
            people: vec![],
            workspaces: vec![],
            tags: vec![],
        },
        revision: 1,
        entity_revisions: std::collections::HashMap::new(),
    });

    task_workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());

    assert!(Rc::ptr_eq(
        &original_patches,
        &task_workspace.detail().patches
    ));
}

#[test]
fn new_people_and_workspaces_refresh_focused_task_detail_options() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: vec![],
        workspaces: vec![],
        tags: vec![],
    });
    let mut task_workspace = TaskWorkspace::new(context);
    let original_patches = Rc::clone(&task_workspace.detail().patches);
    let person = Person::new("person-1".into(), "Ada".into(), String::new());
    let workspace = Workspace::new(
        "workspace-1".into(),
        "CORE".into(),
        "Core".into(),
        String::new(),
    );
    store
        .borrow_mut()
        .dispatch(AppEvent::PersonCreated(person.clone()));
    store
        .borrow_mut()
        .dispatch(AppEvent::WorkspaceCreated(workspace.clone()));

    task_workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());

    assert!(!Rc::ptr_eq(
        &original_patches,
        &task_workspace.detail().patches
    ));
    assert_eq!(task_workspace.detail().people_snapshot, vec![person]);
    assert_eq!(task_workspace.detail().workspaces_snapshot, vec![workspace]);
}

fn rendered_area_has_focus_style(node: &impl TuiNode<AppMsg>, canvas: Rect, area: Rect) -> bool {
    let mut terminal = Terminal::new(TestBackend::new(canvas.width, canvas.height))
        .expect("terminal should build");
    terminal
        .draw(|frame| node.render(frame, canvas, &mut RenderCtx::new()))
        .expect("node should render");
    let buffer = terminal.backend().buffer();
    let theme = tuicore::theme();
    (area.y..area.bottom()).any(|y| {
        (area.x..area.right()).any(|x| {
            let cell = buffer.cell((x, y)).expect("focused area cell should exist");
            cell.fg == theme.highlight_fg() && cell.bg == theme.highlight_bg()
        })
    })
}

#[test]
fn task_tab_shows_new_actions_and_filters_below_tabs() {
    assert_eq!(
        TaskView::OPTIONS,
        [
            TaskView::All,
            TaskView::Backlog,
            TaskView::Active,
            TaskView::Snoozed,
            TaskView::Archived,
        ]
    );
    assert_eq!(
        TaskView::OPTIONS.map(TaskView::label),
        ["All", "Backlog", "Active", "Snoozed", "Archived"]
    );
    assert_eq!(
        TaskView::OPTIONS.map(TaskView::icon),
        ["", "", "", "󰒲", ""]
    );
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: vec![Workspace::new(
            "workspace-1".into(),
            "APP".into(),
            "Application".into(),
            String::new(),
        )],
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let area = Rect::new(0, 0, 80, 40);

    app.layout(area, &mut LayoutCtx::new());
    let text = rendered_text(&app, area);

    for expected in [
        " Active",
        &keys::TASK_VIEW_MENU.label(),
        &keys::TASK_LABEL_FILTER.label(),
        "󰲋 Space",
        " Tags",
        &format!("Task |{}|", keys::TASK_QUICK_CREATE.label()),
        &format!("Note |{}|", keys::NOTE_QUICK_CREATE.label()),
    ] {
        assert!(
            text.contains(expected),
            "missing header text: {expected}; rendered: {text:?}"
        );
    }
    let workspace = text
        .find("󰲋 Space")
        .expect("workspace filter should render");
    let labels = text.find(" Tags").expect("tag filter should render");
    let tabs = text
        .find("Tasks · Calendar · Notes")
        .expect("tabs should render");
    let new_task = text
        .find(&format!("Task |{}|", keys::TASK_QUICK_CREATE.label()))
        .expect("new task button should render");
    let new_note = text
        .find(&format!("Note |{}|", keys::NOTE_QUICK_CREATE.label()))
        .expect("new note button should render");
    assert!(tabs < new_task && new_task < new_note && new_note < workspace && workspace < labels);
    assert!(!text.contains("View:"));
    assert!(!text.contains("Resolve"));
    assert!(!text.contains("Permanently"));
}

#[test]
fn task_label_filter_uses_and_logic_after_state_filtering() {
    let mut active_both = task_with("active-both", "Active both", TaskState::Todo);
    active_both.tag_ids = vec!["api".into(), "urgent".into()];
    let mut active_one = task_with("active-one", "Active one", TaskState::InProgress);
    active_one.tag_ids = vec!["api".into()];
    let mut backlog_both = task_with("backlog-both", "Backlog both", TaskState::Backlog);
    backlog_both.tag_ids = vec!["api".into(), "urgent".into()];
    let tasks = [active_both, active_one, backlog_both];
    let labels = ["api".to_string(), "urgent".to_string()];

    let active = task_rows_for_view(&tasks, TaskView::Active, None, &labels);
    let backlog = task_rows_for_view(&tasks, TaskView::Backlog, None, &labels);

    assert_eq!(
        active
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["active-both"]
    );
    assert_eq!(
        backlog
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["backlog-both"]
    );
}

#[test]
fn committed_label_filter_refreshes_workspace_rows() {
    let mut both = task_with("both", "Both labels", TaskState::Todo);
    both.tag_ids = vec!["api".into(), "urgent".into()];
    let mut one = task_with("one", "One label", TaskState::InProgress);
    one.tag_ids = vec!["api".into()];
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![both, one],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: vec![
            Tag::new("api".into(), "API".into()),
            Tag::new("urgent".into(), "Urgent".into()),
        ],
    });
    let mut workspace = TaskWorkspace::new(context);
    *workspace.active_label_filter.borrow_mut() = vec!["api".into(), "urgent".into()];

    assert!(workspace.sync_label_filter_change());

    assert_eq!(
        workspace
            .table()
            .rows()
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["both"]
    );
    assert_eq!(workspace.detail().task_id.as_deref(), Some("both"));
}

#[test]
fn committed_workspace_filter_selects_one_workspace_or_none() {
    let mut first = task_with("first", "First workspace", TaskState::Todo);
    first.workspace_id = Some("workspace-1".into());
    let mut second = task_with("second", "Second workspace", TaskState::Todo);
    second.workspace_id = Some("workspace-2".into());
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![first, second],
        people: Vec::new(),
        workspaces: vec![
            Workspace::new(
                "workspace-1".into(),
                "ONE".into(),
                "One".into(),
                String::new(),
            ),
            Workspace::new(
                "workspace-2".into(),
                "TWO".into(),
                "Two".into(),
                String::new(),
            ),
        ],
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    *workspace.active_workspace_filter.borrow_mut() = Some("workspace-2".into());

    assert!(workspace.sync_workspace_filter_change());
    assert_eq!(
        workspace
            .table()
            .rows()
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["second"]
    );

    *workspace.active_workspace_filter.borrow_mut() = None;
    assert!(workspace.sync_workspace_filter_change());
    assert_eq!(workspace.table().rows().len(), 2);
}

#[test]
fn task_table_state_column_is_icon_only() {
    let mut table = task_table(
        vec![
            task_with("todo", "Todo work", TaskState::Todo),
            task_with("backlog", "Backlog work", TaskState::Backlog),
            task_with("active", "Active work", TaskState::InProgress),
            task_with("done", "Done work", TaskState::Done),
            task_with("snoozed", "Snoozed work", TaskState::Snoozed),
            task_with("rejected", "Rejected work", TaskState::Rejected),
        ],
        None,
    );
    let area = Rect::new(0, 0, 100, 10);
    <TaskTable as TuiNode<AppMsg>>::layout(&mut table, area, &mut LayoutCtx::new());

    let text = rendered_text(&table, area);

    assert!(!text.contains("State"));
    for label in [
        "BACKLOG",
        "TODO",
        "IN-PROGRESS",
        "DONE",
        "SNOOZED",
        "REJECTED",
    ] {
        assert!(
            !text.contains(label),
            "state label leaked into table: {label}"
        );
    }
    for icon in ["", "", "", "", "󰒲", ""] {
        assert!(text.contains(icon), "missing state icon: {icon}");
    }
}

#[test]
fn task_table_shows_current_workspace_key_task_number_and_title() {
    let workspace = Workspace::new(
        "workspace".into(),
        "core".into(),
        "Core".into(),
        String::new(),
    );
    let mut task = task_with("OLD-42", "Ship it", TaskState::Todo);
    task.workspace_id = Some(workspace.id.clone());
    let mut table =
        task_table_with_copy_context(vec![task], None, TaskCopyContext::new(&[workspace]));
    let area = Rect::new(0, 0, 80, 5);
    <TaskTable as TuiNode<AppMsg>>::layout(&mut table, area, &mut LayoutCtx::new());

    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            <TaskTable as TuiNode<AppMsg>>::render(&table, frame, area, &mut RenderCtx::new())
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let cells = buffer.content();
    let id_start = cells
        .windows(7)
        .position(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == "CORE-42")
        .expect("task display ID should render");

    assert_eq!(cells[id_start].fg, tuicore::theme().subtle_fg());
    assert!(cells[id_start].modifier.contains(Modifier::BOLD));
    assert!(!cells[id_start + 10].modifier.contains(Modifier::BOLD));
    assert!(rendered_text(&table, area).contains("CORE-42 Ship it"));
}

#[test]
fn task_table_wraps_long_titles_without_a_horizontal_scrollbar() {
    let table = task_table(
        vec![task_with(
            "long",
            "Bake Thompson wedding cake tailored to every requested detail",
            TaskState::InProgress,
        )],
        None,
    );
    let area = Rect::new(0, 0, 30, 5);
    let mut terminal =
        Terminal::new(TestBackend::new(area.width, area.height)).expect("terminal should build");

    terminal
        .draw(|frame| {
            <TaskTable as TuiNode<AppMsg>>::render(&table, frame, area, &mut RenderCtx::new())
        })
        .expect("table should render");

    let buffer = terminal.backend().buffer();
    let scrollbar = (0..area.width)
        .map(|x| buffer.cell((x, area.height - 1)).unwrap().symbol())
        .collect::<String>();
    assert!(
        !scrollbar.contains('━') && !scrollbar.contains('─'),
        "unexpected horizontal scrollbar: {scrollbar:?}"
    );
}

#[test]
fn narrow_task_workspace_renders_task_and_detail() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);

    let area = Rect::new(0, 0, 80, 40);
    workspace.layout(area, &mut LayoutCtx::new());

    let text = rendered_text(&workspace, area);
    assert!(text.contains("Original"));
    assert!(text.contains("Title"));
}

#[test]
fn wide_task_workspace_aligns_detail_with_toolbar_top() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);

    workspace.layout(Rect::new(0, 0, 120, 40), &mut LayoutCtx::new());

    let (table_area, detail_area) = workspace.layout.child_areas();
    assert_eq!(table_area.width, 54);
    assert_eq!(detail_area.width, 66);
    assert_eq!(detail_area.y, 0);
}

#[test]
fn task_table_priority_is_icon_only_in_second_column() {
    let mut low = task_with("low", "Alpha work", TaskState::Todo);
    low.priority = TaskPriority::Low;
    let mut medium = task_with("medium", "Beta work", TaskState::Todo);
    medium.priority = TaskPriority::Medium;
    let mut high = task_with("high", "Gamma work", TaskState::Todo);
    high.priority = TaskPriority::High;
    let mut table = task_table(vec![low, medium, high], None);
    let area = Rect::new(0, 0, 100, 8);
    <TaskTable as TuiNode<AppMsg>>::layout(&mut table, area, &mut LayoutCtx::new());

    let text = rendered_text(&table, area);

    assert!(!text.contains("Priority"));
    for label in ["Low", "Medium", "High"] {
        assert!(
            !text.contains(label),
            "priority label leaked into table: {label}"
        );
    }
    for icon in ["󰅀", "󰇼", "󰅃"] {
        assert!(text.contains(icon), "missing priority icon: {icon}");
    }
}

#[test]
fn task_table_uses_persisted_rank_order() {
    let mut older_medium = task_with("older-medium", "Older medium", TaskState::Todo);
    older_medium.priority = TaskPriority::Medium;
    let mut high = task_with("high", "High priority", TaskState::Todo);
    high.priority = TaskPriority::High;
    let mut newer_medium = task_with("newer-medium", "Newer medium", TaskState::Todo);
    newer_medium.priority = TaskPriority::Medium;
    let mut low = task_with("low", "Low priority", TaskState::Todo);
    low.priority = TaskPriority::Low;
    high.rank = 1;
    newer_medium.rank = 2;
    older_medium.rank = 3;
    low.rank = 4;
    let rows = task_rows_for_view(
        &[older_medium, high, newer_medium, low],
        TaskView::Active,
        None,
        &[],
    );
    let mut table = task_table(rows, None);
    let area = Rect::new(0, 0, 100, 10);
    <TaskTable as TuiNode<AppMsg>>::layout(&mut table, area, &mut LayoutCtx::new());

    let text = rendered_text(&table, area);
    let high_index = text.find("High priority").expect("high task should render");
    let newer_medium_index = text
        .find("Newer medium")
        .expect("newer medium task should render");
    let older_medium_index = text
        .find("Older medium")
        .expect("older medium task should render");
    let low_index = text.find("Low priority").expect("low task should render");

    assert!(high_index < newer_medium_index);
    assert!(newer_medium_index < older_medium_index);
    assert!(older_medium_index < low_index);
}

#[test]
fn yanking_highlighted_task_copies_tuido_reference() {
    let workspace = Workspace::new(
        "workspace-alpha".into(),
        "ALPHA".into(),
        "Alpha".into(),
        String::new(),
    );
    let first = task_with("task-first", "Wrong highlighted task", TaskState::Todo);
    let mut highlighted = task_with("OLD-1234", "Ship agent export", TaskState::Snoozed);
    highlighted.workspace_id = Some(workspace.id.clone());
    let copy_context = TaskCopyContext::new(&[workspace]);
    let mut table = task_table_with_copy_context(vec![first, highlighted], None, copy_context);
    table.data_view_mut().highlight_id(&"OLD-1234".to_string());

    let effects = yank_task_table(&mut table);
    let payload = effects
        .clipboard
        .expect("yank should request clipboard copy");
    assert_eq!(payload, "Tuido ALPHA-1234 \"Ship agent export\"");
    assert!(effects.outcome.handled());
    assert_eq!(effects.notifications.len(), 1);
    assert_eq!(
        effects.notifications,
        vec![tuicore::Notification::info(
            "Copied to clipboard",
            format!("\"{payload}\"")
        )]
    );
}

#[test]
fn agent_yank_copies_unworkspaceed_numeric_task_command() {
    let task_id = "1234";
    let task_title = "Add yank agent hotkey";
    let task = task_with(task_id, task_title, TaskState::Todo);
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![task],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);

    let mut ctx = EventCtx::default();
    let outcome = workspace.event(
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_AGENT_YANK.hotkey())),
        &mut ctx,
    );
    let effects = tuicore::DispatchEffects::from_event_ctx(outcome, ctx);
    let expected = format!("Tuido execute 1234 \"{task_title}\"");

    assert_eq!(effects.clipboard.as_deref(), Some(expected.as_str()));
    assert_eq!(
        effects.notifications,
        vec![tuicore::Notification::info(
            "Copied to clipboard",
            format!("\"{expected}\"")
        )]
    );
}

#[test]
fn agent_yank_copies_workspace_key_task_command() {
    let workspace = Workspace::new(
        "workspace-1".into(),
        "proj".into(),
        "Workspace".into(),
        String::new(),
    );
    let mut task = task_with("OLD-1234", "Workspace task", TaskState::Todo);
    task.workspace_id = Some(workspace.id.clone());
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![task],
        people: Vec::new(),
        workspaces: vec![workspace],
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);

    let mut ctx = EventCtx::default();
    let outcome = workspace.event(
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_AGENT_YANK.hotkey())),
        &mut ctx,
    );
    let effects = tuicore::DispatchEffects::from_event_ctx(outcome, ctx);

    assert_eq!(
        effects.clipboard.as_deref(),
        Some("Tuido execute PROJ-1234 \"Workspace task\"")
    );
}

#[test]
fn task_yanks_use_the_table_highlight_over_stale_detail_selection() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with("1234", "Stale selection", TaskState::Todo),
            task_with("5678", "Current highlight", TaskState::Todo),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.table_focused = true;
    workspace.table_mut().highlight_id(&"5678".to_string());

    for (hotkey, expected) in [
        (
            keys::TASK_AGENT_YANK.hotkey(),
            "Tuido execute 5678 \"Current highlight\"",
        ),
        (
            keys::TASK_AGENT_YANK_CLARIFY.hotkey(),
            "Tuido clarify 5678 \"Current highlight\"",
        ),
    ] {
        let mut ctx = EventCtx::default();

        workspace.event(&TuiEvent::Hotkey(HotkeyEvent::Commit(hotkey)), &mut ctx);

        assert_eq!(ctx.clipboard_request(), Some(expected));
    }

    let mut ctx = EventCtx::default();
    workspace.event(&TuiEvent::Yank, &mut ctx);

    assert_eq!(
        ctx.clipboard_request(),
        Some("Tuido 5678 \"Current highlight\"")
    );
}

#[test]
fn task_list_group_yanks_copy_all_selected_tasks_then_clear_selection() {
    let first = task_with_rank("1", "First \"quoted\"", TaskState::Todo, 1);
    let second = task_with_rank("2", "Second \\ path", TaskState::Todo, 2);
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![first, second],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.table_focused = true;

    for (event, expected) in [
        (
            TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_AGENT_YANK.hotkey())),
            r#"Tuido execute 1 "First \"quoted\""; 2 "Second \\ path""#,
        ),
        (
            TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_AGENT_YANK_CLARIFY.hotkey())),
            r#"Tuido clarify 1 "First \"quoted\""; 2 "Second \\ path""#,
        ),
        (
            TuiEvent::Yank,
            r#"Tuido 1 "First \"quoted\""; 2 "Second \\ path""#,
        ),
    ] {
        workspace.table_mut().highlight_id(&"1".to_string());
        workspace.task_list_mut().event(
            &TuiEvent::Key(KeyEvent {
                code: Key::Down,
                modifiers: KeyModifiers::SHIFT,
            }),
            &mut EventCtx::default(),
        );
        let mut ctx = EventCtx::default();

        workspace.event(&event, &mut ctx);

        assert_eq!(ctx.clipboard_request(), Some(expected));
        assert!(workspace.task_list().transient_selected_ids().is_empty());
    }
}

#[test]
fn detail_focused_yanks_prefer_transient_task_group() {
    let first = task_with_rank("1", "First \"quoted\"", TaskState::Todo, 1);
    let second = task_with_rank("2", "Second \\ path", TaskState::Todo, 2);
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![first, second],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let mut layout = LayoutCtx::new();
    workspace.layout(Rect::new(0, 0, 120, 24), &mut layout);
    let detail_path = layout
        .focus_targets()
        .iter()
        .find(|target| target.path.keys().first() == Some(&ChildKey::second()))
        .expect("detail control should be focusable")
        .path
        .clone();

    for (event, expected) in [
        (
            TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_AGENT_YANK.hotkey())),
            r#"Tuido execute 1 "First \"quoted\""; 2 "Second \\ path""#,
        ),
        (
            TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_AGENT_YANK_CLARIFY.hotkey())),
            r#"Tuido clarify 1 "First \"quoted\""; 2 "Second \\ path""#,
        ),
        (
            TuiEvent::Yank,
            r#"Tuido 1 "First \"quoted\""; 2 "Second \\ path""#,
        ),
    ] {
        workspace.table_mut().highlight_id(&"1".to_string());
        workspace.task_list_mut().event(
            &TuiEvent::Key(KeyEvent {
                code: Key::Down,
                modifiers: KeyModifiers::SHIFT,
            }),
            &mut EventCtx::default(),
        );
        let mut ctx = EventCtx::default();

        workspace.dispatch_event(&EventRoute::new(detail_path.clone()), &event, &mut ctx);

        assert_eq!(ctx.clipboard_request(), Some(expected));
        assert!(workspace.task_list().transient_selected_ids().is_empty());
    }
}

#[test]
fn clarify_yank_copies_selected_task_command_from_detail_view() {
    let workspace = Workspace::new(
        "workspace-1".into(),
        "proj".into(),
        "Workspace".into(),
        String::new(),
    );
    let mut task = task_with("OLD-1234", "Clarify this task", TaskState::Todo);
    task.workspace_id = Some(workspace.id.clone());
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![task],
        people: Vec::new(),
        workspaces: vec![workspace],
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let mut layout = LayoutCtx::new();
    workspace.layout(Rect::new(0, 0, 120, 24), &mut layout);
    let detail_path = layout
        .focus_targets()
        .iter()
        .find(|target| target.path.keys().first() == Some(&ChildKey::second()))
        .expect("detail control should be focusable")
        .path
        .clone();

    let mut ctx = EventCtx::default();
    let outcome = workspace.dispatch_event(
        &EventRoute::new(detail_path.clone()),
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_AGENT_YANK_CLARIFY.hotkey())),
        &mut ctx,
    );
    let effects = tuicore::DispatchEffects::from_event_ctx(outcome, ctx);

    assert_eq!(
        effects.clipboard.as_deref(),
        Some("Tuido clarify PROJ-1234 \"Clarify this task\"")
    );

    let mut yank_ctx = EventCtx::default();
    let outcome = workspace.dispatch_event(
        &EventRoute::new(detail_path),
        &TuiEvent::Yank,
        &mut yank_ctx,
    );
    let effects = tuicore::DispatchEffects::from_event_ctx(outcome, yank_ctx);

    assert_eq!(
        effects.clipboard.as_deref(),
        Some("Tuido PROJ-1234 \"Clarify this task\"")
    );
}

#[test]
fn app_startup_selects_and_focuses_first_ranked_task() {
    let mut older_low = task_with("older-low", "Older low", TaskState::InProgress);
    older_low.priority = TaskPriority::Low;
    older_low.rank = 2;
    let mut newer_high = task_with("newer-high", "Newer high", TaskState::InProgress);
    newer_high.priority = TaskPriority::High;
    newer_high.rank = 1;
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![older_low, newer_high],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let workspace = TaskWorkspace::new(context.clone());

    assert_eq!(
        workspace.table().highlighted_id().as_deref(),
        Some("newer-high")
    );
    assert_eq!(
        workspace.table().selected_id().as_deref(),
        Some("newer-high")
    );

    let mut app = App::new(store, Rc::clone(&context.coordinator));
    let mut layout = LayoutCtx::new();
    app.layout(Rect::new(0, 0, 120, 50), &mut layout);
    let expected = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "data-view"
                && !target
                    .path
                    .keys()
                    .iter()
                    .any(|part| matches!(part.as_str(), "checklist" | "links"))
        })
        .expect("task table should be focusable")
        .clone();
    let mut focus = FocusManager::new();

    let transition = focus
        .apply_request(&initial_task_table_focus_request(), layout.focus_targets())
        .expect("initial task table focus should apply");

    assert_eq!(transition.current, Some(expected));
}

#[test]
fn task_detail_hotkeys_are_registered_while_task_table_is_focused() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(store, Rc::clone(&context.coordinator));
    let mut layout = LayoutCtx::new();
    app.layout(Rect::new(0, 0, 80, 24), &mut layout);

    let task_table = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "data-view"
                && !target
                    .path
                    .keys()
                    .iter()
                    .any(|part| part.as_str() == "links")
        })
        .expect("task table should be focusable");
    assert!(!task_table.suppress_global_hotkeys);
    assert!(!task_table.focused_events_before_global_hotkeys);
    assert!(
        task_table
            .hotkey_sequences
            .contains(&keys::TASK_AGENT_YANK.hotkey())
    );
    assert!(
        task_table
            .hotkey_sequences
            .contains(&keys::TASK_AGENT_YANK_CLARIFY.hotkey())
    );

    for hotkey in [
        keys::TASK_TITLE_FIELD.hotkey(),
        keys::TASK_TAGS_FIELD.hotkey(),
        keys::TASK_CHECKLIST_FIELD.hotkey(),
        keys::TASK_URL_LINKS_FIELD.hotkey(),
        keys::TASK_ISSUE_LINKS_FIELD.hotkey(),
    ] {
        assert_eq!(
            layout
                .focus_targets()
                .iter()
                .filter(|target| target.hotkey_sequences.contains(&hotkey))
                .count(),
            1,
            "{hotkey} should be registered exactly once"
        );
    }
}

#[test]
fn reselecting_current_task_does_not_rebuild_detail_controls() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let original_patches = Rc::clone(&workspace.detail().patches);

    workspace.select_task("task-1", &mut EventCtx::default());

    assert!(Rc::ptr_eq(&original_patches, &workspace.detail().patches));
    assert_eq!(
        store.borrow().state().selected_task_id.as_deref(),
        Some("task-1")
    );
}

#[test]
fn enter_on_task_link_opens_it_in_the_browser() {
    let mut task = test_task();
    task.links = vec!["www.example.com/item".into()];
    let opened = Rc::new(RefCell::new(Vec::new()));
    let opened_by_handler = Rc::clone(&opened);
    let mut input = TaskLinksInput::with_opener(
        &task,
        Rc::new(RefCell::new(Vec::new())),
        move |url, mode| {
            opened_by_handler.borrow_mut().push((url.to_string(), mode));
            Ok(())
        },
    );
    let area = Rect::new(0, 0, 40, 5);
    let mut layout = LayoutCtx::new();
    input.layout(area, &mut layout);
    let target = layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "data-view")
        .expect("links list should be focusable")
        .clone();
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: target.path.clone(),
                id: target.id.clone(),
            },
            layout.focus_targets(),
        )
        .expect("links list focus should apply");
    let mut dispatcher = TreeDispatcher::new();
    dispatcher.dispatch_focus(&mut input, transition, AnimationSettings::default());

    let effects = dispatcher.dispatch_event(
        &mut input,
        &EventRoute::new(target.path.clone()),
        &TuiEvent::Key(Key::Enter.into()),
        AnimationSettings::default(),
    );

    assert!(effects.outcome.handled());
    assert_eq!(
        opened.borrow().as_slice(),
        [(
            "https://www.example.com/item".to_string(),
            LinkOpenMode::Foreground
        )]
    );
}

#[test]
fn ctrl_enter_on_task_link_opens_it_without_requesting_focus() {
    let mut task = test_task();
    task.links = vec!["www.example.com/item".into()];
    let opened = Rc::new(RefCell::new(Vec::new()));
    let opened_by_handler = Rc::clone(&opened);
    let mut input = TaskLinksInput::with_opener(
        &task,
        Rc::new(RefCell::new(Vec::new())),
        move |url, mode| {
            opened_by_handler.borrow_mut().push((url.to_string(), mode));
            Ok(())
        },
    );
    let area = Rect::new(0, 0, 40, 5);
    let mut layout = LayoutCtx::new();
    input.layout(area, &mut layout);
    let target = layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "data-view")
        .expect("links list should be focusable")
        .clone();
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: target.path.clone(),
                id: target.id.clone(),
            },
            layout.focus_targets(),
        )
        .expect("links list focus should apply");
    let mut dispatcher = TreeDispatcher::new();
    dispatcher.dispatch_focus(&mut input, transition, AnimationSettings::default());

    let effects = dispatcher.dispatch_event(
        &mut input,
        &EventRoute::new(target.path.clone()),
        &TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::CONTROL,
        }),
        AnimationSettings::default(),
    );

    assert!(effects.outcome.handled());
    assert_eq!(
        opened.borrow().as_slice(),
        [(
            "https://www.example.com/item".to_string(),
            LinkOpenMode::Background
        )]
    );

    dispatcher.dispatch_event(
        &mut input,
        &EventRoute::new(target.path.clone()),
        &TuiEvent::Key(Key::Char('e').into()),
        AnimationSettings::default(),
    );
    opened.borrow_mut().clear();
    dispatcher.dispatch_event(
        &mut input,
        &EventRoute::new(target.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Enter,
            modifiers: KeyModifiers::CONTROL,
        }),
        AnimationSettings::default(),
    );
    assert!(opened.borrow().is_empty());
}

#[test]
fn space_reveals_highlighted_task_link_title_for_two_seconds() {
    let mut task = test_task();
    task.links = vec![TaskLink {
        url: "https://example.com/docs".into(),
        title: Some("Example documentation".into()),
        last_fetched: Some("123".into()),
    }];
    let mut input =
        TaskLinksInput::with_opener(&task, Rc::new(RefCell::new(Vec::new())), |_, _| Ok(()));
    let area = Rect::new(0, 0, 60, 5);
    let mut layout = LayoutCtx::new();
    input.layout(area, &mut layout);
    let target = layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "data-view")
        .expect("links list should be focusable")
        .clone();
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: target.path.clone(),
                id: target.id.clone(),
            },
            layout.focus_targets(),
        )
        .expect("links list focus should apply");
    let mut dispatcher = TreeDispatcher::new();
    dispatcher.dispatch_focus(&mut input, transition, AnimationSettings::default());

    assert!(rendered_text(&input, area).contains("https://example.com/docs"));
    let effects = dispatcher.dispatch_event(
        &mut input,
        &EventRoute::new(target.path.clone()),
        &TuiEvent::Key(Key::Char(' ').into()),
        AnimationSettings::default(),
    );
    assert!(effects.outcome.handled());
    assert!(effects.tick);
    assert!(rendered_text(&input, area).contains("Example documentation"));

    input.tick(Duration::from_secs(2), AnimationSettings::default());
    assert!(rendered_text(&input, area).contains("https://example.com/docs"));
}

#[test]
fn ctrl_x_removes_highlighted_task_link() {
    let mut task = test_task();
    task.links = vec!["https://example.com/item".into()];
    let patches = Rc::new(RefCell::new(Vec::new()));
    let mut input = TaskLinksInput::with_opener(&task, Rc::clone(&patches), |_, _| Ok(()));
    let area = Rect::new(0, 0, 40, 5);
    let mut layout = LayoutCtx::new();
    input.layout(area, &mut layout);
    let target = layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "data-view")
        .expect("links list should be focusable")
        .clone();
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: target.path.clone(),
                id: target.id.clone(),
            },
            layout.focus_targets(),
        )
        .expect("links list focus should apply");
    let mut dispatcher = TreeDispatcher::new();
    dispatcher.dispatch_focus(&mut input, transition, AnimationSettings::default());

    let effects = dispatcher.dispatch_event(
        &mut input,
        &EventRoute::new(target.path),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('x'),
            modifiers: KeyModifiers::CONTROL,
        }),
        AnimationSettings::default(),
    );

    assert!(effects.outcome.handled());
    assert!(matches!(patches.borrow().as_slice(), [TaskPatch::Links(links)] if links.is_empty()));
}

#[test]
fn task_tab_exposes_separate_task_and_note_actions() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(store, Rc::clone(&context.coordinator));
    let mut layout = LayoutCtx::new();
    let area = Rect::new(0, 0, 80, 40);
    app.layout(area, &mut layout);
    let tabs = layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "tabs")
        .expect("missing app tabs");
    let task_button = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target
                .path
                .keys()
                .iter()
                .any(|part| part.as_str() == "new-task")
        })
        .expect("missing task tab new task button");
    let note_button = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target
                .path
                .keys()
                .iter()
                .any(|part| part.as_str() == "new-note")
        })
        .expect("missing task tab new note button");
    for component in ["workspace", "labels"] {
        let control = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target
                    .path
                    .keys()
                    .iter()
                    .any(|part| part.as_str() == component)
            })
            .unwrap_or_else(|| panic!("missing task tab {component} control"));
        assert_eq!(control.area.y, area.y.saturating_add(1));
        assert!(control.area.x > note_button.area.x);
    }
    let workspace = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target
                .path
                .keys()
                .iter()
                .any(|part| part.as_str() == "workspace")
        })
        .expect("missing task tab workspace control");
    assert!(workspace.area.x > note_button.area.x);
    assert_eq!(tabs.area.y, area.y);
    assert_eq!(task_button.area.y, area.y.saturating_add(1));
    assert_eq!(task_button.area.x, area.x);
    assert_eq!(note_button.area.y, area.y.saturating_add(1));
    assert!(note_button.area.x > task_button.area.x);
    let task_button_path = task_button.path.clone();

    let mut create_ctx = EventCtx::default();
    let create = app.dispatch_event(
        &EventRoute::new(task_button_path.clone()),
        &TuiEvent::Key(Key::Enter.into()),
        &mut create_ctx,
    );
    assert!(create.handled());
    assert!(matches!(
        create_ctx.messages(),
        [AppMsg::OpenCreateTask {
            calendar_date: None
        }]
    ));

    let mut hotkey_ctx = EventCtx::default();
    let hotkey = app.dispatch_event(
        &EventRoute::new(task_button_path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_QUICK_CREATE.hotkey())),
        &mut hotkey_ctx,
    );
    assert!(hotkey.handled());
    assert!(matches!(
        hotkey_ctx.messages(),
        [AppMsg::OpenCreateTask {
            calendar_date: None
        }]
    ));

    let mut note_ctx = EventCtx::default();
    let note = app.dispatch_event(
        &EventRoute::new(note_button.path.clone()),
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::NOTE_QUICK_CREATE.hotkey())),
        &mut note_ctx,
    );
    assert!(note.handled());
    assert!(matches!(note_ctx.messages(), [AppMsg::CreateNote]));
}

#[test]
fn escape_from_app_header_new_button_focuses_tabs() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(store, Rc::clone(&context.coordinator));
    let mut layout = LayoutCtx::new();
    app.layout(Rect::new(0, 0, 80, 40), &mut layout);
    let button_path = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target
                .path
                .keys()
                .iter()
                .any(|part| part.as_str() == "new-task")
        })
        .expect("new task button should be focusable")
        .path
        .clone();

    for key in [
        KeyEvent::from(Key::Esc),
        KeyEvent {
            code: Key::Char('['),
            modifiers: KeyModifiers::CONTROL,
        },
    ] {
        let mut ctx = EventCtx::default();
        let outcome = app.dispatch_event(
            &EventRoute::new(button_path.clone()),
            &TuiEvent::Key(key),
            &mut ctx,
        );

        assert!(outcome.handled());
        assert_eq!(ctx.focus_request(), Some(&app_tabs_focus_request()));
    }
}

#[test]
fn escape_from_task_toolbar_filters_focuses_data_view() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let mut layout = LayoutCtx::new();
    workspace.layout(Rect::new(0, 0, 80, 40), &mut layout);
    let toolbar_path = layout
        .focus_targets()
        .iter()
        .find(|target| {
            let path = target.path.keys();
            path.iter().any(|part| part.as_str() == "view")
                && path.iter().any(|part| part.as_str() == "trigger")
        })
        .expect("task filter button should be focusable")
        .path
        .clone();
    let close_keys = [
        KeyEvent::from(Key::Esc),
        KeyEvent {
            code: Key::Char('['),
            modifiers: KeyModifiers::CONTROL,
        },
    ];

    for key in close_keys {
        let mut ctx = EventCtx::default();
        let outcome = workspace.dispatch_event(
            &EventRoute::new(toolbar_path.clone()),
            &TuiEvent::Key(key),
            &mut ctx,
        );

        assert!(outcome.handled());
        assert_eq!(
            ctx.focus_request(),
            Some(&initial_task_table_focus_request())
        );
    }
}

#[test]
fn canceling_open_task_view_menu_focuses_data_view() {
    for key in [
        KeyEvent::from(Key::Esc),
        KeyEvent {
            code: Key::Char('['),
            modifiers: KeyModifiers::CONTROL,
        },
    ] {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let area = Rect::new(0, 0, 80, 40);
        let mut closed_layout = LayoutCtx::new();
        workspace.layout(area, &mut closed_layout);
        let trigger_path = closed_layout
            .focus_targets()
            .iter()
            .find(|target| {
                let path = target.path.keys();
                path.iter().any(|part| part.as_str() == "view")
                    && path.iter().any(|part| part.as_str() == "trigger")
            })
            .expect("task view trigger should be focusable")
            .path
            .clone();
        workspace.dispatch_event(
            &EventRoute::new(trigger_path),
            &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_VIEW_MENU.hotkey())),
            &mut EventCtx::default(),
        );
        let mut open_layout = LayoutCtx::new();
        workspace.layout(area, &mut open_layout);
        let menu_path = open_layout
            .focus_targets()
            .iter()
            .find(|target| {
                let path = target.path.keys();
                path.iter().any(|part| part.as_str() == "view")
                    && path.iter().any(|part| part.as_str() == "menu")
            })
            .expect("open task view menu should be focusable")
            .path
            .clone();
        let mut ctx = EventCtx::default();

        let outcome =
            workspace.dispatch_event(&EventRoute::new(menu_path), &TuiEvent::Key(key), &mut ctx);

        assert!(outcome.handled());
        assert_eq!(
            ctx.focus_request(),
            Some(&initial_task_table_focus_request())
        );
        assert_eq!(ctx.propagation(), Propagation::Stopped);
    }
}

#[test]
fn escape_from_global_filters_focuses_active_tab_content() {
    let close_keys = [
        KeyEvent::from(Key::Esc),
        KeyEvent {
            code: Key::Char('['),
            modifiers: KeyModifiers::CONTROL,
        },
    ];

    for (active_tab, expected_focus) in [
        (0, initial_task_table_focus_request()),
        (CALENDAR_TAB_INDEX, initial_calendar_focus_request()),
    ] {
        for component in ["workspace", "labels"] {
            for key in close_keys {
                let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
                    tasks: vec![test_task()],
                    people: Vec::new(),
                    workspaces: vec![Workspace::new(
                        "workspace-1".into(),
                        "APP".into(),
                        "Application".into(),
                        String::new(),
                    )],
                    tags: vec![Tag::new("tag-1".into(), "Tag".into())],
                });
                let mut filters = TaskFilterControls::new(
                    context,
                    Rc::new(RefCell::new(None)),
                    Rc::new(RefCell::new(Vec::new())),
                    Rc::new(Cell::new(active_tab)),
                );
                let mut layout = LayoutCtx::new();
                filters.layout(Rect::new(0, 0, 60, 1), &mut layout);
                let target = layout
                    .focus_targets()
                    .iter()
                    .find(|target| {
                        let path = target.path.keys();
                        path.iter().any(|part| part.as_str() == component)
                            && target.id.as_str() == "field"
                    })
                    .expect("global filter should be focusable");
                let expected_hotkey = if component == "workspace" {
                    keys::TASK_WORKSPACE_FILTER.hotkey()
                } else {
                    keys::TASK_LABEL_FILTER.hotkey()
                };
                assert_eq!(target.hotkey_sequences, [expected_hotkey]);
                let mut ctx = EventCtx::default();

                let outcome = filters.dispatch_event(
                    &EventRoute::new(target.path.clone()),
                    &TuiEvent::Key(key),
                    &mut ctx,
                );

                assert!(outcome.handled());
                assert_eq!(ctx.focus_request(), Some(&expected_focus));
                assert_eq!(ctx.propagation(), Propagation::Stopped);
            }
        }
    }
}

#[test]
fn submitting_global_filters_focuses_tabs() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let filters = TaskFilterControls::new(
        context,
        Rc::new(RefCell::new(None)),
        Rc::new(RefCell::new(Vec::new())),
        Rc::new(Cell::new(0)),
    );
    filters.filter_submitted.set(true);
    let mut ctx = EventCtx::default();
    let outcome = filters.finish_event(
        EventOutcome::Handled,
        &TuiEvent::Key(Key::Enter.into()),
        &mut ctx,
    );

    assert!(outcome.handled());
    assert_eq!(
        ctx.focus_request(),
        Some(&initial_task_table_focus_request())
    );
    assert_eq!(ctx.propagation(), Propagation::Stopped);
}

#[test]
fn escape_from_global_filter_focuses_real_calendar_target() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: vec![Workspace::new(
            "workspace-1".into(),
            "APP".into(),
            "Application".into(),
            String::new(),
        )],
        tags: Vec::new(),
    });
    let mut app = App::new(store, context.coordinator);
    let area = Rect::new(0, 0, 100, 40);
    let mut task_layout = LayoutCtx::new();
    app.layout(area, &mut task_layout);
    let task_path = task_layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "data-view")
        .expect("task table should be focusable")
        .path
        .clone();
    app.dispatch_event(
        &EventRoute::new(task_path),
        &TuiEvent::Key(Key::Char(']').into()),
        &mut EventCtx::default(),
    );
    let mut calendar_layout = LayoutCtx::new();
    app.layout(area, &mut calendar_layout);
    let calendar = calendar_layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "calendar")
        .expect("calendar should be focusable");
    let workspace = calendar_layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "field"
                && target
                    .path
                    .keys()
                    .iter()
                    .any(|part| part.as_str() == "workspace")
        })
        .expect("workspace filter should be focusable");
    let mut ctx = EventCtx::default();

    let outcome = app.dispatch_event(
        &EventRoute::new(workspace.path.clone()),
        &TuiEvent::Key(Key::Esc.into()),
        &mut ctx,
    );

    assert!(outcome.handled());
    assert_eq!(
        ctx.focus_request(),
        Some(&FocusRequest::TargetAt {
            path: calendar.path.clone(),
            id: calendar.id.clone(),
        })
    );
}

#[test]
fn first_tab_switches_focus_calendar_and_note_content() {
    let note = test_note("note-0", 0);
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(store, context.coordinator);
    set_notes(&mut app, vec![note]);
    let area = Rect::new(0, 0, 100, 40);
    let mut task_layout = LayoutCtx::new();
    app.layout(area, &mut task_layout);
    let task_path = task_layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "data-view")
        .expect("task table should be focusable")
        .path
        .clone();
    let mut calendar_switch = EventCtx::default();

    app.dispatch_event(
        &EventRoute::new(task_path),
        &TuiEvent::Key(Key::Char(']').into()),
        &mut calendar_switch,
    );

    assert_eq!(
        calendar_switch.focus_request(),
        Some(&initial_calendar_focus_request())
    );

    let mut calendar_layout = LayoutCtx::new();
    app.layout(area, &mut calendar_layout);
    let calendar_path = calendar_layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "calendar")
        .expect("calendar should be focusable")
        .path
        .clone();
    let mut notes_switch = EventCtx::default();

    app.dispatch_event(
        &EventRoute::new(calendar_path),
        &TuiEvent::Key(Key::Char(']').into()),
        &mut notes_switch,
    );

    assert_eq!(
        notes_switch.focus_request(),
        Some(&FocusRequest::Path(note_path(
            notes_workspace_focus_path(),
            "note-0",
        )))
    );
}

#[test]
fn escape_from_task_detail_controls_focuses_data_view() {
    let close_keys = [
        KeyEvent::from(Key::Esc),
        KeyEvent {
            code: Key::Char('['),
            modifiers: KeyModifiers::CONTROL,
        },
    ];

    for key in close_keys {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let mut app = App::new(store, Rc::clone(&context.coordinator));
        let mut layout = LayoutCtx::new();
        app.layout(Rect::new(0, 0, 120, 50), &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target
                    .path
                    .keys()
                    .iter()
                    .any(|part| part.as_str() == "title")
            })
            .expect("task title should be focusable")
            .clone();
        let mut focus = FocusManager::new();
        let transition = focus
            .apply_request(
                &FocusRequest::TargetAt {
                    path: target.path.clone(),
                    id: target.id.clone(),
                },
                layout.focus_targets(),
            )
            .expect("detail focus should apply");
        let mut dispatcher = TreeDispatcher::new();
        dispatcher.dispatch_focus(&mut app, transition, AnimationSettings::default());
        let route = EventRoute::new(focus.current_path());
        let activated = dispatcher.dispatch_event(
            &mut app,
            &route,
            &TuiEvent::Key(Key::Enter.into()),
            AnimationSettings::default(),
        );
        assert!(activated.outcome.handled());

        let effects = dispatcher.dispatch_event(
            &mut app,
            &route,
            &TuiEvent::Key(key),
            AnimationSettings::default(),
        );

        assert!(effects.outcome.handled());
        assert_eq!(
            effects.focus_request,
            Some(initial_task_table_focus_request())
        );
    }
}

#[test]
fn task_view_menu_shortcut_opens_and_switches_to_snoozed() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with("active", "Active work", TaskState::InProgress),
            task_with("snoozed", "Snoozed work", TaskState::Snoozed),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let area = Rect::new(0, 0, 80, 40);
    let mut layout = LayoutCtx::new();
    workspace.layout(area, &mut layout);
    let trigger = layout
        .focus_targets()
        .iter()
        .find(|target| {
            let path = target.path.keys();
            path.iter().any(|part| part.as_str() == "view")
                && path.iter().any(|part| part.as_str() == "trigger")
        })
        .expect("view menu trigger should be focusable")
        .clone();
    let trigger_route = EventRoute::new(trigger.path);
    let mut open_ctx = EventCtx::default();
    let open = workspace.dispatch_event(
        &trigger_route,
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_VIEW_MENU.hotkey())),
        &mut open_ctx,
    );
    assert!(open.handled());
    assert!(matches!(
        open_ctx.focus_request(),
        Some(FocusRequest::TargetAt { id, .. }) if id.as_str() == "search"
    ));

    let mut open_layout = LayoutCtx::new();
    workspace.layout(area, &mut open_layout);
    let panel = open_layout
        .focus_targets()
        .iter()
        .find(|target| {
            let path = target.path.keys();
            path.iter().any(|part| part.as_str() == "view")
                && path.iter().any(|part| part.as_str() == "menu")
        })
        .expect("open view menu search should be focusable")
        .clone();
    let panel_route = EventRoute::new(panel.path);
    let next = KeyEvent {
        code: Key::Char('j'),
        modifiers: KeyModifiers::CONTROL,
    };
    for key in [next, next, next, KeyEvent::from(Key::Enter)] {
        let outcome =
            workspace.dispatch_event(&panel_route, &TuiEvent::Key(key), &mut EventCtx::default());
        assert!(outcome.handled(), "menu ignored {key:?}");
    }

    assert_eq!(workspace.task_view, TaskView::Snoozed);
    workspace.layout(area, &mut LayoutCtx::new());
    let text = rendered_text(&workspace, area);
    assert!(text.contains("󰒲 Snoozed"));
    assert!(text.contains("Snoozed work"));
    assert!(!text.contains("Active work"));
}

#[test]
fn task_views_group_tasks_by_workflow_state() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with("active", "Active work", TaskState::InProgress),
            task_with("todo", "Todo work", TaskState::Todo),
            task_with("backlog", "Backlog work", TaskState::Backlog),
            task_with("done", "Completed work", TaskState::Done),
            task_with("rejected", "Rejected work", TaskState::Rejected),
            task_with("snoozed", "Snoozed work", TaskState::Snoozed),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let area = Rect::new(0, 0, 120, 40);

    workspace.layout(area, &mut LayoutCtx::new());
    let active = rendered_text(&workspace, area);
    assert!(active.contains("Active work"));
    assert!(active.contains("Todo work"));
    assert!(!active.contains("Backlog work"));
    assert!(!active.contains("Completed work"));

    *workspace.pending_task_view.borrow_mut() = Some(TaskView::Backlog);
    assert!(workspace.sync_task_view_change());
    workspace.layout(area, &mut LayoutCtx::new());
    let backlog = rendered_text(&workspace, area);
    assert!(backlog.contains("Backlog work"));
    assert!(!backlog.contains("Todo work"));

    *workspace.pending_task_view.borrow_mut() = Some(TaskView::Snoozed);
    assert!(workspace.sync_task_view_change());
    workspace.layout(area, &mut LayoutCtx::new());
    let snoozed = rendered_text(&workspace, area);
    assert!(snoozed.contains("Snoozed work"));
    assert!(!snoozed.contains("Todo work"));

    *workspace.pending_task_view.borrow_mut() = Some(TaskView::Archived);
    assert!(workspace.sync_task_view_change());
    workspace.layout(area, &mut LayoutCtx::new());
    let archived = rendered_text(&workspace, area);
    assert!(archived.contains("Completed work"));
    assert!(archived.contains("Rejected work"));
    assert!(!archived.contains("Snoozed work"));

    *workspace.pending_task_view.borrow_mut() = Some(TaskView::All);
    assert!(workspace.sync_task_view_change());
    workspace.layout(area, &mut LayoutCtx::new());
    let all = rendered_text(&workspace, area);
    for title in ["Active work", "Todo work", "Backlog work", "Snoozed work"] {
        assert!(all.contains(title), "missing task in All view: {title}");
    }
    assert!(!all.contains("Completed work"));
    assert!(!all.contains("Rejected work"));
}

#[test]
fn switching_views_selects_first_visible_task() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with_rank("active-1", "Active one", TaskState::InProgress, 1),
            task_with_rank("backlog-1", "Backlog one", TaskState::Backlog, 2),
            task_with_rank("backlog-2", "Backlog two", TaskState::Backlog, 1),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);

    *workspace.pending_task_view.borrow_mut() = Some(TaskView::Backlog);
    assert!(workspace.sync_task_view_change());
    select_workspace_task(&mut workspace, "backlog-2");
    *workspace.pending_task_view.borrow_mut() = Some(TaskView::Active);
    assert!(workspace.sync_task_view_change());
    *workspace.pending_task_view.borrow_mut() = Some(TaskView::Backlog);
    assert!(workspace.sync_task_view_change());

    assert_eq!(
        workspace.table().highlighted_id().as_deref(),
        Some("backlog-2")
    );
    assert_eq!(workspace.detail().task_id.as_deref(), Some("backlog-2"));
}

#[test]
fn applying_filters_preserves_selected_task_when_still_visible() {
    let workspace_id = "workspace-1".to_string();
    let tag_id = "tag-1".to_string();
    let mut tasks = vec![
        task_with_rank("first", "First", TaskState::Todo, 3),
        task_with_rank("selected", "Selected", TaskState::Todo, 2),
        task_with_rank("last", "Last", TaskState::Todo, 1),
    ];
    for task in &mut tasks {
        task.workspace_id = Some(workspace_id.clone());
        task.tag_ids.push(tag_id.clone());
    }
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks,
        people: Vec::new(),
        workspaces: vec![Workspace::new(
            workspace_id.clone(),
            "APP".into(),
            "Application".into(),
            String::new(),
        )],
        tags: vec![Tag::new(tag_id.clone(), "Tag".into())],
    });
    let mut workspace = TaskWorkspace::new(context);
    select_workspace_task(&mut workspace, "selected");

    *workspace.active_workspace_filter.borrow_mut() = Some(workspace_id);
    assert!(workspace.sync_workspace_filter_change());
    assert_eq!(
        workspace.table().highlighted_id().as_deref(),
        Some("selected")
    );

    workspace.active_label_filter.borrow_mut().push(tag_id);
    assert!(workspace.sync_label_filter_change());
    assert_eq!(
        workspace.table().highlighted_id().as_deref(),
        Some("selected")
    );
}

#[test]
fn switching_task_view_clears_table_search() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with("active-1", "Active one", TaskState::InProgress),
            task_with("todo-1", "Todo one", TaskState::Todo),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.table_mut().set_search_query("Active");
    *workspace.pending_task_view.borrow_mut() = Some(TaskView::Backlog);

    assert!(workspace.sync_task_view_change());

    assert!(workspace.table().transform_state().search.is_empty());
}

#[test]
fn switching_views_focuses_first_visible_table_row() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with_rank("active-1", "Active one", TaskState::InProgress, 1),
            task_with_rank("backlog-1", "Backlog one", TaskState::Backlog, 2),
            task_with_rank("backlog-2", "Backlog two", TaskState::Backlog, 1),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    *workspace.pending_task_view.borrow_mut() = Some(TaskView::Backlog);
    let mut ctx = EventCtx::default();

    workspace.event(&TuiEvent::Key(Key::Char('~').into()), &mut ctx);

    assert_eq!(
        workspace.table().highlighted_id().as_deref(),
        Some("backlog-2")
    );
    assert_eq!(
        ctx.focus_request(),
        Some(&initial_task_table_focus_request())
    );
}

#[test]
fn state_change_selects_next_visible_task() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with_rank("active-1", "Active one", TaskState::InProgress, 3),
            task_with_rank("active-2", "Active two", TaskState::InProgress, 2),
            task_with_rank("active-3", "Active three", TaskState::InProgress, 1),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    select_workspace_task(&mut workspace, "active-2");

    store.borrow_mut().dispatch(AppEvent::PatchTask {
        task_id: "active-2".to_string(),
        patch: TaskPatch::State(TaskState::Done),
    });
    workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());

    assert_eq!(
        workspace.table().highlighted_id().as_deref(),
        Some("active-1")
    );
    assert_eq!(workspace.detail().task_id.as_deref(), Some("active-1"));
}

#[test]
fn detail_state_change_focuses_newly_selected_table_row() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with_rank("active-1", "Active one", TaskState::InProgress, 3),
            task_with_rank("active-2", "Active two", TaskState::InProgress, 2),
            task_with_rank("active-3", "Active three", TaskState::InProgress, 1),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    select_workspace_task(&mut workspace, "active-2");
    workspace
        .detail_mut()
        .patches
        .borrow_mut()
        .push(TaskPatch::State(TaskState::Done));
    let mut ctx = EventCtx::default();

    workspace.event(&TuiEvent::Key(Key::Char('~').into()), &mut ctx);

    assert_eq!(
        workspace.table().highlighted_id().as_deref(),
        Some("active-1")
    );
    assert_eq!(
        ctx.focus_request(),
        Some(&initial_task_table_focus_request())
    );
}

#[test]
fn state_change_for_last_task_selects_previous_visible_task() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with("active-1", "Active one", TaskState::InProgress),
            task_with("active-2", "Active two", TaskState::InProgress),
            task_with("active-3", "Active three", TaskState::InProgress),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    select_workspace_task(&mut workspace, "active-1");

    store.borrow_mut().dispatch(AppEvent::PatchTask {
        task_id: "active-1".to_string(),
        patch: TaskPatch::State(TaskState::Done),
    });
    workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());

    assert_eq!(
        workspace.table().highlighted_id().as_deref(),
        Some("active-2")
    );
    assert_eq!(workspace.detail().task_id.as_deref(), Some("active-2"));
}

#[test]
fn deleting_task_selects_next_visible_row_or_previous_at_end() {
    let tasks = || {
        let mut low = task_with("low", "Low", TaskState::InProgress);
        low.priority = TaskPriority::Low;
        low.rank = 3;
        let mut medium = task_with("medium", "Medium", TaskState::InProgress);
        medium.priority = TaskPriority::Medium;
        medium.rank = 2;
        let mut high = task_with("high", "High", TaskState::InProgress);
        high.priority = TaskPriority::High;
        high.rank = 1;
        vec![low, medium, high]
    };

    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: tasks(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    select_workspace_task(&mut workspace, "medium");
    store
        .borrow_mut()
        .dispatch(AppEvent::TaskDeleted("medium".to_string()));
    workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());

    assert_eq!(workspace.table().highlighted_id().as_deref(), Some("low"));
    assert_eq!(workspace.detail().task_id.as_deref(), Some("low"));

    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: tasks(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    select_workspace_task(&mut workspace, "low");
    store
        .borrow_mut()
        .dispatch(AppEvent::TaskDeleted("low".to_string()));
    workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());

    assert_eq!(
        workspace.table().highlighted_id().as_deref(),
        Some("medium")
    );
    assert_eq!(workspace.detail().task_id.as_deref(), Some("medium"));
}

#[test]
fn detail_state_change_with_no_remaining_tasks_clears_detail() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![task_with("active-1", "Active one", TaskState::InProgress)],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let area = Rect::new(0, 0, 100, 30);
    workspace.layout(area, &mut LayoutCtx::new());

    workspace
        .detail_mut()
        .patches
        .borrow_mut()
        .push(TaskPatch::State(TaskState::Done));
    assert!(workspace.sync_detail_changes(None).changed);
    workspace.layout(area, &mut LayoutCtx::new());

    let text = rendered_text(&workspace, area);
    assert!(text.contains("No active tasks"));
    assert!(!text.contains("No task selected."));
    assert_eq!(workspace.table().highlighted_id(), None);
    assert_eq!(workspace.detail().task_id, None);
    assert!(!workspace.layout.is_second_visible());
}

#[path = "tests_workspace.rs"]
mod workspace;
