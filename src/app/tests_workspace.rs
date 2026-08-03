use super::*;

#[test]
fn selecting_workspace_from_detail_dropdown_refreshes_title_panel_identifier() {
    let workspace = Workspace::new(
        "workspace-1".into(),
        "APP".into(),
        "Application".into(),
        String::new(),
    );
    let task = task_with("OLD-42", "Original", TaskState::Todo);
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![task],
        people: Vec::new(),
        workspaces: vec![workspace],
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let area = Rect::new(0, 0, 120, 40);
    let mut layout = LayoutCtx::new();
    workspace.layout(area, &mut layout);
    let workspace_field = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "field"
                && target
                    .path
                    .keys()
                    .iter()
                    .any(|key| key.as_str() == "workspaces")
        })
        .expect("task workspace should be focusable")
        .clone();
    let mut dispatcher = TreeDispatcher::new();
    let open = dispatcher.dispatch_event(
        &mut workspace,
        &EventRoute::new(workspace_field.path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_WORKSPACES_FIELD.hotkey())),
        AnimationSettings::default(),
    );
    let focus_request = open
        .focus_request
        .as_ref()
        .expect("workspace hotkey should request dropdown search focus");
    let mut open_layout = LayoutCtx::new();
    workspace.layout(area, &mut open_layout);
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(focus_request, open_layout.focus_targets())
        .expect("workspace dropdown search should accept focus");
    let focused = transition.current.clone().unwrap();
    dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());

    dispatcher.dispatch_event(
        &mut workspace,
        &EventRoute::new(focused.path.clone()),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('j'),
            modifiers: KeyModifiers::CONTROL,
        }),
        AnimationSettings::default(),
    );
    dispatcher.dispatch_event(
        &mut workspace,
        &EventRoute::new(focused.path),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        AnimationSettings::default(),
    );
    workspace.layout(area, &mut LayoutCtx::new());

    let detail = rendered_text(workspace.detail(), area);
    assert!(detail.contains("APP-42"), "rendered detail: {detail:?}");

    let mut selected_layout = LayoutCtx::new();
    workspace.layout(area, &mut selected_layout);
    let workspace_field = selected_layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "field"
                && target
                    .path
                    .keys()
                    .iter()
                    .any(|key| key.as_str() == "workspaces")
        })
        .unwrap()
        .clone();
    let open = dispatcher.dispatch_event(
        &mut workspace,
        &EventRoute::new(workspace_field.path),
        &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_WORKSPACES_FIELD.hotkey())),
        AnimationSettings::default(),
    );
    let mut open_layout = LayoutCtx::new();
    workspace.layout(area, &mut open_layout);
    let mut focus = FocusManager::new();
    let transition = focus
        .apply_request(
            open.focus_request.as_ref().unwrap(),
            open_layout.focus_targets(),
        )
        .unwrap();
    let focused = transition.current.clone().unwrap();
    dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
    dispatcher.dispatch_event(
        &mut workspace,
        &EventRoute::new(focused.path.clone()),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('k'),
            modifiers: KeyModifiers::CONTROL,
        }),
        AnimationSettings::default(),
    );
    dispatcher.dispatch_event(
        &mut workspace,
        &EventRoute::new(focused.path),
        &TuiEvent::Key(KeyEvent::from(Key::Enter)),
        AnimationSettings::default(),
    );
    workspace.layout(area, &mut LayoutCtx::new());

    let detail = rendered_text(workspace.detail(), area);
    assert!(!detail.contains("APP-42"), "rendered detail: {detail:?}");
}

#[test]
fn task_table_ignores_data_view_filter_mode_hotkey() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
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
fn empty_task_table_centers_seasonal_message_and_ornament() {
    let table = task_table_with_copy_context_on(
        Vec::new(),
        None,
        TaskCopyContext::default(),
        time::macros::date!(2026 - 12 - 01),
    );
    let area = Rect::new(0, 0, 40, 9);
    let mut terminal =
        Terminal::new(TestBackend::new(area.width, area.height)).expect("terminal should build");
    terminal
        .draw(|frame| table.render(frame, area, &mut RenderCtx::new()))
        .expect("task table should render");
    let buffer = terminal.backend().buffer();
    let line = |y| {
        (0..area.width)
            .map(|x| buffer.cell((x, y)).unwrap().symbol())
            .collect::<String>()
    };

    assert_eq!(line(3), "       No tasks match your filters      ");
    assert!(line(4).trim().is_empty());
    assert_eq!(line(5).trim(), "╶┄ ✧ ·  · ✧ ┄╴");
}

#[test]
fn empty_task_workspace_collapses_detail_and_gives_table_full_pane() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let area = Rect::new(0, 0, 120, 30);
    workspace.layout(area, &mut LayoutCtx::new());

    let text = rendered_text(&workspace, area);
    let (_, table_area) = workspace.layout.first().child_areas();
    let (_, detail_area) = workspace.layout.child_areas();
    assert_eq!(table_area, Rect::new(0, 1, 120, 29));
    assert_eq!(detail_area, Rect::default());
    assert!(text.contains("No active tasks"));
    assert!(!text.contains("No task selected."));
}

#[test]
fn task_views_explain_when_their_state_bucket_is_empty() {
    for (view, other_state, message) in [
        (TaskView::Active, TaskState::Backlog, "No active tasks"),
        (TaskView::Backlog, TaskState::Todo, "No tasks in backlog"),
        (TaskView::Snoozed, TaskState::Todo, "No snoozed tasks"),
        (TaskView::Archived, TaskState::Todo, "No archived tasks"),
        (TaskView::All, TaskState::Done, "No open tasks"),
    ] {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![task_with("other", "Other task", other_state)],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        if view != TaskView::Active {
            *workspace.pending_task_view.borrow_mut() = Some(view);
            assert!(workspace.sync_task_view_change());
        }
        let area = Rect::new(0, 0, 100, 30);
        workspace.layout(area, &mut LayoutCtx::new());

        let text = rendered_text(&workspace, area);

        assert!(text.contains(message), "missing {message}: {text}");
        assert!(!text.contains("No tasks match your filters"));
    }
}

#[test]
fn task_search_hides_detail_and_clearing_search_restores_it() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let mut ctx = EventCtx::default();

    workspace.table_mut().set_search_query("no match");
    workspace.sync_table_events(&mut ctx);
    assert_eq!(workspace.table().highlighted_id(), None);
    assert_eq!(workspace.visible_selection.borrow().as_deref(), None);
    assert_eq!(workspace.detail().task_id, None);
    assert!(!workspace.layout.is_second_visible());
    workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());
    assert!(
        rendered_text(&workspace, Rect::new(0, 0, 100, 30)).contains("No tasks match your filters")
    );

    workspace.table_mut().clear_search();
    workspace.sync_table_events(&mut ctx);
    assert_eq!(
        workspace.table().highlighted_id().as_deref(),
        Some("task-1")
    );
    assert_eq!(workspace.detail().task_id.as_deref(), Some("task-1"));
    assert!(workspace.layout.is_second_visible());
}

