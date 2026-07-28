use super::*;

#[test]
fn task_table_ignores_data_view_filter_mode_hotkey() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);

    let outcome = workspace
        .table_mut()
        .on_key(KeyEvent::from(Key::Char('f')), Rect::new(0, 0, 80, 20));

    assert!(!outcome.handled);
    assert!(!outcome.changed);
    assert!(workspace.table().transform_state().filters.is_empty());
}

#[test]
fn task_table_hides_column_headers() {
    let table = task_table(
        vec![task_with("work", "Write report", TaskState::Todo)],
        Some("work"),
    );

    let text = rendered_text(&table, Rect::new(0, 0, 80, 10));

    assert!(!text.contains("Size"));
    assert!(!text.contains("Task"));
    assert!(text.contains("Write report"));
}

#[test]
fn hidden_task_cannot_be_deleted_from_empty_active_view() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![task_with("backlog", "Backlog work", TaskState::Backlog)],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.table_focused = true;
    let mut ctx = EventCtx::default();

    let outcome = workspace.event(&TuiEvent::Key(KeyEvent::from(Key::Delete)), &mut ctx);

    assert_eq!(outcome, EventOutcome::Ignored);
    assert!(ctx.messages().is_empty());
}

#[test]
fn created_todo_switches_hidden_view_to_active_and_selects_task() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    *workspace.pending_task_view.borrow_mut() = Some(TaskView::Snoozed);
    workspace.sync_task_view_change();
    workspace.layout(Rect::new(0, 0, 120, 40), &mut LayoutCtx::new());
    let created = Task::quick_capture(
        "task-2".to_string(),
        "Captured".to_string(),
        String::new(),
        TaskSize::Small,
    );

    store
        .borrow_mut()
        .dispatch(AppEvent::TaskCreated(created.clone()));
    workspace.layout(Rect::new(0, 0, 120, 40), &mut LayoutCtx::new());

    assert_eq!(
        store.borrow().state().selected_task_id.as_deref(),
        Some("task-2")
    );
    assert_eq!(workspace.task_view, TaskView::Active);
    assert_eq!(*workspace.active_task_view.borrow(), TaskView::Active);
    assert_eq!(
        workspace.table_mut().highlighted_id().as_deref(),
        Some("task-2")
    );
    assert_eq!(
        workspace.table_mut().selected_id().as_deref(),
        Some("task-2")
    );
    assert_eq!(workspace.detail_mut().task_id.as_deref(), Some("task-2"));
}

#[test]
fn created_todo_stays_in_active_view_and_selects_task() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.layout(Rect::new(0, 0, 120, 40), &mut LayoutCtx::new());
    let created = Task::quick_capture(
        "task-2".to_string(),
        "Captured".to_string(),
        String::new(),
        TaskSize::Small,
    );

    store
        .borrow_mut()
        .dispatch(AppEvent::TaskCreated(created.clone()));
    workspace.layout(Rect::new(0, 0, 120, 40), &mut LayoutCtx::new());

    assert_eq!(workspace.task_view, TaskView::Active);
    assert_eq!(
        workspace.table().highlighted_id().as_deref(),
        Some("task-2")
    );
    assert_eq!(workspace.detail().task_id.as_deref(), Some("task-2"));
}

#[test]
fn escape_keeps_task_table_focused_as_tab_root() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.table_focused = true;
    let mut ctx = EventCtx::default();

    let outcome = workspace.event(&TuiEvent::Key(KeyEvent::from(Key::Esc)), &mut ctx);

    assert!(outcome.handled());
    assert_eq!(ctx.propagation(), Propagation::Stopped);
}

#[test]
fn delete_opens_confirmation_from_focused_task_table() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.table_focused = true;
    let mut ctx = EventCtx::default();

    let outcome = workspace.event(&TuiEvent::Key(KeyEvent::from(Key::Delete)), &mut ctx);

    assert!(outcome.handled());
    assert!(matches!(
        ctx.messages(),
        [AppMsg::OpenDeleteTask(task_id)] if task_id == "task-1"
    ));
}

