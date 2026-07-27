use super::*;
use crate::domain::{SaveTarget, TaskField, WorkspaceSnapshot};
use ratatui::{Terminal, backend::TestBackend};
use sqlx::any::AnyPoolOptions;
use tuicore::{
    FocusManager, HotkeyEvent, Key, KeyEvent, KeyModifiers, Propagation, TreeDispatcher,
};

fn test_task() -> Task {
    Task {
        id: "task-1".to_string(),
        title: "Original".to_string(),
        state: TaskState::InProgress,
        size: TaskSize::Small,
        priority: TaskPriority::Medium,
        start_date: None,
        due_date: None,
        snoozed_until: None,
        people_ids: Vec::new(),
        project_ids: Vec::new(),
        tag_ids: Vec::new(),
        links: Vec::new(),
        description: "Existing detail".to_string(),
    }
}

#[test]
fn status_bar_enables_weather_and_exposes_forecast_menu() {
    assert!(weather_provider_config().is_enabled());
    assert!(STATUS_BAR_MENU_ITEMS.contains(&StatusBarMenuItem::WeatherForecast));
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

fn task_with(id: &str, title: &str, state: TaskState) -> Task {
    let mut task = test_task();
    task.id = id.to_string();
    task.title = title.to_string();
    task.state = state;
    task
}

fn yank_task_table(table: &mut TaskTable) -> tuicore::DispatchEffects<AppMsg> {
    let mut ctx = EventCtx::default();
    let outcome = table.event(&TuiEvent::Yank, &mut ctx);
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
    let context = AppContext {
        store: Rc::clone(&store),
        coordinator,
    };
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
fn external_refresh_repopulates_selected_task_detail_once_draft_is_safe() {
    let task = test_task();
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![task.clone()],
        people: vec![],
        projects: vec![],
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
            projects: vec![],
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
fn task_toolbar_shows_icon_view_label_and_new_binding() {
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
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let area = Rect::new(0, 0, 80, 40);

    workspace.layout(area, &mut LayoutCtx::new());
    let text = rendered_text(&workspace, area);

    for expected in [
        " Active",
        &keys::TASK_VIEW_MENU.label(),
        &keys::TASK_QUICK_CREATE.label(),
        "New",
    ] {
        assert!(text.contains(expected), "missing toolbar text: {expected}");
    }
    assert!(!text.contains("View:"));
    assert!(!text.contains("Resolve"));
    assert!(!text.contains("Permanently"));
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
fn task_table_fixed_columns_keep_padding_before_flexible_title() {
    for width in [40, 100, 200] {
        let mut table = task_table(
            vec![task_with("active", "Zebra work", TaskState::InProgress)],
            None,
        );
        let area = Rect::new(0, 0, width, 5);
        <TaskTable as TuiNode<AppMsg>>::layout(&mut table, area, &mut LayoutCtx::new());
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))
            .expect("terminal should build");

        terminal
            .draw(|frame| {
                <TaskTable as TuiNode<AppMsg>>::render(&table, frame, area, &mut RenderCtx::new())
            })
            .expect("table should render");

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((0, 1)).unwrap().symbol(), "");
        assert_eq!(buffer.cell((1, 1)).unwrap().symbol(), " ");
        assert_eq!(buffer.cell((2, 1)).unwrap().symbol(), "󰇼");
        assert_eq!(buffer.cell((3, 1)).unwrap().symbol(), " ");
        assert_eq!(buffer.cell((4, 1)).unwrap().symbol(), "S");
        assert_eq!(buffer.cell((5, 1)).unwrap().symbol(), "M");
        assert_eq!(buffer.cell((6, 1)).unwrap().symbol(), "A");
        assert_eq!(buffer.cell((7, 1)).unwrap().symbol(), " ");
        assert_eq!(buffer.cell((8, 1)).unwrap().symbol(), "Z");
    }
}

#[test]
fn task_table_shows_horizontal_scrollbar_for_long_titles() {
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
        scrollbar.contains('━') || scrollbar.contains('─'),
        "missing horizontal scrollbar: {scrollbar:?}"
    );
}