#[test]
fn task_labels_with_no_matches_use_filtered_empty_message() {
    let mut task = task_with("task-1", "API task", TaskState::Todo);
    task.tag_ids = vec!["api".into()];
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![task],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: vec![
            Tag::new("api".into(), "API".into()),
            Tag::new("urgent".into(), "Urgent".into()),
        ],
    });
    let mut workspace = TaskWorkspace::new(context);
    *workspace.active_label_filter.borrow_mut() = vec!["urgent".into()];

    assert!(workspace.sync_label_filter_change());
    let area = Rect::new(0, 0, 100, 30);
    workspace.layout(area, &mut LayoutCtx::new());

    assert!(rendered_text(&workspace, area).contains("No tasks match your filters"));
}

#[test]
fn hidden_task_cannot_be_deleted_from_empty_active_view() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![task_with("backlog", "Backlog work", TaskState::Backlog)],
        people: Vec::new(),
        workspaces: Vec::new(),
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
fn created_backlog_task_becomes_visible_and_selected_from_any_task_view() {
    for initial_view in [TaskView::Active, TaskView::Snoozed] {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        *workspace.pending_task_view.borrow_mut() = Some(initial_view);
        workspace.sync_task_view_change();
        workspace.layout(Rect::new(0, 0, 120, 40), &mut LayoutCtx::new());

        store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(Task::quick_capture(
                "task-2".to_string(),
                "Captured".to_string(),
                String::new(),
                TaskSize::Small,
            )));
        workspace.layout(Rect::new(0, 0, 120, 40), &mut LayoutCtx::new());

        assert_eq!(workspace.task_view, TaskView::Backlog);
        assert_eq!(*workspace.active_task_view.borrow(), TaskView::Backlog);
        assert_eq!(
            store.borrow().state().selected_task_id.as_deref(),
            Some("task-2")
        );
        assert_eq!(
            workspace.table().highlighted_id().as_deref(),
            Some("task-2")
        );
        assert_eq!(workspace.table().selected_id().as_deref(), Some("task-2"));
        assert_eq!(workspace.detail().task_id.as_deref(), Some("task-2"));
    }
}

#[test]
fn escape_keeps_task_table_focused_as_tab_root() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
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
fn focus_tracking_matches_master_detail_layout_paths() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let mut layout = LayoutCtx::new();
    workspace.layout(Rect::new(0, 0, 120, 40), &mut layout);
    let table = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.id.as_str() == "data-view"
                && target.path.keys().first() == Some(&ChildKey::first())
        })
        .unwrap()
        .clone();
    let detail = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target.path.keys().first() == Some(&ChildKey::second())
                && target.path.keys().iter().any(|key| key.as_str() == "title")
        })
        .unwrap()
        .clone();

    workspace.dispatch_focus(&table, true, &mut FocusCtx::default());
    assert!(workspace.table_focused);
    assert!(!workspace.detail_draft_protected);

    workspace.dispatch_focus(&detail, true, &mut FocusCtx::default());
    assert!(!workspace.table_focused);
    assert!(workspace.detail_draft_protected);
}

#[test]
fn delete_shortcuts_open_confirmation_from_focused_task_table() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.table_focused = true;
    for key in [
        KeyEvent::from(Key::Delete),
        KeyEvent::from(Key::Backspace),
        KeyEvent {
            code: Key::Char('x'),
            modifiers: KeyModifiers::CONTROL,
        },
    ] {
        let mut ctx = EventCtx::default();
        let outcome = workspace.event(&TuiEvent::Key(key), &mut ctx);

        assert!(outcome.handled());
        assert!(matches!(
            ctx.messages(),
            [AppMsg::OpenDeleteTask { task_id, return_focus: None }] if task_id == "task-1"
        ));
    }
}

#[test]
fn ctrl_c_opens_complete_dialog_from_focused_task_table() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.table_focused = true;
    let mut ctx = EventCtx::default();

    let outcome = workspace.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('c'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut ctx,
    );

    assert!(outcome.handled());
    assert!(matches!(
        ctx.messages(),
        [AppMsg::OpenCompleteTask { task_id, return_focus: None }] if task_id == "task-1"
    ));
}

#[test]
fn ctrl_t_requests_direct_progress_transition_for_every_task_state() {
    for (state, view) in [
        (TaskState::Backlog, TaskView::Backlog),
        (TaskState::Todo, TaskView::Active),
        (TaskState::InProgress, TaskView::Active),
        (TaskState::Done, TaskView::Archived),
        (TaskState::Snoozed, TaskView::Snoozed),
        (TaskState::Rejected, TaskView::Archived),
    ] {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![task_with("task-1", "Shortcut task", state)],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        if view != TaskView::Active {
            *workspace.pending_task_view.borrow_mut() = Some(view);
            assert!(workspace.sync_task_view_change());
        }
        workspace.table_focused = true;
        let mut ctx = EventCtx::default();

        let outcome = workspace.event(
            &TuiEvent::Key(KeyEvent {
                code: Key::Char('t'),
                modifiers: KeyModifiers::CONTROL,
            }),
            &mut ctx,
        );

        assert!(outcome.handled(), "{state:?} should transition");
        assert!(matches!(
            ctx.messages(),
            [AppMsg::ToggleTaskProgress(task_id)] if task_id == "task-1"
        ));
        assert_eq!(store.borrow().state().tasks[0].state, state);
        assert!(ctx.notifications().is_empty());
    }
}

#[test]
fn direct_progress_transition_updates_state_persists_and_notifies() {
    for (from, to, label) in [
        (TaskState::Backlog, TaskState::Todo, "todo"),
        (TaskState::Todo, TaskState::InProgress, "in-progress"),
        (TaskState::InProgress, TaskState::Todo, "todo"),
        (TaskState::Done, TaskState::Todo, "todo"),
        (TaskState::Snoozed, TaskState::Todo, "todo"),
        (TaskState::Rejected, TaskState::Todo, "todo"),
    ] {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![task_with("task-1", "Shortcut task", from)],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let coordinator = Rc::clone(&context.coordinator);
        let mut app = App::new(context.store, context.coordinator);
        let mut ctx = EventCtx::default();

        app.toggle_task_progress("task-1".into(), &mut ctx);

        assert_eq!(store.borrow().state().tasks[0].state, to);
        assert!(coordinator.borrow().has_pending());
        assert!(ctx.layout_requested());
        assert_eq!(
            ctx.notifications(),
            &[tuicore::Notification::success(
                "Task moved",
                format!("“Shortcut task” moved to {label}.")
            )]
        );
    }
}

#[test]
fn ctrl_t_is_inert_without_a_highlighted_task() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.table_focused = true;
    let mut ctx = EventCtx::default();

    let outcome = workspace.event(
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('t'),
            modifiers: KeyModifiers::CONTROL,
        }),
        &mut ctx,
    );

    assert_eq!(outcome, EventOutcome::Ignored);
    assert!(store.borrow().state().tasks.is_empty());
    assert!(ctx.messages().is_empty());
    assert!(ctx.notifications().is_empty());
}