#[test]
fn quick_menu_opens_from_task_list_or_detail_and_b_snoozes_from_either() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let snooze = TuiEvent::Key(KeyEvent::from(Key::Char('b')));
    let quick_menu = TuiEvent::Key(KeyEvent::from(Key::Char('.')));

    let mut detail_snooze = EventCtx::default();
    assert!(workspace.event(&snooze, &mut detail_snooze).handled());
    assert_eq!(
        detail_snooze.focus_request(),
        Some(&initial_task_table_focus_request())
    );
    assert!(matches!(
        detail_snooze.messages(),
        [AppMsg::OpenTaskSnooze(task_id)] if task_id == "task-1"
    ));

    let mut detail = EventCtx::default();
    assert!(workspace.event(&quick_menu, &mut detail).handled());
    assert!(matches!(
        detail.messages(),
        [AppMsg::OpenTaskQuickMenu(task_id)] if task_id == "task-1"
    ));

    workspace.table_focused = true;
    let mut snooze_ctx = EventCtx::default();
    assert!(workspace.event(&snooze, &mut snooze_ctx).handled());
    assert!(matches!(
        snooze_ctx.messages(),
        [AppMsg::OpenTaskSnooze(task_id)] if task_id == "task-1"
    ));

    let mut focused = EventCtx::default();
    assert!(workspace.event(&quick_menu, &mut focused).handled());
    assert!(matches!(
        focused.messages(),
        [AppMsg::OpenTaskQuickMenu(task_id)] if task_id == "task-1"
    ));

    *workspace.visible_selection.borrow_mut() = None;
    let mut hidden = EventCtx::default();
    assert_eq!(
        workspace.event(&quick_menu, &mut hidden),
        EventOutcome::Ignored
    );
    assert!(hidden.messages().is_empty());
}

#[test]
fn control_m_from_task_detail_focuses_table_and_enters_move_mode() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.table_focused = false;
    let mut ctx = EventCtx::default();

    let outcome = workspace.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('m'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut ctx,
    );

    assert!(outcome.handled());
    assert_eq!(
        ctx.focus_request(),
        Some(&initial_task_table_focus_request())
    );
    assert!(workspace.task_list().is_reordering());
}

#[test]
fn snooze_dialog_uses_open_filled_dropdown_and_quick_selection_message() {
    let now = time::macros::datetime!(2026-07-23 12:00);
    let mut dialog = SnoozeDialog::new("task-1".into(), now, None, false);
    let hint = dialog.measure(LayoutProposal::unbounded());
    assert_eq!(hint.preferred, tuicore::LayoutSize::new(46, 12));

    let mut ctx = EventCtx::default();
    dialog.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
    assert!(matches!(
        ctx.messages(),
        [AppMsg::SnoozeTask { task_id, until, remember_custom: None }]
            if task_id == "task-1" && *until == time::macros::datetime!(2026-07-24 8:00)
    ));
}

#[test]
fn custom_snooze_returns_focus_to_task_table() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let mut open_ctx = EventCtx::default();
    app.open_task_snooze_dialog("task-1", &mut open_ctx);
    let custom = time::macros::datetime!(2026-07-30 14:30);
    let mut submit_ctx = EventCtx::default();

    app.snooze_task("task-1".into(), custom, Some(custom), &mut submit_ctx);

    assert_eq!(
        submit_ctx.focus_request(),
        Some(&initial_task_table_focus_request())
    );
    assert_eq!(
        store.borrow().state().selected_task_id.as_deref(),
        Some("task-1")
    );
}

#[test]
fn snooze_dialog_renders_search_and_options_through_modal_portals() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let mut activation = EventCtx::default();
    let primary = app.primary_dialog();
    primary.replace_layer(
        AppDialog::Snooze(Box::new(SnoozeDialog::new(
            "task-1".into(),
            time::macros::datetime!(2026-07-23 12:00),
            None,
            false,
        ))),
        &mut activation,
    );
    primary.set_fit_content(true);
    primary.set_active_with_context(true, &mut activation);
    let area = Rect::new(0, 0, 100, 30);
    app.layout(area, &mut LayoutCtx::new());
    let mut terminal =
        Terminal::new(TestBackend::new(area.width, area.height)).expect("terminal should build");
    terminal
        .draw(|frame| {
            let mut ctx = RenderCtx::new();
            app.render(frame, area, &mut ctx);
            ctx.flush(frame);
        })
        .expect("app should render");
    let text = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(text.contains("Search..."));
    assert!(text.contains("Tomorrow"));
    assert!(text.contains("This weekend"));
    assert!(text.contains("Pick date & time"));
}