#[test]
fn narrow_task_workspace_keeps_detail_view_visible() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);

    workspace.layout(Rect::new(0, 0, 80, 40), &mut LayoutCtx::new());

    let (table_area, detail_area) = workspace.layout.second().child_areas();
    assert!(detail_area.height > 0);
    assert_eq!(table_area.height + detail_area.height, 39);
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
fn task_table_sorts_high_priority_first_and_newest_first_within_priority() {
    let mut older_medium = task_with("older-medium", "Older medium", TaskState::Todo);
    older_medium.priority = TaskPriority::Medium;
    let mut high = task_with("high", "High priority", TaskState::Todo);
    high.priority = TaskPriority::High;
    let mut newer_medium = task_with("newer-medium", "Newer medium", TaskState::Todo);
    newer_medium.priority = TaskPriority::Medium;
    let mut low = task_with("low", "Low priority", TaskState::Todo);
    low.priority = TaskPriority::Low;
    let rows = task_rows_for_view(&[older_medium, high, newer_medium, low], TaskView::Active);
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
fn yanking_highlighted_task_copies_pretty_resolved_agent_json() {
    let ada = Person {
        id: "person-ada".into(),
        name: "Ada Lovelace".into(),
        email: "ada@example.com".into(),
        about: "Computing pioneer".into(),
        active: true,
    };
    let grace = Person {
        id: "person-grace".into(),
        name: "Grace Hopper".into(),
        email: "grace@example.com".into(),
        about: "Compiler expert".into(),
        active: false,
    };
    let project_alpha = Project {
        id: "project-alpha".into(),
        key: "ALPHA".into(),
        name: "Alpha".into(),
        description: "First project".into(),
        lead_person_id: Some(grace.id.clone()),
    };
    let project_beta = Project {
        id: "project-beta".into(),
        key: "BETA".into(),
        name: "Beta".into(),
        description: String::new(),
        lead_person_id: None,
    };
    let urgent = Tag {
        id: "tag-urgent".into(),
        label: "urgent".into(),
    };
    let backend = Tag {
        id: "tag-backend".into(),
        label: "backend".into(),
    };
    let first = task_with("task-first", "Wrong highlighted task", TaskState::Todo);
    let mut highlighted = task_with("task-highlighted", "Ship agent export", TaskState::Snoozed);
    highlighted.description = "Full detail\nwith context".into();
    highlighted.size = TaskSize::Big;
    highlighted.priority = TaskPriority::High;
    highlighted.start_date = None;
    highlighted.due_date = Some("2026-08-04".into());
    highlighted.snoozed_until = Some(PrimitiveDateTime::new(
        Date::from_calendar_date(2026, time::Month::August, 3).unwrap(),
        time::Time::from_hms(9, 8, 7).unwrap(),
    ));
    highlighted.people_ids = vec![grace.id.clone(), ada.id.clone()];
    highlighted.project_ids = vec![project_beta.id.clone(), project_alpha.id.clone()];
    highlighted.tag_ids = vec![backend.id.clone(), urgent.id.clone()];
    highlighted.links = vec![
        "www.example.com/work".into(),
        "https://tracker.example/ABC-1".into(),
    ];
    let copy_context = TaskCopyContext::new(
        &[ada.clone(), grace.clone()],
        &[project_alpha.clone(), project_beta.clone()],
        &[urgent.clone(), backend.clone()],
    );
    let mut table = task_table_with_copy_context(vec![first, highlighted], None, copy_context);
    table.highlight_id(&"task-highlighted".to_string());

    let effects = yank_task_table(&mut table);
    let payload = effects
        .clipboard
        .expect("yank should request clipboard copy");
    let json: serde_json::Value = serde_json::from_str(&payload).expect("copy should be JSON");

    assert!(
        payload.contains("\n  \"id\""),
        "JSON should be pretty printed"
    );
    assert_eq!(json["id"], "task-highlighted");
    assert_eq!(json["title"], "Ship agent export");
    assert_eq!(json["description"], "Full detail\nwith context");
    assert!(json.get("detail").is_none());
    assert_eq!(json["state"], "snoozed");
    assert_eq!(json["size"], "big");
    assert_eq!(json["priority"], "high");
    assert!(json["start_date"].is_null());
    assert_eq!(json["due_date"], "2026-08-04");
    assert_eq!(json["snoozed_until"], "2026-08-03T09:08:07");
    assert_eq!(
        json["people"],
        serde_json::json!([
            {"id": "person-grace", "name": "Grace Hopper", "email": "grace@example.com", "active": false},
            {"id": "person-ada", "name": "Ada Lovelace", "email": "ada@example.com", "active": true}
        ])
    );
    assert_eq!(
        json["projects"],
        serde_json::json!([
            {"id": "project-beta", "key": "BETA", "name": "Beta", "description": "", "lead": null},
            {"id": "project-alpha", "key": "ALPHA", "name": "Alpha", "description": "First project", "lead": {"id": "person-grace", "name": "Grace Hopper", "email": "grace@example.com", "active": false}}
        ])
    );
    assert_eq!(
        json["tags"],
        serde_json::json!([
            {"id": "tag-backend", "label": "backend"},
            {"id": "tag-urgent", "label": "urgent"}
        ])
    );
    assert_eq!(
        json["links"],
        serde_json::json!([
            "https://www.example.com/work",
            "https://tracker.example/ABC-1"
        ])
    );
    for excluded in [
        "revision",
        "people_ids",
        "project_ids",
        "tag_ids",
        "workspace_revision",
        "selected_task_id",
        "save_errors",
    ] {
        assert!(json.get(excluded).is_none(), "unexpected field: {excluded}");
    }
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
fn yanking_task_with_missing_relation_copies_descriptive_json_error() {
    let mut task = test_task();
    task.people_ids = vec!["missing-person".into()];
    let mut table =
        task_table_with_copy_context(vec![task], Some("task-1"), TaskCopyContext::default());

    let effects = yank_task_table(&mut table);
    let payload = effects.clipboard.expect("error document should be copied");
    let json: serde_json::Value = serde_json::from_str(&payload).expect("error should be JSON");

    assert_eq!(
        json,
        serde_json::json!({
            "error": {
                "message": "task copy could not resolve relationship",
                "task_id": "task-1",
                "relation": "person",
                "id": "missing-person"
            }
        })
    );
}

#[test]
fn app_startup_selects_and_focuses_first_sorted_task() {
    let mut older_low = task_with("older-low", "Older low", TaskState::InProgress);
    older_low.priority = TaskPriority::Low;
    let mut newer_high = task_with("newer-high", "Newer high", TaskState::InProgress);
    newer_high.priority = TaskPriority::High;
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![older_low, newer_high],
        people: Vec::new(),
        projects: Vec::new(),
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
                    .any(|part| part.as_str() == "links")
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
        projects: Vec::new(),
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

    for hotkey in [
        keys::TASK_TITLE_FIELD.hotkey(),
        keys::TASK_TAGS_FIELD.hotkey(),
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
fn enter_on_task_link_opens_it_in_the_browser() {
    let mut task = test_task();
    task.links = vec!["www.example.com/item".to_string()];
    let opened = Rc::new(RefCell::new(Vec::new()));
    let opened_by_handler = Rc::clone(&opened);
    let mut input =
        TaskLinksInput::with_opener(&task, Rc::new(RefCell::new(Vec::new())), move |url| {
            opened_by_handler.borrow_mut().push(url.to_string());
            Ok(())
        });
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
        &TuiEvent::Key(Key::Enter.into()),
        AnimationSettings::default(),
    );

    assert!(effects.outcome.handled());
    assert_eq!(opened.borrow().as_slice(), ["https://www.example.com/item"]);
}

#[test]
fn task_table_state_icon_uses_row_text_color() {
    let mut table = task_table(
        vec![task_with("active", "Zebra work", TaskState::InProgress)],
        None,
    );
    let area = Rect::new(0, 0, 100, 5);
    <TaskTable as TuiNode<AppMsg>>::layout(&mut table, area, &mut LayoutCtx::new());
    let mut terminal =
        Terminal::new(TestBackend::new(area.width, area.height)).expect("terminal should build");
    terminal
        .draw(|frame| {
            <TaskTable as TuiNode<AppMsg>>::render(&table, frame, area, &mut RenderCtx::new())
        })
        .expect("table should render");
    let cells = terminal.backend().buffer().content();
    let icon = cells
        .iter()
        .find(|cell| cell.symbol() == "")
        .expect("state icon should render");
    let title = cells
        .iter()
        .find(|cell| cell.symbol() == "Z")
        .expect("task title should render");

    assert_eq!(icon.fg, title.fg);
}

#[test]
fn task_toolbar_new_button_emits_create_action() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let mut layout = LayoutCtx::new();
    workspace.layout(Rect::new(0, 0, 80, 40), &mut layout);
    let button_path = layout
        .focus_targets()
        .iter()
        .find(|target| target.path.keys().iter().any(|part| part.as_str() == "new"))
        .expect("missing new task toolbar button")
        .path
        .clone();

    let mut create_ctx = EventCtx::default();
    let create = workspace.dispatch_event(
        &EventRoute::new(button_path.clone()),
        &TuiEvent::Key(Key::Enter.into()),
        &mut create_ctx,
    );
    assert!(create.handled());
    assert!(matches!(create_ctx.messages(), [AppMsg::OpenCreateTask]));

    let mut hotkey_ctx = EventCtx::default();
    let hotkey = workspace.dispatch_event(
        &EventRoute::new(button_path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_QUICK_CREATE.hotkey())),
        &mut hotkey_ctx,
    );
    assert!(hotkey.handled());
    assert!(matches!(hotkey_ctx.messages(), [AppMsg::OpenCreateTask]));
}

#[test]
fn escape_from_task_toolbar_controls_focuses_data_view() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let mut layout = LayoutCtx::new();
    workspace.layout(Rect::new(0, 0, 80, 40), &mut layout);
    let toolbar_paths = [
        layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().iter().any(|part| part.as_str() == "new"))
            .expect("new task button should be focusable")
            .path
            .clone(),
        layout
            .focus_targets()
            .iter()
            .find(|target| {
                let path = target.path.keys();
                path.iter().any(|part| part.as_str() == "view")
                    && path
                        .iter()
                        .any(|part| part.as_str() == TASK_VIEW_MENU_TRIGGER)
            })
            .expect("task filter button should be focusable")
            .path
            .clone(),
    ];
    let close_keys = [
        KeyEvent::from(Key::Esc),
        KeyEvent {
            code: Key::Char('['),
            modifiers: KeyModifiers::CONTROL,
        },
    ];

    for path in toolbar_paths {
        for key in close_keys {
            let mut ctx = EventCtx::default();
            let outcome = workspace.dispatch_event(
                &EventRoute::new(path.clone()),
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
            projects: Vec::new(),
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
        projects: Vec::new(),
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
                && path
                    .iter()
                    .any(|part| part.as_str() == TASK_VIEW_MENU_TRIGGER)
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
                && path
                    .iter()
                    .any(|part| part.as_str() == TASK_VIEW_MENU_PANEL)
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
        projects: Vec::new(),
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
            task_with("active-1", "Active one", TaskState::InProgress),
            task_with("backlog-1", "Backlog one", TaskState::Backlog),
            task_with("backlog-2", "Backlog two", TaskState::Backlog),
        ],
        people: Vec::new(),
        projects: Vec::new(),
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
fn switching_task_view_clears_table_search() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            task_with("active-1", "Active one", TaskState::InProgress),
            task_with("todo-1", "Todo one", TaskState::Todo),
        ],
        people: Vec::new(),
        projects: Vec::new(),
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
            task_with("active-1", "Active one", TaskState::InProgress),
            task_with("backlog-1", "Backlog one", TaskState::Backlog),
            task_with("backlog-2", "Backlog two", TaskState::Backlog),
        ],
        people: Vec::new(),
        projects: Vec::new(),
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
            task_with("active-1", "Active one", TaskState::InProgress),
            task_with("active-2", "Active two", TaskState::InProgress),
            task_with("active-3", "Active three", TaskState::InProgress),
        ],
        people: Vec::new(),
        projects: Vec::new(),
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
            task_with("active-1", "Active one", TaskState::InProgress),
            task_with("active-2", "Active two", TaskState::InProgress),
            task_with("active-3", "Active three", TaskState::InProgress),
        ],
        people: Vec::new(),
        projects: Vec::new(),
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
        projects: Vec::new(),
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
        let mut medium = task_with("medium", "Medium", TaskState::InProgress);
        medium.priority = TaskPriority::Medium;
        let mut high = task_with("high", "High", TaskState::InProgress);
        high.priority = TaskPriority::High;
        vec![low, medium, high]
    };

    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: tasks(),
        people: Vec::new(),
        projects: Vec::new(),
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
        projects: Vec::new(),
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
        projects: Vec::new(),
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
    assert!(workspace.sync_detail_changes().changed);
    workspace.layout(area, &mut LayoutCtx::new());

    let text = rendered_text(&workspace, area);
    assert!(text.contains("No results found."));
    assert!(text.contains("No task selected."));
    assert_eq!(workspace.table().highlighted_id(), None);
    assert_eq!(workspace.detail().task_id, None);
}

#[path = "tests_workspace.rs"]
mod workspace;