#[test]
fn ctrl_c_from_task_detail_preserves_full_path_and_child_ownership() {
    let complete = TuiEvent::Key(KeyEvent {
        code: Key::Char('c'),
        modifiers: KeyModifiers::CONTROL,
    });
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let mut layout = LayoutCtx::new();
    workspace.layout(Rect::new(0, 0, 120, 80), &mut layout);
    let title = layout
        .focus_targets()
        .iter()
        .find(|target| target.path.keys().iter().any(|key| key.as_str() == "title"))
        .expect("title should be focusable");
    let route = EventRoute::new(title.path.clone());

    let effects = TreeDispatcher::new().dispatch_event(
        &mut workspace,
        &route,
        &complete,
        AnimationSettings::default(),
    );

    assert!(effects.outcome.handled());
    assert!(matches!(
        effects.messages.as_slice(),
        [AppMsg::OpenCompleteTask {
            task_id,
            return_focus: Some(return_focus),
        }] if task_id == "task-1" && return_focus == &title.path
    ));

    for child_outcome in [EventOutcome::Handled, EventOutcome::Ignored] {
        let mut ctx = EventCtx::default();
        if !child_outcome.handled() {
            ctx.stop_propagation();
        }
        assert_eq!(
            workspace.handle_detail_complete_shortcut(child_outcome, &route, &complete, &mut ctx,),
            None
        );
        assert!(ctx.messages().is_empty());
    }
}

#[test]
fn ctrl_x_from_task_detail_opens_delete_except_under_links() {
    let ctrl_x = TuiEvent::Key(KeyEvent {
        code: Key::Char('x'),
        modifiers: KeyModifiers::CONTROL,
    });
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let mut layout = LayoutCtx::new();
    workspace.layout(Rect::new(0, 0, 120, 80), &mut layout);
    let title = layout
        .focus_targets()
        .iter()
        .find(|target| target.path.keys().iter().any(|key| key.as_str() == "title"))
        .expect("title should be focusable");

    let effects = TreeDispatcher::new().dispatch_event(
        &mut workspace,
        &EventRoute::new(title.path.clone()),
        &ctrl_x,
        AnimationSettings::default(),
    );

    assert!(effects.outcome.handled());
    assert!(matches!(
        effects.messages.as_slice(),
        [AppMsg::OpenDeleteTask {
            task_id,
            return_focus: Some(return_focus),
        }] if task_id == "task-1" && return_focus == &title.path
    ));

    let title_route = EventRoute::new(title.path.clone());
    let mut handled_ctx = EventCtx::default();
    assert_eq!(
        workspace.handle_detail_delete_shortcut(
            EventOutcome::Handled,
            &title_route,
            &ctrl_x,
            &mut handled_ctx,
        ),
        None
    );
    assert!(handled_ctx.messages().is_empty());

    let mut stopped_ctx = EventCtx::default();
    stopped_ctx.stop_propagation();
    assert_eq!(
        workspace.handle_detail_delete_shortcut(
            EventOutcome::Ignored,
            &title_route,
            &ctrl_x,
            &mut stopped_ctx,
        ),
        None
    );
    assert!(stopped_ctx.messages().is_empty());

    for links in [Vec::new(), vec!["https://example.com".to_string()]] {
        let populated = !links.is_empty();
        let mut task = test_task();
        task.links = links;
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![task],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let mut layout = LayoutCtx::new();
        workspace.layout(Rect::new(0, 0, 120, 80), &mut layout);
        let links = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target.id.as_str() == "data-view"
                    && target.path.keys().iter().any(|key| key.as_str() == "links")
            })
            .expect("links should be focusable");

        let effects = TreeDispatcher::new().dispatch_event(
            &mut workspace,
            &EventRoute::new(links.path.clone()),
            &ctrl_x,
            AnimationSettings::default(),
        );

        if populated {
            assert!(effects.outcome.handled());
        }
        assert!(
            !effects
                .messages
                .iter()
                .any(|message| matches!(message, AppMsg::OpenDeleteTask { .. }))
        );
    }

    for editor_key in [Key::Char('+'), Key::Char('e')] {
        let mut task = test_task();
        task.links = vec!["https://example.com".to_string()];
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![task],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let area = Rect::new(0, 0, 120, 80);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let links = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target.id.as_str() == "data-view"
                    && target.path.keys().iter().any(|key| key.as_str() == "links")
            })
            .expect("links should be focusable")
            .path
            .clone();
        TreeDispatcher::new().dispatch_event(
            &mut workspace,
            &EventRoute::new(links),
            &TuiEvent::Key(editor_key.into()),
            AnimationSettings::default(),
        );
        let mut editor_layout = LayoutCtx::new();
        workspace.layout(area, &mut editor_layout);
        let editor = editor_layout
            .focus_targets()
            .iter()
            .find(|target| {
                target.path.keys().iter().any(|key| key.as_str() == "links")
                    && target
                        .path
                        .keys()
                        .iter()
                        .any(|key| key.as_str() == "add-input")
            })
            .expect("link editor should be focusable");

        let effects = TreeDispatcher::new().dispatch_event(
            &mut workspace,
            &EventRoute::new(editor.path.clone()),
            &ctrl_x,
            AnimationSettings::default(),
        );

        assert!(
            !effects
                .messages
                .iter()
                .any(|message| matches!(message, AppMsg::OpenDeleteTask { .. }))
        );
    }
}

#[test]
fn first_escape_cancels_checklist_and_link_insert_before_leaving_detail() {
    let cancel_keys = [
        KeyEvent::from(Key::Esc),
        KeyEvent {
            code: Key::Char('['),
            modifiers: KeyModifiers::CONTROL,
        },
    ];

    for component in ["checklist", "links"] {
        for cancel_key in cancel_keys {
            let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
                tasks: vec![test_task()],
                people: Vec::new(),
                workspaces: Vec::new(),
                tags: Vec::new(),
            });
            let mut workspace = TaskWorkspace::new(context);
            let area = Rect::new(0, 0, 120, 80);
            let mut layout = LayoutCtx::new();
            workspace.layout(area, &mut layout);
            let data_path = layout
                .focus_targets()
                .iter()
                .find(|target| {
                    target.id.as_str() == "data-view"
                        && target
                            .path
                            .keys()
                            .iter()
                            .any(|key| key.as_str() == component)
                })
                .expect("list control should be focusable")
                .path
                .clone();
            workspace.dispatch_event(
                &EventRoute::new(data_path.clone()),
                &TuiEvent::Key(Key::Char('+').into()),
                &mut EventCtx::default(),
            );
            let mut editor_layout = LayoutCtx::new();
            workspace.layout(area, &mut editor_layout);
            let editor_path = editor_layout
                .focus_targets()
                .iter()
                .find(|target| {
                    target
                        .path
                        .keys()
                        .iter()
                        .any(|key| key.as_str() == component)
                        && target
                            .path
                            .keys()
                            .iter()
                            .any(|key| key.as_str() == "add-input")
                })
                .expect("insert editor should be focusable")
                .path
                .clone();

            let mut first_ctx = EventCtx::default();
            let first = workspace.dispatch_event(
                &EventRoute::new(editor_path),
                &TuiEvent::Key(cancel_key),
                &mut first_ctx,
            );
            assert!(first.handled());
            assert_ne!(
                first_ctx.focus_request(),
                Some(&initial_task_table_focus_request())
            );

            let mut second_ctx = EventCtx::default();
            let second = workspace.dispatch_event(
                &EventRoute::new(data_path),
                &TuiEvent::Key(cancel_key),
                &mut second_ctx,
            );
            assert!(second.handled());
            assert_eq!(
                second_ctx.focus_request(),
                Some(&initial_task_table_focus_request())
            );
        }
    }
}