#[test]
fn snoozed_detail_places_datetime_field_before_start_and_end_dates_and_queues_selection() {
    let until = time::macros::datetime!(2026-08-24 8:00);
    let mut task = test_task();
    task.state = TaskState::Snoozed;
    task.snoozed_until = Some(until);
    let mut detail = TaskDetailForm::new(Some(&task), &[], &[], &[], None);
    let area = Rect::new(0, 0, 80, 120);
    detail.layout(area, &mut LayoutCtx::new());
    let text = rendered_text(&detail, area);
    let snoozed_until = text
        .find("Snoozed until")
        .expect("snoozed datetime should render");
    let start_date = text.find("Start date").expect("start date should render");
    let end_date = text.find("End date").expect("end date should render");
    assert!(snoozed_until < start_date);
    assert!(start_date < end_date);

    let mut layout = LayoutCtx::new();
    detail.layout(area, &mut layout);
    let target = layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "date-time-picker-dropdown")
        .expect("snoozed datetime should be focusable")
        .clone();
    assert_eq!(target.hotkey_sequences, ["su"]);
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: target.path.clone(),
                id: target.id.clone(),
            },
            layout.focus_targets(),
        )
        .expect("datetime focus should apply");
    let mut dispatcher = TreeDispatcher::new();
    dispatcher.dispatch_focus(&mut detail, transition, AnimationSettings::default());
    let route = EventRoute::new(focus.current_path());
    for key in [Key::Enter, Key::Right, Key::Enter, Key::Enter, Key::Enter] {
        let effects = dispatcher.dispatch_event(
            &mut detail,
            &route,
            &TuiEvent::Key(key.into()),
            AnimationSettings::default(),
        );
        assert!(effects.outcome.handled());
    }

    let patches = detail.take_patches();
    assert!(matches!(
        patches.as_slice(),
        [(task_id, TaskPatch::Snooze { until, remember_custom: None })]
            if task_id == "task-1" && *until == time::macros::datetime!(2026-08-25 8:00)
    ));

    task.state = TaskState::Todo;
    task.snoozed_until = None;
    let mut active_detail = TaskDetailForm::new(Some(&task), &[], &[], &[], None);
    active_detail.layout(area, &mut LayoutCtx::new());
    assert!(!rendered_text(&active_detail, area).contains("Snoozed until"));
}

#[test]
fn snoozed_until_hotkey_opens_detail_picker() {
    let mut task = test_task();
    task.state = TaskState::Snoozed;
    task.snoozed_until = Some(time::macros::datetime!(2026-07-24 8:00));
    let mut detail = TaskDetailForm::new(Some(&task), &[], &[], &[], None);
    let area = Rect::new(0, 0, 80, 120);
    let mut layout = LayoutCtx::new();
    detail.layout(area, &mut layout);
    let target = layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "date-time-picker-dropdown")
        .expect("snoozed datetime should be focusable");

    let effects = TreeDispatcher::new().dispatch_event(
        &mut detail,
        &EventRoute::new(target.path.clone()),
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_SNOOZED_UNTIL_FIELD.hotkey())),
        AnimationSettings::default(),
    );

    assert!(effects.outcome.handled());
    assert!(effects.layout);
}

#[test]
fn active_snooze_modal_receives_routed_navigation_and_selection() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let mut activation = EventCtx::default();
    let primary = app.primary_dialog();
    primary.replace_layer(
        AppDialog::Snooze(Box::new(SnoozeDialog::new(
            "task-1".into(),
            time::macros::datetime!(2026-07-23 12:00),
            None,
            false,
        ))),
        &mut activation,
    );
    primary.set_fit_content(true);
    primary.set_active_with_context(true, &mut activation);
    let area = Rect::new(0, 0, 100, 30);
    let mut layout = LayoutCtx::new();
    app.layout(area, &mut layout);
    let menu = layout
        .focus_targets()
        .iter()
        .rev()
        .find(|target| target.id.as_str() == "input")
        .expect("active modal dropdown search should be focusable")
        .clone();
    let mut focus = FocusManager::new();
    let mut dispatcher = TreeDispatcher::new();
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: menu.path.clone(),
                id: menu.id.clone(),
            },
            layout.focus_targets(),
        )
        .expect("modal menu focus should apply");
    dispatcher.dispatch_focus(&mut app, transition, AnimationSettings::default());
    let route = EventRoute::new(focus.current_path());
    let down = dispatcher.dispatch_event(
        &mut app,
        &route,
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('j'),
            modifiers: KeyModifiers::CONTROL,
        }),
        AnimationSettings::default(),
    );
    assert!(down.outcome.handled());
    let select = dispatcher.dispatch_event(
        &mut app,
        &route,
        &TuiEvent::Key(Key::Enter.into()),
        AnimationSettings::default(),
    );
    assert!(matches!(
        select.messages.as_slice(),
        [AppMsg::SnoozeTask {
            task_id,
            until,
            remember_custom: None
        }] if task_id == "task-1" && *until == time::macros::datetime!(2026-07-25 8:00)
    ));
}

#[test]
fn backspace_opens_confirmation_from_focused_task_table() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.table_focused = true;
    let mut ctx = EventCtx::default();

    let outcome = workspace.event(&TuiEvent::Key(Key::Backspace.into()), &mut ctx);

    assert!(outcome.handled());
    assert!(matches!(
        ctx.messages(),
        [AppMsg::OpenDeleteTask(task_id)] if task_id == "task-1"
    ));
}

#[test]
fn backspace_targets_visible_task_even_when_store_selection_is_stale() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            test_task(),
            task_with("task-2", "Second", TaskState::InProgress),
        ],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.table_focused = true;
    workspace.table_mut().highlight_id(&"task-2".to_string());
    workspace.table_mut().select_id("task-2".to_string());
    workspace.table_mut().take_events();
    *workspace.visible_selection.borrow_mut() = Some("task-2".to_string());
    let mut ctx = EventCtx::default();

    let outcome = workspace.event(&TuiEvent::Key(Key::Backspace.into()), &mut ctx);

    assert!(outcome.handled());
    assert!(matches!(
        ctx.messages(),
        [AppMsg::OpenDeleteTask(task_id)] if task_id == "task-2"
    ));
}

#[test]
fn ctrl_x_opens_confirmation_from_focused_task_table() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.table_focused = true;
    let mut ctx = EventCtx::default();
    let key = KeyEvent {
        code: Key::Char('x'),
        modifiers: KeyModifiers::CONTROL,
    };

    let outcome = workspace.event(&TuiEvent::Key(key), &mut ctx);

    assert!(outcome.handled());
    assert!(matches!(
        ctx.messages(),
        [AppMsg::OpenDeleteTask(task_id)] if task_id == "task-1"
    ));
}

#[test]
fn completed_task_moves_from_in_progress_to_archived_view() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let area = Rect::new(0, 0, 120, 40);
    workspace.layout(area, &mut LayoutCtx::new());

    store.borrow_mut().dispatch(AppEvent::PatchTask {
        task_id: "task-1".to_string(),
        patch: TaskPatch::State(TaskState::Done),
    });
    workspace.layout(area, &mut LayoutCtx::new());

    let text = rendered_text(&workspace, area);
    assert!(!text.contains("Original"));
    assert!(text.contains("No results found."));

    *workspace.pending_task_view.borrow_mut() = Some(TaskView::Archived);
    assert!(workspace.sync_task_view_change());
    workspace.layout(area, &mut LayoutCtx::new());

    let text = rendered_text(&workspace, area);
    assert!(text.contains("Original"));
    assert!(text.contains("Done"));
}

#[test]
fn confirmed_delete_removes_task_from_state_immediately() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);

    app.delete_task("task-1".to_string(), &mut EventCtx::default());

    assert!(app.context.store.borrow().state().tasks.is_empty());
}

#[test]
fn management_create_dialog_layers_over_management_workspace() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let mut ctx = EventCtx::default();

    app.open_management_dialog(ManagementDialogKind::People, &mut ctx);
    app.open_create_management_dialog(ManagementDialogKind::People, &mut ctx);

    assert!(app.root.is_active());
    assert!(app.root.base().is_active());
    assert!(matches!(app.root.layer(), AppDialog::CreateManagement(_)));
    assert!(matches!(app.root.base().layer(), AppDialog::People(_)));

    app.close_management_overlay(&mut ctx);

    assert!(!app.root.is_active());
    assert!(app.root.base().is_active());
    assert!(matches!(app.root.base().layer(), AppDialog::People(_)));
}

#[test]
fn delete_confirmation_uses_d_shortcut() {
    let mut dialog = delete_task_dialog(&test_task());
    let mut ctx = EventCtx::default();

    let outcome = dialog.event(&TuiEvent::Key(KeyEvent::from(Key::Char('d'))), &mut ctx);

    assert!(outcome.handled());
    assert!(matches!(
        ctx.messages(),
        [AppMsg::DeleteTaskConfirmed(task_id)] if task_id == "task-1"
    ));

    let mut dialog = delete_task_dialog(&test_task());
    let mut old_shortcut_ctx = EventCtx::default();
    dialog.event(
        &TuiEvent::Key(KeyEvent::from(Key::Char('o'))),
        &mut old_shortcut_ctx,
    );
    assert!(old_shortcut_ctx.messages().is_empty());
}

#[test]
fn delete_task_dialog_fits_its_content() {
    let snapshot = WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    };
    let (_runtime, context, _store) = test_context(snapshot);
    let mut app = App::new(context.store, context.coordinator);
    let area = Rect::new(0, 0, 120, 40);

    app.open_delete_task_dialog("task-1", &mut EventCtx::default());
    let mut delete_layout = LayoutCtx::new();
    app.layout(area, &mut delete_layout);
    let delete_area = delete_layout
        .overlays()
        .first()
        .expect("delete dialog should register an overlay")
        .area;

    assert!(delete_area.width > 20);
    assert!(delete_area.height >= 3);
    assert!(delete_area.width < area.width / 2);
    assert!(delete_area.height < area.height / 4);
}