#[test]
fn detail_dialog_cancel_restores_resolvable_full_app_focus_path() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let area = Rect::new(0, 0, 120, 80);
    let mut layout = LayoutCtx::new();
    app.layout(area, &mut layout);
    let description_path = layout
        .focus_targets()
        .iter()
        .find(|target| {
            target
                .path
                .keys()
                .iter()
                .any(|key| key.as_str() == "description")
        })
        .expect("description should be focusable")
        .path
        .clone();

    let effects = TreeDispatcher::new().dispatch_event(
        &mut app,
        &EventRoute::new(description_path.clone()),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('z'),
            modifiers: KeyModifiers::CONTROL,
        }),
        AnimationSettings::default(),
    );
    let [
        AppMsg::OpenTaskSnooze {
            task_id,
            return_focus: Some(return_focus),
        },
    ] = effects.messages.as_slice()
    else {
        panic!("detail snooze should carry return focus");
    };
    assert_eq!(task_id, "task-1");
    assert_eq!(return_focus, &description_path);

    app.open_task_snooze_dialog(
        task_id,
        Some(return_focus.clone()),
        &mut EventCtx::default(),
    );
    let mut close_ctx = EventCtx::default();
    app.close_snooze_dialog(&mut close_ctx);
    let mut post_close_layout = LayoutCtx::new();
    app.layout(area, &mut post_close_layout);
    let transition = FocusManager::new()
        .apply_request(
            close_ctx
                .focus_request()
                .expect("snooze close should request focus"),
            post_close_layout.focus_targets(),
        )
        .expect("restored detail focus should resolve");
    assert_eq!(
        transition.current.expect("detail should regain focus").path,
        description_path
    );
    assert_eq!(app.snooze_return_focus, None);

    let delete_effects = TreeDispatcher::new().dispatch_event(
        &mut app,
        &EventRoute::new(description_path.clone()),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('x'),
            modifiers: KeyModifiers::CONTROL,
        }),
        AnimationSettings::default(),
    );
    let [
        AppMsg::OpenDeleteTask {
            task_id,
            return_focus: Some(return_focus),
        },
    ] = delete_effects.messages.as_slice()
    else {
        panic!("detail delete should carry return focus");
    };
    assert_eq!(task_id, "task-1");
    assert_eq!(return_focus, &description_path);

    app.open_delete_task_dialog(
        task_id,
        Some(return_focus.clone()),
        &mut EventCtx::default(),
    );
    let mut delete_close_ctx = EventCtx::default();
    app.close_delete_task_dialog(&mut delete_close_ctx);
    let mut delete_close_layout = LayoutCtx::new();
    app.layout(area, &mut delete_close_layout);
    let delete_transition = FocusManager::new()
        .apply_request(
            delete_close_ctx
                .focus_request()
                .expect("delete close should request focus"),
            delete_close_layout.focus_targets(),
        )
        .expect("restored detail focus should resolve");
    assert_eq!(
        delete_transition
            .current
            .expect("detail should regain focus")
            .path,
        description_path
    );
    assert_eq!(app.delete_return_focus, None);

    app.open_complete_task_dialog(
        "task-1",
        Some(description_path.clone()),
        &mut EventCtx::default(),
    );
    let mut complete_close_ctx = EventCtx::default();
    app.close_complete_task_dialog(&mut complete_close_ctx);
    assert_eq!(
        complete_close_ctx.focus_request(),
        Some(&FocusRequest::Path(description_path))
    );
    assert_eq!(app.complete_return_focus, None);
}

#[test]
fn table_origin_dialog_cancel_focuses_task_table() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let mut snooze_ctx = EventCtx::default();
    app.close_snooze_dialog(&mut snooze_ctx);
    assert_eq!(
        snooze_ctx.focus_request(),
        Some(&initial_task_table_focus_request())
    );

    let mut dialog = delete_task_dialog(&test_task());
    let mut dialog_ctx = EventCtx::default();
    dialog.event(&TuiEvent::Key(Key::Esc.into()), &mut dialog_ctx);
    assert!(matches!(
        dialog_ctx.messages(),
        [AppMsg::CloseDeleteTaskDialog]
    ));

    app.open_delete_task_dialog("task-1", None, &mut EventCtx::default());
    let mut delete_ctx = EventCtx::default();
    app.close_delete_task_dialog(&mut delete_ctx);
    assert_eq!(
        delete_ctx.focus_request(),
        Some(&initial_task_table_focus_request())
    );

    app.open_complete_task_dialog("task-1", None, &mut EventCtx::default());
    let mut complete_ctx = EventCtx::default();
    app.close_complete_task_dialog(&mut complete_ctx);
    assert_eq!(
        complete_ctx.focus_request(),
        Some(&initial_task_table_focus_request())
    );
}

#[test]
fn calendar_origin_dialog_cancel_restores_calendar_focus() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            test_task(),
            task_with("task-2", "Calendar task", TaskState::Snoozed),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let calendar_path = TreePath::from_keys([
        ChildKey::new("tabs"),
        ChildKey::new("tab-1"),
        ChildKey::first(),
    ]);

    app.open_task_snooze_dialog(
        "task-2",
        Some(calendar_path.clone()),
        &mut EventCtx::default(),
    );
    let mut snooze_ctx = EventCtx::default();
    app.close_snooze_dialog(&mut snooze_ctx);
    assert_eq!(
        snooze_ctx.focus_request(),
        Some(&FocusRequest::Path(calendar_path.clone()))
    );

    app.open_delete_task_dialog(
        "task-2",
        Some(calendar_path.clone()),
        &mut EventCtx::default(),
    );
    let mut delete_ctx = EventCtx::default();
    app.close_delete_task_dialog(&mut delete_ctx);
    assert_eq!(
        delete_ctx.focus_request(),
        Some(&FocusRequest::Path(calendar_path.clone()))
    );

    app.open_complete_task_dialog(
        "task-2",
        Some(calendar_path.clone()),
        &mut EventCtx::default(),
    );
    let mut complete_ctx = EventCtx::default();
    app.close_complete_task_dialog(&mut complete_ctx);
    assert_eq!(
        complete_ctx.focus_request(),
        Some(&FocusRequest::Path(calendar_path))
    );
}