#[test]
fn create_task_dialog_fits_its_content_height() {
    let snapshot = WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    };
    let (_runtime, context, _store) = test_context(snapshot);
    let mut app = App::new(context.store, context.coordinator);
    let area = Rect::new(0, 0, 120, 40);

    app.open_create_task_dialog(&mut EventCtx::default());
    let mut layout = LayoutCtx::new();
    app.layout(area, &mut layout);
    let dialog_area = layout
        .overlays()
        .first()
        .expect("create task dialog should register an overlay")
        .area;
    let measured_height = create_task_dialog_host()
        .measure(LayoutProposal::at_most(dialog_area.width, area.height))
        .preferred
        .height;

    assert_eq!(dialog_area.width, 80);
    assert_eq!(dialog_area.height, measured_height);
}

#[test]
fn create_task_submission_formats_title_and_uses_quick_capture_defaults() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);

    app.submit_create_task(
        CreateTaskDraft {
            title: "  fix   dont crash... ".to_string(),
        },
        &mut EventCtx::default(),
    );

    let state = store.borrow();
    let task = state.state().tasks.first().expect("task should be created");
    assert_eq!(task.title, "Fix don't crash");
    assert_eq!(task.description, "");
    assert_eq!(task.size, TaskSize::Small);
}

#[test]
fn creation_dialogs_close_from_nested_control_focus_mode() {
    let area = Rect::new(0, 0, 80, 24);
    let cases = [
        (
            create_management_dialog_host(ManagementDialogKind::Projects),
            KeyEvent::from(Key::Esc),
            true,
            "textarea",
            1,
        ),
        (
            create_task_dialog_host(),
            KeyEvent {
                code: Key::Char('['),
                modifiers: KeyModifiers::CONTROL,
            },
            false,
            "input",
            2,
        ),
    ];

    for (mut dialog, key, management, focus_id, presses) in cases {
        let mut layout = LayoutCtx::new();
        dialog.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.id.as_str() == focus_id)
            .expect("creation input should be focusable")
            .clone();
        let mut ctx = EventCtx::default();

        for _ in 0..presses {
            ctx = EventCtx::default();
            dialog.dispatch_event(
                &EventRoute::new(target.path.clone()),
                &TuiEvent::Key(key),
                &mut ctx,
            );
        }
        if management {
            assert!(matches!(ctx.messages(), [AppMsg::CloseManagementOverlay]));
        } else {
            assert!(matches!(ctx.messages(), [AppMsg::CloseDialog]));
        }
    }
}

#[test]
fn management_dialogs_close_from_nested_control_focus_mode() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: vec![Person::new("person-1".into(), "Ada".into(), String::new())],
        projects: vec![Project::new(
            "project-1".into(),
            "CORE".into(),
            "Core".into(),
            String::new(),
        )],
        tags: vec![Tag::new("tag-1".into(), "api".into())],
    });
    let area = Rect::new(0, 0, 100, 30);
    let cases = [
        (ManagementDialogKind::People, KeyEvent::from(Key::Esc)),
        (
            ManagementDialogKind::Projects,
            KeyEvent {
                code: Key::Char('['),
                modifiers: KeyModifiers::CONTROL,
            },
        ),
        (ManagementDialogKind::Tags, KeyEvent::from(Key::Esc)),
    ];

    for (kind, key) in cases {
        let mut dialog = management_dialog(context.clone(), kind);
        let mut layout = LayoutCtx::new();
        dialog.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.id.as_str() == "input")
            .expect("management detail input should be focusable")
            .clone();
        let mut ctx = EventCtx::default();

        let outcome =
            dialog.dispatch_event(&EventRoute::new(target.path), &TuiEvent::Key(key), &mut ctx);

        assert!(outcome.handled(), "{kind:?} should close");
        assert!(matches!(ctx.messages(), [AppMsg::CloseDialog]));
    }
}