#[test]
fn missing_task_dialog_targets_clear_origin_and_focus_task_table() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    app.open_task_quick_menu("task-1", &mut EventCtx::default());
    app.snooze_return_focus = Some(TreePath::from_keys([ChildKey::new("stale")]));
    let mut ctx = EventCtx::default();

    app.open_task_snooze_dialog("missing", None, &mut ctx);

    assert_eq!(app.snooze_return_focus, None);
    assert!(!app.primary_dialog().is_active());
    assert_eq!(
        ctx.focus_request(),
        Some(&initial_task_table_focus_request())
    );

    app.open_task_quick_menu("task-1", &mut EventCtx::default());
    app.delete_return_focus = Some(TreePath::from_keys([ChildKey::new("stale")]));
    let mut delete_ctx = EventCtx::default();
    app.open_delete_task_dialog("missing", None, &mut delete_ctx);

    assert_eq!(app.delete_return_focus, None);
    assert!(!app.primary_dialog().is_active());
    assert_eq!(
        delete_ctx.focus_request(),
        Some(&initial_task_table_focus_request())
    );

    app.open_task_quick_menu("task-1", &mut EventCtx::default());
    app.complete_return_focus = Some(CompleteReturnFocus {
        task_id: "task-1".into(),
        task_state: TaskState::InProgress,
        task_selected_on_open: true,
        path: TreePath::from_keys([ChildKey::new("stale")]),
    });
    let mut complete_ctx = EventCtx::default();
    app.open_complete_task_dialog("missing", None, &mut complete_ctx);

    assert_eq!(app.complete_return_focus, None);
    assert!(!app.primary_dialog().is_active());
    assert_eq!(
        complete_ctx.focus_request(),
        Some(&initial_task_table_focus_request())
    );
}

#[test]
fn quick_menu_opens_with_visible_task_and_ctrl_z_snoozes_from_table() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    let snooze = TuiEvent::Key(KeyEvent {
        code: Key::Char('z'),
        modifiers: KeyModifiers::CONTROL,
    });
    let quick_menu = TuiEvent::Key(KeyEvent::from(Key::Char('.')));

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
        [AppMsg::OpenTaskSnooze { task_id, return_focus: None }] if task_id == "task-1"
    ));

    let mut plain_b_ctx = EventCtx::default();
    assert_eq!(
        workspace.event(&TuiEvent::Key(Key::Char('b').into()), &mut plain_b_ctx),
        EventOutcome::Ignored
    );
    assert!(plain_b_ctx.messages().is_empty());

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
        workspaces: Vec::new(),
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
fn task_reordering_and_move_to_edge_actions_notify() {
    let tasks = vec![
        task_with_rank("first", "First", TaskState::Todo, 1),
        task_with_rank("second", "Second", TaskState::Todo, 2),
    ];
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: tasks.clone(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut workspace = TaskWorkspace::new(context);
    workspace.table_mut().highlight_id(&"first".to_string());
    let mut list_ctx = EventCtx::default();
    for key in [
        KeyEvent {
            code: Key::Char('m'),
            modifiers: KeyModifiers::CONTROL,
        },
        KeyEvent::from(Key::Down),
        KeyEvent::from(Key::Enter),
    ] {
        workspace
            .task_list_mut()
            .event(&TuiEvent::Key(key), &mut list_ctx);
    }
    let mut reorder_ctx = EventCtx::default();

    workspace.sync_table_events(&mut reorder_ctx);

    assert_eq!(
        reorder_ctx.notifications(),
        &[tuicore::Notification::success(
            "Tasks reordered",
            "Task order was updated."
        )]
    );

    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks,
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let mut move_ctx = EventCtx::default();

    app.move_task_to_edge("second", true, &mut move_ctx);

    assert_eq!(
        move_ctx.notifications(),
        &[tuicore::Notification::success(
            "Task moved",
            "“Second” moved to the top."
        )]
    );
}

#[test]
fn calendar_move_to_edge_only_reorders_tasks_at_the_same_time() {
    let eight = time::macros::datetime!(2026-07-31 8:00);
    let nine = time::macros::datetime!(2026-07-31 9:00);
    let mut tasks = vec![
        task_with_rank("first", "First", TaskState::Snoozed, 1),
        task_with_rank("second", "Second", TaskState::Snoozed, 2),
        task_with_rank("third", "Third", TaskState::Snoozed, 3),
        task_with_rank("later", "Later", TaskState::Snoozed, 4),
    ];
    for task in &mut tasks[..3] {
        task.snoozed_until = Some(eight);
    }
    tasks[3].snoozed_until = Some(nine);
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks,
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);

    app.move_calendar_task_to_edge("second", eight, false, &mut EventCtx::default());

    let state = store.borrow();
    let rank = |id: &str| {
        state
            .state()
            .tasks
            .iter()
            .find(|task| task.id == id)
            .unwrap()
            .rank
    };
    assert_eq!(rank("first"), 1);
    assert_eq!(rank("second"), 3);
    assert_eq!(rank("third"), 2);
    assert_eq!(rank("later"), 4);
}

#[test]
fn successful_snooze_and_unsnooze_clear_return_focus_and_focus_task_table() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let mut open_ctx = EventCtx::default();
    let detail_path = TreePath::from_keys([ChildKey::new("detail"), ChildKey::new("title")]);
    app.open_task_snooze_dialog("task-1", Some(detail_path.clone()), &mut open_ctx);
    let custom = time::macros::datetime!(2026-07-30 14:30);
    let mut submit_ctx = EventCtx::default();

    app.snooze_task("task-1".into(), custom, Some(custom), &mut submit_ctx);

    assert_eq!(app.snooze_return_focus, None);
    assert_eq!(
        submit_ctx.focus_request(),
        Some(&initial_task_table_focus_request())
    );
    assert_eq!(
        store.borrow().state().selected_task_id.as_deref(),
        Some("task-1")
    );
    assert_eq!(
        submit_ctx.notifications(),
        &[tuicore::Notification::success(
            "Task snoozed",
            "“Original” snoozed until 2026-07-30T14:30:00."
        )]
    );

    app.snooze_return_focus = Some(detail_path);
    let mut unsnooze_ctx = EventCtx::default();
    app.unsnooze_task("task-1".into(), &mut unsnooze_ctx);

    assert_eq!(app.snooze_return_focus, None);
    assert_eq!(
        unsnooze_ctx.focus_request(),
        Some(&initial_task_table_focus_request())
    );
    assert_eq!(
        unsnooze_ctx.notifications(),
        &[tuicore::Notification::success(
            "Task unsnoozed",
            "“Original” moved to todo."
        )]
    );
}

#[test]
fn snooze_dialog_renders_search_and_options_through_modal_portals() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
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
fn snooze_dropdown_does_not_dim_dialog_backdrop_twice() {
    let mut dialog = DialogLayer::new(
        Flex::<AppMsg>::column(),
        SnoozeDialog::new(
            "task-1".into(),
            time::macros::datetime!(2026-07-23 12:00),
            None,
            false,
        ),
    )
    .fit_content()
    .backdrop(DialogBackdrop::dim().amount(0.5));
    let area = Rect::new(0, 0, 100, 30);
    let mut layout = LayoutCtx::new();
    dialog.layout(area, &mut layout);
    let modal = layout
        .overlays()
        .iter()
        .find(|overlay| overlay.layer == tuicore::OverlayLayer::Modal)
        .expect("snooze dialog should register a modal")
        .area;
    let popover = layout
        .overlays()
        .iter()
        .find(|overlay| overlay.layer == tuicore::OverlayLayer::Popover)
        .expect("snooze dropdown should register a popover");
    let contains =
        |rect: Rect, x, y| x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom();
    let outside_dialog = (0..area.height)
        .flat_map(|y| (0..area.width).map(move |x| (x, y)))
        .find(|&(x, y)| !contains(modal, x, y))
        .expect("canvas should include space outside dialog");
    let inside_dialog = (modal.y..modal.bottom())
        .flat_map(|y| (modal.x..modal.right()).map(move |x| (x, y)))
        .find(|&(x, y)| !contains(popover.anchor, x, y) && !contains(popover.area, x, y))
        .expect("dialog should include space outside dropdown");
    let mut terminal =
        Terminal::new(TestBackend::new(area.width, area.height)).expect("terminal should build");

    terminal
        .draw(|frame| {
            let mut ctx = RenderCtx::new();
            dialog.render(frame, area, &mut ctx);
            ctx.flush(frame);
        })
        .expect("dialog should render");

    let buffer = terminal.backend().buffer();
    let outside = buffer
        .cell(outside_dialog)
        .expect("outside dialog cell should exist");
    let inside = buffer
        .cell(inside_dialog)
        .expect("inside dialog cell should exist");
    assert!(outside.modifier.contains(Modifier::DIM));
    assert_eq!(inside.fg, outside.fg);
}