#[test]
fn created_task_state_hotkey_focuses_open_dropdown() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let area = Rect::new(0, 0, 120, 40);
    workspace.layout(area, &mut LayoutCtx::new());
    store
        .borrow_mut()
        .dispatch(AppEvent::TaskCreated(Task::quick_capture(
            "task-2".to_string(),
            "Captured".to_string(),
            String::new(),
            TaskSize::Small,
        )));
    let mut layout = LayoutCtx::new();
    workspace.layout(area, &mut layout);
    let task_state = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "field"
                && target.path.keys().iter().any(|key| key.as_str() == "state")
        })
        .expect("task state should be focusable")
        .clone();
    let mut dispatcher = TreeDispatcher::new();

    let effects = dispatcher.dispatch_event(
        &mut workspace,
        &EventRoute::new(task_state.path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_STATE_FIELD.hotkey())),
        AnimationSettings::default(),
    );

    assert!(effects.layout);
    let focus_request = effects
        .focus_request
        .as_ref()
        .expect("state hotkey should request dropdown search focus");
    assert!(matches!(
        focus_request,
        FocusRequest::TargetAt { id, .. } if id.as_str() == "input"
    ));
    let mut open_layout = LayoutCtx::new();
    workspace.layout(area, &mut open_layout);
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(focus_request, open_layout.focus_targets())
        .expect("open dropdown search should accept focus");
    dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());

    assert_eq!(
        focus.current().map(|target| target.id.as_str()),
        Some("input")
    );
    assert!(rendered_text(&workspace, area).contains("Search..."));
}

#[test]
fn open_size_dropdown_consumes_b_without_snoozing_task() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let area = Rect::new(0, 0, 120, 40);
    let mut layout = LayoutCtx::new();
    workspace.layout(area, &mut layout);
    let task_size = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "field"
                && target.path.keys().iter().any(|key| key.as_str() == "size")
        })
        .expect("task size should be focusable")
        .clone();
    let mut dispatcher = TreeDispatcher::new();
    let open = dispatcher.dispatch_event(
        &mut workspace,
        &EventRoute::new(task_size.path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_SIZE_FIELD.hotkey())),
        AnimationSettings::default(),
    );
    let focus_request = open
        .focus_request
        .as_ref()
        .expect("size hotkey should request dropdown search focus");
    let mut open_layout = LayoutCtx::new();
    workspace.layout(area, &mut open_layout);
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(focus_request, open_layout.focus_targets())
        .expect("open dropdown search should accept focus");
    dispatcher.dispatch_focus(
        &mut workspace,
        transition.clone(),
        AnimationSettings::default(),
    );
    let focused = transition
        .current
        .expect("dropdown search should become focused");

    let effects = dispatcher.dispatch_event(
        &mut workspace,
        &EventRoute::new(focused.path),
        &TuiEvent::Key(KeyEvent::from(Key::Char('b'))),
        AnimationSettings::default(),
    );

    assert!(effects.outcome.handled());
    assert!(effects.messages.is_empty());
}

#[test]
fn task_state_switcher_excludes_snoozed() {
    let choice_ids = state_choices()
        .into_iter()
        .map(|choice| choice.id)
        .collect::<Vec<_>>();

    assert_eq!(
        choice_ids,
        ["backlog", "todo", "in_progress", "done", "rejected"]
    );
}

#[test]
fn task_description_registers_edit_and_editor_hotkeys_and_requests_existing_value() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let area = Rect::new(0, 0, 120, 40);
    let mut layout = LayoutCtx::new();
    workspace.layout(area, &mut layout);
    let description = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "textarea"
                && target
                    .path
                    .keys()
                    .iter()
                    .any(|key| key.as_str() == "description")
        })
        .expect("description input should be focusable");

    assert_eq!(description.hotkey_sequences, ["dd", "do"]);

    let effects = TreeDispatcher::new().dispatch_event(
        &mut workspace,
        &EventRoute::new(description.path.clone()),
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_DESCRIPTION_EDITOR.hotkey())),
        AnimationSettings::default(),
    );

    assert_eq!(
        effects
            .external_editor
            .expect("editor hotkey should request external editor")
            .value,
        "Existing detail"
    );
}