#[test]
fn snoozed_detail_queues_datetime_selection() {
    let until = time::macros::datetime!(2026-08-24 8:00);
    let mut task = test_task();
    task.state = TaskState::Snoozed;
    task.snoozed_until = Some(until);
    let mut detail = TaskDetailForm::new(
        Some(&task),
        std::slice::from_ref(&task),
        &[],
        &[],
        &[],
        None,
    );
    let area = Rect::new(0, 0, 80, 120);
    detail.layout(area, &mut LayoutCtx::new());
    let text = rendered_text(&detail, area);
    assert!(text.contains("Snoozed until"));

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
    let mut active_detail = TaskDetailForm::new(
        Some(&task),
        std::slice::from_ref(&task),
        &[],
        &[],
        &[],
        None,
    );
    active_detail.layout(area, &mut LayoutCtx::new());
    assert!(!rendered_text(&active_detail, area).contains("Snoozed until"));
}

#[test]
fn snoozed_until_hotkey_opens_detail_picker() {
    let mut task = test_task();
    task.state = TaskState::Snoozed;
    task.snoozed_until = Some(time::macros::datetime!(2026-07-24 8:00));
    let mut detail = TaskDetailForm::new(
        Some(&task),
        std::slice::from_ref(&task),
        &[],
        &[],
        &[],
        None,
    );
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
        workspaces: Vec::new(),
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
fn backspace_targets_visible_task_even_when_store_selection_is_stale() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![
            test_task(),
            task_with("task-2", "Second", TaskState::InProgress),
        ],
        people: Vec::new(),
        workspaces: Vec::new(),
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
        [AppMsg::OpenDeleteTask { task_id, return_focus: None }] if task_id == "task-2"
    ));
}

#[test]
fn completed_task_moves_from_in_progress_to_archived_view() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
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
    assert!(text.contains("No active tasks"));

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
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    app.delete_return_focus = Some(TreePath::from_keys([ChildKey::new("detail")]));
    let mut ctx = EventCtx::default();

    app.delete_task("task-1".to_string(), &mut ctx);

    assert!(app.context.store.borrow().state().tasks.is_empty());
    assert_eq!(app.delete_return_focus, None);
    assert_eq!(
        ctx.notifications(),
        &[tuicore::Notification::success(
            "Task deleted",
            "“Original” was deleted."
        )]
    );
}

#[test]
fn management_create_dialog_layers_over_management_workspace() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
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
fn creating_management_entities_notifies_for_people_workspaces_and_tags() {
    let cases = [
        (
            ManagementEntityDraft::Person {
                name: "Ada".into(),
                email: "ada@example.com".into(),
                about: "Compiler expert".into(),
            },
            tuicore::Notification::success("Person created", "“Ada” was created."),
        ),
        (
            ManagementEntityDraft::Workspace {
                key: "CORE".into(),
                name: "Core".into(),
                description: "Platform".into(),
            },
            tuicore::Notification::success("Workspace created", "“Core” was created."),
        ),
        (
            ManagementEntityDraft::Tag {
                label: "backend".into(),
            },
            tuicore::Notification::success("Tag created", "“backend” was created."),
        ),
    ];

    for (draft, expected) in cases {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let mut app = App::new(context.store, context.coordinator);
        let mut ctx = EventCtx::default();

        app.submit_create_management(draft, &mut ctx);

        assert_eq!(ctx.notifications(), &[expected]);
    }
}

#[test]
fn deleting_management_entities_notifies_for_people_workspaces_and_tags() {
    let cases = [
        (
            ManagementDialogKind::People,
            "person-1",
            WorkspaceSnapshot {
                tasks: Vec::new(),
                people: vec![Person::new("person-1".into(), "Ada".into(), String::new())],
                workspaces: Vec::new(),
                tags: Vec::new(),
            },
            tuicore::Notification::success("Person deleted", "“Ada” was deleted."),
        ),
        (
            ManagementDialogKind::Workspaces,
            "workspace-1",
            WorkspaceSnapshot {
                tasks: Vec::new(),
                people: Vec::new(),
                workspaces: vec![Workspace::new(
                    "workspace-1".into(),
                    "CORE".into(),
                    "Core".into(),
                    String::new(),
                )],
                tags: Vec::new(),
            },
            tuicore::Notification::success("Workspace deleted", "“Core” was deleted."),
        ),
        (
            ManagementDialogKind::Tags,
            "tag-1",
            WorkspaceSnapshot {
                tasks: Vec::new(),
                people: Vec::new(),
                workspaces: Vec::new(),
                tags: vec![Tag::new("tag-1".into(), "backend".into())],
            },
            tuicore::Notification::success("Tag deleted", "“backend” was deleted."),
        ),
    ];

    for (kind, entity_id, snapshot, expected) in cases {
        let (_runtime, context, _store) = test_context(snapshot);
        let mut app = App::new(context.store, context.coordinator);
        let mut ctx = EventCtx::default();

        app.delete_management(kind, entity_id, &mut ctx);

        assert_eq!(ctx.notifications(), &[expected]);
    }
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
fn complete_dialog_has_done_reject_and_cancel_actions() {
    for (key, expected_state) in [
        (Key::Char('d'), Some(TaskState::Done)),
        (Key::Char('r'), Some(TaskState::Rejected)),
        (Key::Char('c'), None),
        (Key::Esc, None),
    ] {
        let mut dialog = complete_task_dialog(&test_task());
        let mut ctx = EventCtx::default();

        let outcome = dialog.event(&TuiEvent::Key(KeyEvent::from(key)), &mut ctx);

        assert!(outcome.handled());
        match expected_state {
            Some(state) => assert!(matches!(
                ctx.messages(),
                [AppMsg::CompleteTask { task_id, state: actual }]
                    if task_id == "task-1" && *actual == state
            )),
            None => assert!(matches!(ctx.messages(), [AppMsg::CloseCompleteTaskDialog])),
        }
    }

    let text = rendered_text(&complete_task_dialog(&test_task()), Rect::new(0, 0, 80, 8));
    assert!(text.contains("Done (d)"));
    assert!(text.contains("Reject (r)"));
    assert!(text.contains("Cancel (c)"));
}

#[test]
fn complete_outcomes_patch_optimistically_persist_and_focus_task_table() {
    for state in [TaskState::Done, TaskState::Rejected] {
        let mut task = test_task();
        task.snoozed_until = Some(time::macros::datetime!(2026-08-24 8:00));
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![task],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let coordinator = Rc::clone(&context.coordinator);
        let mut workspace = TaskWorkspace::new(context.clone());
        let area = Rect::new(0, 0, 120, 40);
        workspace.layout(area, &mut LayoutCtx::new());
        assert_eq!(
            workspace.table().highlighted_id().as_deref(),
            Some("task-1")
        );
        assert_eq!(workspace.detail().task_id.as_deref(), Some("task-1"));
        let mut app = App::new(context.store, context.coordinator);
        app.complete_return_focus = Some(CompleteReturnFocus {
            task_id: "task-1".into(),
            task_state: TaskState::InProgress,
            task_selected_on_open: true,
            path: TreePath::from_keys([ChildKey::new("detail")]),
        });
        let mut ctx = EventCtx::default();

        app.complete_task("task-1".into(), state, &mut ctx);

        let store = store.borrow();
        let saved = &store.state().tasks[0];
        assert_eq!(saved.state, state);
        assert_eq!(saved.snoozed_until, None);
        assert!(coordinator.borrow().has_pending());
        assert_eq!(app.complete_return_focus, None);
        assert_eq!(
            ctx.focus_request(),
            Some(&initial_task_table_focus_request())
        );
        assert!(!app.primary_dialog().is_active());
        let expected = match state {
            TaskState::Done => {
                tuicore::Notification::success("Task completed", "“Original” moved to done.")
            }
            TaskState::Rejected => {
                tuicore::Notification::success("Task rejected", "“Original” moved to rejected.")
            }
            _ => unreachable!(),
        };
        assert_eq!(ctx.notifications(), &[expected]);
        drop(store);
        workspace.layout(area, &mut LayoutCtx::new());
        assert_eq!(workspace.table().highlighted_id(), None);
        assert_eq!(workspace.detail().task_id, None);
    }
}

#[test]
fn complete_cancel_falls_back_to_task_table_when_origin_task_disappears() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let origin = TreePath::from_keys([ChildKey::new("detail"), ChildKey::new("title")]);
    app.open_complete_task_dialog("task-1", Some(origin), &mut EventCtx::default());
    store
        .borrow_mut()
        .dispatch(AppEvent::TaskDeleted("task-1".into()));
    let mut ctx = EventCtx::default();

    app.close_complete_task_dialog(&mut ctx);

    assert_eq!(app.complete_return_focus, None);
    assert_eq!(
        ctx.focus_request(),
        Some(&initial_task_table_focus_request())
    );
}