#[test]
fn title_blur_during_description_hotkey_preserves_description_focus() {
    sqlx::any::install_default_drivers();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");
    let _runtime_guard = runtime.enter();
    let pool = AnyPoolOptions::new()
        .connect_lazy("sqlite::memory:")
        .expect("lazy pool should build");
    let store = Rc::new(RefCell::new(Store::new(
        AppState::from_snapshot(WorkspaceSnapshot {
            tasks: vec![Task {
                id: "task-1".to_string(),
                rank: 1,
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
            }],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        }),
        reduce_app_state as fn(&mut AppState, AppEvent) -> tuicore::DispatchOutcome,
    )));
    let coordinator = Rc::new(RefCell::new(PersistenceCoordinator::new(
        Rc::clone(&store),
        pool,
        crate::storage::SqlDialect::Sqlite,
        runtime.handle().clone(),
        None,
    )));
    let mut workspace = TaskWorkspace::new(AppContext {
        store: Rc::clone(&store),
        coordinator,
    });
    let area = Rect::new(0, 0, 120, 40);
    let mut layout = LayoutCtx::new();
    workspace.layout(area, &mut layout);
    let title = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "input"
                && target.path.keys().iter().any(|key| key.as_str() == "title")
        })
        .expect("title input should be focusable")
        .clone();
    let description = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "textarea"
                && target
                    .path
                    .keys()
                    .iter()
                    .any(|key| key.as_str() == "description")
        })
        .expect("description input should be focusable")
        .clone();
    let mut focus = FocusManager::new();
    let mut dispatcher = TreeDispatcher::new();
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: title.path.clone(),
                id: title.id.clone(),
            },
            layout.focus_targets(),
        )
        .expect("title focus should change");
    dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());

    let title_route = EventRoute::new(title.path);
    for key in [Key::Enter, Key::Char('!'), Key::Esc] {
        let effects = dispatcher.dispatch_event(
            &mut workspace,
            &title_route,
            &TuiEvent::Key(key.into()),
            AnimationSettings::default(),
        );
        assert_eq!(effects.outcome, EventOutcome::Handled);
    }

    let description_route = EventRoute::new(description.path.clone());
    let hotkey_effects = dispatcher.dispatch_event(
        &mut workspace,
        &description_route,
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_DESCRIPTION_FIELD.hotkey())),
        AnimationSettings::default(),
    );
    assert!(hotkey_effects.layout);

    let mut first_transition_layout = LayoutCtx::new();
    workspace.layout(area, &mut first_transition_layout);
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: description.path.clone(),
                id: description.id.clone(),
            },
            first_transition_layout.focus_targets(),
        )
        .expect("description focus should change");
    let focus_effects =
        dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
    assert!(focus_effects.layout);

    let mut post_transition_layout = LayoutCtx::new();
    workspace.layout(area, &mut post_transition_layout);
    assert!(
        focus
            .validate(post_transition_layout.focus_targets())
            .is_none()
    );

    let store_ref = store.borrow();
    assert_eq!(store_ref.state().tasks[0].title, "Original!");
    drop(store_ref);

    let printable_effects = dispatcher.dispatch_event(
        &mut workspace,
        &EventRoute::new(focus.current_path()),
        &TuiEvent::Key(Key::Char('x').into()),
        AnimationSettings::default(),
    );
    assert_eq!(printable_effects.outcome, EventOutcome::Handled);

    let mut terminal =
        Terminal::new(TestBackend::new(area.width, area.height)).expect("terminal should build");
    terminal
        .draw(|frame| workspace.render(frame, area, &mut RenderCtx::new()))
        .expect("workspace should render");
    let buffer = terminal.backend().buffer();
    let mut rendered_table = String::new();
    for y in 0..area.height {
        for x in 0..70 {
            rendered_table.push_str(
                buffer
                    .cell((x, y))
                    .expect("table cell should exist")
                    .symbol(),
            );
        }
    }
    assert!(rendered_table.contains("Original!"));
}