#[test]
fn delete_task_dialog_fits_its_content() {
    let snapshot = WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    };
    let (_runtime, context, _store) = test_context(snapshot);
    let mut app = App::new(context.store, context.coordinator);
    let area = Rect::new(0, 0, 120, 40);

    app.open_delete_task_dialog("task-1", None, &mut EventCtx::default());
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
        workspaces: Vec::new(),
        tags: Vec::new(),
    };
    let (_runtime, context, _store) = test_context(snapshot);
    let mut app = App::new(context.store, context.coordinator);
    let area = Rect::new(0, 0, 120, 40);

    app.open_create_task_dialog(None, &mut EventCtx::default());
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
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let mut app = App::new(context.store, context.coordinator);
    let mut ctx = EventCtx::default();

    app.submit_create_task(
        CreateTaskDraft {
            title: "  fix   dont crash... ".to_string(),
        },
        &mut ctx,
    );

    let state = store.borrow();
    let task = state.state().tasks.first().expect("task should be created");
    assert_eq!(task.title, "Fix don't crash");
    assert_eq!(task.description, "");
    assert_eq!(task.size, TaskSize::Small);
    assert_eq!(task.state, TaskState::Backlog);
    assert_eq!(
        ctx.focus_request(),
        Some(&initial_task_table_focus_request())
    );
    assert_eq!(
        ctx.notifications(),
        &[tuicore::Notification::success(
            "Task created",
            "“Fix don't crash” was added to backlog."
        )]
    );
}

#[test]
fn create_task_submission_uses_default_workspace() {
    let workspace = Workspace::new(
        "workspace-1".into(),
        "ONE".into(),
        "Workspace one".into(),
        String::new(),
    );
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: vec![workspace.clone()],
        tags: Vec::new(),
    });
    store
        .borrow_mut()
        .dispatch(AppEvent::AppSettingChangeRequested {
            key: DEFAULT_WORKSPACE_SETTING.into(),
            value: workspace.id.clone(),
            generation: 1,
        });
    let mut app = App::new(context.store, context.coordinator);

    app.submit_create_task(
        CreateTaskDraft {
            title: "Defaulted task".into(),
        },
        &mut EventCtx::default(),
    );

    assert_eq!(
        store.borrow().state().tasks[0].workspace_id.as_deref(),
        Some("workspace-1")
    );
}

#[test]
fn calendar_title_submission_opens_scheduler_without_creating_task() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    store
        .borrow_mut()
        .dispatch(AppEvent::AppSettingChangeRequested {
            key: DEFAULT_SNOOZE_TIME_SETTING.into(),
            value: "14:30".into(),
            generation: 1,
        });
    let coordinator = Rc::clone(&context.coordinator);
    let mut app = App::new(context.store, context.coordinator);
    let mut layout = LayoutCtx::new();
    app.layout(Rect::new(0, 0, 100, 30), &mut layout);
    let task_path = layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "data-view")
        .expect("task table should be focusable")
        .path
        .clone();
    let new_path = layout
        .focus_targets()
        .iter()
        .find(|target| target.path.keys().iter().any(|part| part.as_str() == "new"))
        .expect("new button should be focusable")
        .path
        .clone();
    app.dispatch_event(
        &EventRoute::new(task_path),
        &TuiEvent::Key(Key::Char(']').into()),
        &mut EventCtx::default(),
    );
    let mut calendar_layout = LayoutCtx::new();
    app.layout(Rect::new(0, 0, 100, 30), &mut calendar_layout);
    let calendar_path = calendar_layout
        .focus_targets()
        .iter()
        .find(|target| target.id.as_str() == "calendar")
        .expect("calendar should be focusable")
        .path
        .clone();
    app.dispatch_event(
        &EventRoute::new(calendar_path),
        &TuiEvent::Key(Key::Right.into()),
        &mut EventCtx::default(),
    );
    let selected_date = app.calendar_create_context.selected_date();
    let mut open_ctx = EventCtx::default();
    app.dispatch_event(
        &EventRoute::new(new_path),
        &TuiEvent::Key(Key::Enter.into()),
        &mut open_ctx,
    );
    let calendar_date = match open_ctx.messages() {
        [AppMsg::OpenCreateTask { calendar_date }] => *calendar_date,
        _ => panic!("new button should open task creation"),
    };
    assert_eq!(calendar_date, Some(selected_date));
    app.open_create_task_dialog(calendar_date, &mut EventCtx::default());

    let mut submit_ctx = EventCtx::default();
    app.submit_create_task(
        CreateTaskDraft {
            title: "Schedule follow up".into(),
        },
        &mut submit_ctx,
    );

    assert!(store.borrow().state().tasks.is_empty());
    assert!(!coordinator.borrow().has_pending());
    assert!(app.pending_calendar_task.is_some());
    assert!(matches!(app.primary_dialog().layer(), AppDialog::Snooze(_)));
    assert!(submit_ctx.notifications().is_empty());
}