#[test]
fn save_failure_and_recovery_preserve_focused_task_description_state() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let area = Rect::new(0, 0, 120, 40);
    let mut layout = LayoutCtx::new();
    workspace.layout(area, &mut layout);
    let description = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "textarea"
                && target
                    .path
                    .keys()
                    .iter()
                    .any(|key| key.as_str() == "description")
        })
        .expect("description should be focusable")
        .clone();
    let mut focus = FocusManager::new();
    let mut dispatcher = TreeDispatcher::new();
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: description.path.clone(),
                id: description.id.clone(),
            },
            layout.focus_targets(),
        )
        .expect("description focus should change");
    dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
    for key in [Key::Enter, Key::Char('x')] {
        assert_eq!(
            dispatcher
                .dispatch_event(
                    &mut workspace,
                    &EventRoute::new(focus.current_path()),
                    &TuiEvent::Key(key.into()),
                    AnimationSettings::default(),
                )
                .outcome,
            EventOutcome::Handled
        );
    }
    assert!(rendered_area_has_focus_style(
        &workspace,
        area,
        description.area
    ));

    store.borrow_mut().dispatch(AppEvent::SaveCompleted {
        target: SaveTarget::task("task-1".to_string(), TaskField::Description),
        error: Some("offline".to_string()),
    });
    let mut failed_layout = LayoutCtx::new();
    workspace.layout(area, &mut failed_layout);
    assert!(focus.validate(failed_layout.focus_targets()).is_none());
    assert!(rendered_text(&workspace, area).contains("Save failed for task-1"));
    assert!(rendered_area_has_focus_style(
        &workspace,
        area,
        description.area
    ));

    let after_failure = dispatcher.dispatch_event(
        &mut workspace,
        &EventRoute::new(focus.current_path()),
        &TuiEvent::Key(Key::Char('y').into()),
        AnimationSettings::default(),
    );
    assert_eq!(after_failure.outcome, EventOutcome::Handled);

    store.borrow_mut().dispatch(AppEvent::SaveCompleted {
        target: SaveTarget::task("task-1".to_string(), TaskField::Description),
        error: None,
    });
    let mut recovered_layout = LayoutCtx::new();
    workspace.layout(area, &mut recovered_layout);
    assert!(focus.validate(recovered_layout.focus_targets()).is_none());
    assert!(rendered_area_has_focus_style(
        &workspace,
        area,
        description.area
    ));

    let tab = dispatcher.dispatch_event(
        &mut workspace,
        &EventRoute::new(focus.current_path()),
        &TuiEvent::Key(Key::Tab.into()),
        AnimationSettings::default(),
    );
    let transition = focus
        .apply_request(
            tab.focus_request.as_ref().unwrap_or(&FocusRequest::Next),
            recovered_layout.focus_targets(),
        )
        .expect("tab should move focus");
    dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
    let back_tab = dispatcher.dispatch_event(
        &mut workspace,
        &EventRoute::new(focus.current_path()),
        &TuiEvent::Key(Key::BackTab.into()),
        AnimationSettings::default(),
    );
    let transition = focus
        .apply_request(
            back_tab
                .focus_request
                .as_ref()
                .unwrap_or(&FocusRequest::Previous),
            recovered_layout.focus_targets(),
        )
        .expect("shift-tab should restore description focus");
    dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
    assert_eq!(
        focus
            .current()
            .expect("focus should remain set")
            .id
            .as_str(),
        "textarea"
    );
    for key in [Key::Enter, Key::Char('z')] {
        assert_eq!(
            dispatcher
                .dispatch_event(
                    &mut workspace,
                    &EventRoute::new(focus.current_path()),
                    &TuiEvent::Key(key.into()),
                    AnimationSettings::default(),
                )
                .outcome,
            EventOutcome::Handled
        );
    }
}

#[test]
fn task_dropdown_save_completion_tabs_to_next_control_without_reset() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let area = Rect::new(0, 0, 120, 40);
    let mut layout = LayoutCtx::new();
    workspace.layout(area, &mut layout);
    let task_size = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "field"
                && target.path.keys().iter().any(|key| key.as_str() == "size")
        })
        .expect("task size should be focusable")
        .clone();
    let mut focus = FocusManager::new();
    let mut dispatcher = TreeDispatcher::new();
    let transition = focus
        .apply_request(
            &FocusRequest::TargetAt {
                path: task_size.path.clone(),
                id: task_size.id.clone(),
            },
            layout.focus_targets(),
        )
        .expect("size focus should change");
    dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
    workspace
        .detail_mut()
        .patches
        .borrow_mut()
        .push(TaskPatch::Size(TaskSize::Big));
    assert!(workspace.sync_detail_changes().changed);
    assert_eq!(store.borrow().state().tasks[0].size, TaskSize::Big);

    store.borrow_mut().dispatch(AppEvent::SaveCompleted {
        target: SaveTarget::task("task-1".to_string(), TaskField::Size),
        error: Some("offline".to_string()),
    });
    let mut post_save_layout = LayoutCtx::new();
    workspace.layout(area, &mut post_save_layout);
    assert!(focus.validate(post_save_layout.focus_targets()).is_none());

    let tab = dispatcher.dispatch_event(
        &mut workspace,
        &EventRoute::new(focus.current_path()),
        &TuiEvent::Key(Key::Tab.into()),
        AnimationSettings::default(),
    );
    let transition = focus
        .apply_request(
            tab.focus_request.as_ref().unwrap_or(&FocusRequest::Next),
            post_save_layout.focus_targets(),
        )
        .expect("tab should move to start date");
    dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
    assert!(
        focus
            .current()
            .expect("next control should be focused")
            .path
            .keys()
            .iter()
            .any(|key| key.as_str() == "start-date")
    );
    assert_eq!(store.borrow().state().tasks[0].size, TaskSize::Big);
}

#[test]
fn management_routing_builds_concrete_dialog_variant() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        projects: Vec::new(),
        tags: Vec::new(),
    });

    assert!(matches!(
        management_dialog(context, ManagementDialogKind::Projects),
        AppDialog::Projects(_)
    ));
}