#[test]
fn calendar_schedule_rejects_non_future_times_then_creates_exact_task_once() {
    let mut existing = test_task();
    existing.rank = 7;
    let workspace = Workspace::new(
        "workspace-1".into(),
        "ONE".into(),
        "Workspace one".into(),
        String::new(),
    );
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![existing],
        people: Vec::new(),
        workspaces: vec![workspace.clone()],
        tags: Vec::new(),
    });
    store
        .borrow_mut()
        .dispatch(AppEvent::AppSettingChangeRequested {
            key: DEFAULT_WORKSPACE_SETTING.into(),
            value: workspace.id.clone(),
            generation: 1,
        });
    let coordinator = Rc::clone(&context.coordinator);
    let mut app = App::new(context.store, context.coordinator);
    app.open_create_task_dialog(
        Some(time::macros::date!(2030 - 01 - 01)),
        &mut EventCtx::default(),
    );
    app.submit_create_task(
        CreateTaskDraft {
            title: "  schedule   follow up ".into(),
        },
        &mut EventCtx::default(),
    );
    let now = time::macros::datetime!(2026-07-23 12:00);
    let warning = tuicore::Notification::warning(
        "Schedule time has passed",
        "Choose a future date and time.",
    );
    let mut past_ctx = EventCtx::default();

    app.schedule_created_task_at(
        time::macros::datetime!(2026-07-23 11:59),
        Some(time::macros::datetime!(2026-07-23 11:59)),
        now,
        &mut past_ctx,
    );

    assert_eq!(store.borrow().state().tasks.len(), 1);
    assert_eq!(store.borrow().state().last_custom_snooze, None);
    assert!(!coordinator.borrow().has_pending());
    assert!(app.primary_dialog().is_active());
    assert!(matches!(app.primary_dialog().layer(), AppDialog::Snooze(_)));
    assert_eq!(
        app.pending_calendar_task
            .as_ref()
            .map(|draft| draft.title.as_str()),
        Some("Schedule follow up")
    );
    assert_eq!(past_ctx.notifications(), std::slice::from_ref(&warning));

    let mut equal_ctx = EventCtx::default();
    app.schedule_created_task_at(now, Some(now), now, &mut equal_ctx);

    assert_eq!(store.borrow().state().tasks.len(), 1);
    assert_eq!(store.borrow().state().last_custom_snooze, None);
    assert!(!coordinator.borrow().has_pending());
    assert!(app.primary_dialog().is_active());
    assert!(matches!(app.primary_dialog().layer(), AppDialog::Snooze(_)));
    assert_eq!(
        app.pending_calendar_task
            .as_ref()
            .map(|draft| draft.title.as_str()),
        Some("Schedule follow up")
    );
    assert_eq!(equal_ctx.notifications(), &[warning]);

    let future = time::macros::datetime!(2026-07-30 14:30);
    let mut success_ctx = EventCtx::default();
    app.schedule_created_task_at(future, Some(future), now, &mut success_ctx);

    let state = store.borrow();
    assert_eq!(state.state().tasks.len(), 2);
    let task = state
        .state()
        .tasks
        .iter()
        .find(|task| task.id.starts_with("pending-"))
        .expect("scheduled task should be optimistically created");
    assert_eq!(task.title, "Schedule follow up");
    assert_eq!(task.state, TaskState::Snoozed);
    assert_eq!(task.snoozed_until, Some(future));
    assert_eq!(task.workspace_id.as_deref(), Some("workspace-1"));
    assert_eq!(task.rank, 8);
    assert_eq!(state.state().last_custom_snooze, Some(future));
    drop(state);
    assert_eq!(
        success_ctx.notifications(),
        &[tuicore::Notification::success(
            "Task created",
            "“Schedule follow up” was scheduled for 2026-07-30T14:30:00."
        )]
    );
    assert!(!app.primary_dialog().is_active());
    assert!(app.pending_calendar_task.is_none());
    assert!(app.create_task_calendar_date.is_none());
    assert!(coordinator.borrow().has_pending());

    app.schedule_created_task_at(future, Some(future), now, &mut EventCtx::default());

    let state = store.borrow();
    assert_eq!(state.state().tasks.len(), 2);
    assert_eq!(
        state
            .state()
            .tasks
            .iter()
            .filter(|task| task.id.starts_with("pending-"))
            .count(),
        1
    );
}

#[test]
fn canceling_calendar_scheduler_discards_pending_draft_without_creating_task() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    let coordinator = Rc::clone(&context.coordinator);
    let mut app = App::new(context.store, context.coordinator);
    app.open_create_task_dialog(
        Some(time::macros::date!(2999 - 01 - 01)),
        &mut EventCtx::default(),
    );
    app.submit_create_task(
        CreateTaskDraft {
            title: "Canceled schedule".into(),
        },
        &mut EventCtx::default(),
    );

    app.close_snooze_dialog(&mut EventCtx::default());

    assert!(app.pending_calendar_task.is_none());
    assert!(app.create_task_calendar_date.is_none());
    assert!(store.borrow().state().tasks.is_empty());
    assert!(!coordinator.borrow().has_pending());
}

#[test]
fn creation_dialogs_close_from_nested_control_focus_mode() {
    let area = Rect::new(0, 0, 80, 24);
    let cases = [
        (
            create_management_dialog_host(ManagementDialogKind::Workspaces),
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
fn created_task_state_hotkey_focuses_open_dropdown() {
    let (_runtime, context, store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
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
fn focused_dropdown_search_allows_ctrl_z_to_snooze_task() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: vec![test_task()],
        people: Vec::new(),
        workspaces: Vec::new(),
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
    let focused_path = focused.path.clone();

    let effects = dispatcher.dispatch_event(
        &mut workspace,
        &EventRoute::new(focused_path.clone()),
        &TuiEvent::Key(KeyEvent {
            code: Key::Char('z'),
            modifiers: KeyModifiers::CONTROL,
        }),
        AnimationSettings::default(),
    );

    assert!(effects.outcome.handled());
    assert!(matches!(
        effects.messages.as_slice(),
        [AppMsg::OpenTaskSnooze {
            task_id,
            return_focus: Some(return_focus),
        }] if task_id == "task-1" && return_focus == &focused_path
    ));
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
        workspaces: Vec::new(),
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
            }],
            people: Vec::new(),
            workspaces: Vec::new(),
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
        workspaces: Vec::new(),
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
        workspaces: Vec::new(),
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
        .expect("tab should move to workspace");
    dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
    assert!(
        focus
            .current()
            .expect("next control should be focused")
            .path
            .keys()
            .iter()
            .any(|key| key.as_str() == "workspaces")
    );
    assert_eq!(store.borrow().state().tasks[0].size, TaskSize::Big);
}

#[test]
fn management_routing_builds_concrete_dialog_variant() {
    let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });

    assert!(matches!(
        management_dialog(context, ManagementDialogKind::Workspaces),
        AppDialog::Workspaces(_)
    ));
}
