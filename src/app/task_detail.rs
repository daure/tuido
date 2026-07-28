use super::*;

pub(super) struct TaskDetailForm {
    pub(super) root: Flex<AppMsg>,
    pub(super) task_id: Option<String>,
    pub(super) task_state: Option<TaskState>,
    pub(super) patches: PatchSink,
    pub(super) save_status: SaveStatusLine,
}

impl TaskDetailForm {
    pub(super) fn new(
        task: Option<&TaskRow>,
        people: &[Person],
        projects: &[Project],
        tags: &[Tag],
        save_error: Option<&str>,
    ) -> Self {
        let patches = Rc::new(RefCell::new(Vec::new()));
        let save_status = SaveStatusLine::new(save_error);
        Self {
            root: Flex::column().child(
                "form",
                detail_form(
                    task,
                    people,
                    projects,
                    tags,
                    Rc::clone(&patches),
                    save_status.clone(),
                ),
                FlexItem::content(),
            ),
            task_id: task.map(|task| task.id.clone()),
            task_state: task.map(|task| task.state),
            patches,
            save_status,
        }
    }

    pub(super) fn take_patches(&mut self) -> Vec<(String, TaskPatch)> {
        let Some(task_id) = self.task_id.clone() else {
            self.patches.borrow_mut().clear();
            return Vec::new();
        };
        self.patches
            .borrow_mut()
            .drain(..)
            .map(|patch| (task_id.clone(), patch))
            .collect()
    }

    pub(super) fn set_task(
        &mut self,
        task: Option<&TaskRow>,
        people: &[Person],
        projects: &[Project],
        tags: &[Tag],
        save_error: Option<&str>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.patches = Rc::new(RefCell::new(Vec::new()));
        self.task_id = task.map(|task| task.id.clone());
        self.task_state = task.map(|task| task.state);
        self.save_status = SaveStatusLine::new(save_error);
        self.root
            .replace(
                "form",
                detail_form(
                    task,
                    people,
                    projects,
                    tags,
                    Rc::clone(&self.patches),
                    self.save_status.clone(),
                ),
                FlexItem::content(),
                ctx,
            )
            .expect("detail form host should contain form child");
    }

    pub(super) fn set_save_error(&self, save_error: Option<&str>) {
        self.save_status.set_error(save_error);
    }
}

impl TuiNode<AppMsg> for TaskDetailForm {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.root.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.root.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.root.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        let outcome = self.root.event(event, ctx);
        detail_outcome_or_escape(outcome, event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let outcome = self.root.dispatch_event(route, event, ctx);
        detail_outcome_or_escape(outcome, event, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        self.root.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.root.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.root.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.root.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.root.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.root.destroy(ctx);
    }
}

pub(super) fn task_toolbar(
    pending_view: TaskViewChange,
    active_view: ActiveTaskView,
) -> Flex<AppMsg> {
    let view = TaskViewMenu::new(pending_view, active_view);
    let new_task = Button::new("New")
        .hotkey(keys::TASK_QUICK_CREATE.hotkey())
        .on_press(|| AppMsg::OpenCreateTask);

    Flex::row()
        .align(CrossAlign::Center)
        .gap(1)
        .child("view", view, FlexItem::content())
        .child("space", Paragraph::new(""), FlexItem::fill(1))
        .child("new", new_task, FlexItem::content())
}

pub(super) fn task_split(store: &AppStore, task_view: TaskView) -> TaskPane {
    let store_ref = store.borrow();
    let state = store_ref.state();
    let rows = task_rows_for_view(&state.tasks, task_view);
    let selected = state.selected_task_id.as_deref().filter(|id| {
        state
            .tasks
            .iter()
            .any(|task| task.id == **id && task_view.contains(task))
    });
    let copy_context = TaskCopyContext::new(&state.people, &state.projects, &state.tags);
    let table = task_table_with_copy_context(rows, selected, copy_context);
    let selected_task = selected.and_then(|id| state.tasks.iter().find(|task| task.id == id));
    let save_error = selected_task.and_then(|task| state.task_status_error(&task.id));
    let detail = TaskDetailForm::new(
        selected_task,
        &state.people,
        &state.projects,
        &state.tags,
        save_error,
    );
    ResponsiveSplit::master_detail(table, detail)
}

pub(super) fn task_rows_for_view(tasks: &[Task], task_view: TaskView) -> Vec<TaskRow> {
    let mut rows = tasks
        .iter()
        .filter(|task| task_view.contains(task))
        .cloned()
        .collect::<Vec<_>>();
    rows.sort_by_key(|task| task.rank);
    rows
}

#[cfg(test)]
pub(super) fn task_table(rows: Vec<TaskRow>, selected_id: Option<&str>) -> TaskTable {
    task_table_with_copy_context(rows, selected_id, TaskCopyContext::default())
}

pub(super) fn task_table_with_copy_context(
    rows: Vec<TaskRow>,
    selected_id: Option<&str>,
    copy_context: TaskCopyContext,
) -> TaskTable {
    let mut table = ListControl::new_fields(
        rows,
        |row: &TaskRow| row.id.clone(),
        [ListControlField::text("Task")],
        |_, _| unreachable!("task creation uses the task dialog"),
    )
    .copy_with(move |row| copy_context.export(row))
    .action_bar(true)
    .panel_visible(false)
    .filter_controls(false)
    .focused_events_before_global_hotkeys(false)
    .activation_mode(ActivationMode::OnActivateKey)
    .selection_mode(SelectionMode::Single)
    .selection_trigger(SelectionTrigger::OnNavigate)
    .keybindings(
        ListControlKeyBindings::default()
            .add([])
            .remove([])
            .edit([])
            .reorder([keys::TASK_MOVE_MODE.key_spec()]),
    )
    .max_rows(usize::MAX)
    .columns(vec![
        Column::text("rank", "", Constraint::Length(0), |row: &TaskRow| {
            row.rank.to_string()
        })
        .reorderable(|row| row.rank, |row, rank| row.rank = rank)
        .hidden(),
        Column::rich(
            "state",
            "",
            Constraint::Length(1),
            |row: &TaskRow, _: &CellContext<String>| Line::from(task_state_icon(row.state)),
        )
        .constrained()
        .filter_key(|row| row.state.label().to_string()),
        Column::rich(
            "priority",
            "",
            Constraint::Length(1),
            |row: &TaskRow, _: &CellContext<String>| priority_icon_line(row.priority),
        )
        .constrained()
        .sortable(|row| match row.priority {
            TaskPriority::Low => "2".to_string(),
            TaskPriority::Medium => "1".to_string(),
            TaskPriority::High => "0".to_string(),
        })
        .filter_key(|row| row.priority.label().to_string()),
        Column::rich(
            "size",
            "Size",
            Constraint::Length(3),
            |row: &TaskRow, _: &CellContext<String>| chip_line(row.size.label(), row.size.role()),
        )
        .constrained()
        .filter_key(|row| row.size.label().to_string()),
        Column::text("title", "Task", Constraint::Fill(1), |row: &TaskRow| {
            row.title.clone()
        })
        .sortable(|row| row.title.clone())
        .filter_key(|row| row.title.clone()),
    ])
    .reorderable_by("rank");
    if let Some(id) = selected_id {
        table.data_view_mut().select_id(id.to_string());
    }
    table
}

pub(super) struct TaskTagsInput {
    pub(super) input: TagInput<String>,
    pub(super) available_tags: Vec<Tag>,
    pub(super) patch_sink: PatchSink,
}

impl TaskTagsInput {
    pub(super) fn new(task: &Task, tags: &[Tag], patch_sink: PatchSink) -> Self {
        let input = TagInput::with_options(
            tags.iter().cloned(),
            |tag| tag.id.clone(),
            |tag| tag.label.clone(),
        )
        .selected_existing(task.tag_ids.iter().cloned())
        .placeholder("")
        .panel("Tags")
        .hotkey(keys::TASK_TAGS_FIELD.hotkey());
        Self {
            input,
            available_tags: tags.to_vec(),
            patch_sink,
        }
    }

    pub(super) fn sync_events(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let events = self.input.take_events();
        let value_changed = events.iter().any(|event| {
            !matches!(
                event,
                TagInputEvent::QueryChanged { .. } | TagInputEvent::SubmitRequested
            )
        });
        if !value_changed {
            return;
        }

        let mut selected = Vec::new();
        for tag in self.input.selected_tags() {
            let tag = match tag {
                SelectedTag::Existing { id, label } => Tag {
                    id: id.clone(),
                    label: label.clone(),
                },
                SelectedTag::Custom { label } => {
                    if let Some(existing) = self
                        .available_tags
                        .iter()
                        .find(|existing| existing.label == *label)
                    {
                        existing.clone()
                    } else {
                        let tag = Tag {
                            id: Uuid::new_v4().to_string(),
                            label: label.trim().to_string(),
                        };
                        self.available_tags.push(tag.clone());
                        tag
                    }
                }
            };
            if !tag.label.is_empty() && !selected.iter().any(|item: &Tag| item.id == tag.id) {
                selected.push(tag);
            }
        }
        self.patch_sink.borrow_mut().push(TaskPatch::Tags(selected));
        ctx.request_layout();
        ctx.request_redraw();
    }
}

impl TuiNode<AppMsg> for TaskTagsInput {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        <TagInput<String> as TuiNode<AppMsg>>::measure(&self.input, proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        <TagInput<String> as TuiNode<AppMsg>>::layout(&mut self.input, area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        <TagInput<String> as TuiNode<AppMsg>>::render(&self.input, frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        let outcome = <TagInput<String> as TuiNode<AppMsg>>::event(&mut self.input, event, ctx);
        self.sync_events(ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let outcome = <TagInput<String> as TuiNode<AppMsg>>::dispatch_event(
            &mut self.input,
            route,
            event,
            ctx,
        );
        self.sync_events(ctx);
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        <TagInput<String> as TuiNode<AppMsg>>::dispatch_focus(
            &mut self.input,
            target,
            focused,
            ctx,
        );
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        <TagInput<String> as TuiNode<AppMsg>>::tick(&mut self.input, dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        <TagInput<String> as TuiNode<AppMsg>>::init(&mut self.input, ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        <TagInput<String> as TuiNode<AppMsg>>::mount(&mut self.input, ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        <TagInput<String> as TuiNode<AppMsg>>::unmount(&mut self.input, ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        <TagInput<String> as TuiNode<AppMsg>>::destroy(&mut self.input, ctx);
    }
}

pub(super) fn detail_form(
    task: Option<&TaskRow>,
    people: &[Person],
    projects: &[Project],
    tags: &[Tag],
    patch_sink: PatchSink,
    save_status: SaveStatusLine,
) -> Flex<AppMsg> {
    let Some(task) = task else {
        return Flex::<AppMsg>::column().child(
            "empty",
            Paragraph::new("No task selected."),
            FlexItem::fixed(1),
        );
    };

    let status_fields = Flex::<AppMsg>::row()
        .gap(1)
        .child(
            "state",
            dropdown_single("State", state_choices(), task.state.id(), {
                let patch_sink = Rc::clone(&patch_sink);
                move |id| {
                    if let Some(value) = TaskState::parse(&id) {
                        patch_sink.borrow_mut().push(TaskPatch::State(value));
                    }
                }
            })
            .hotkey(keys::TASK_STATE_FIELD.hotkey()),
            FlexItem::fill(1),
        )
        .child(
            "priority",
            dropdown_single("Priority", priority_choices(), task.priority.id(), {
                let patch_sink = Rc::clone(&patch_sink);
                move |id| {
                    if let Some(value) = TaskPriority::parse(&id) {
                        patch_sink.borrow_mut().push(TaskPatch::Priority(value));
                    }
                }
            })
            .hotkey(keys::TASK_PRIORITY_FIELD.hotkey()),
            FlexItem::fill(1),
        )
        .child(
            "size",
            dropdown_single("Size", size_choices(), task.size.id(), {
                let patch_sink = Rc::clone(&patch_sink);
                move |id| {
                    if let Some(value) = TaskSize::parse(&id) {
                        patch_sink.borrow_mut().push(TaskPatch::Size(value));
                    }
                }
            })
            .hotkey(keys::TASK_SIZE_FIELD.hotkey()),
            FlexItem::fill(1),
        );

    let mut date_fields = Flex::<AppMsg>::row().gap(1);
    if task.state == TaskState::Snoozed {
        date_fields = date_fields.child(
            "snoozed-until",
            DateTimePickerDropdown::<AppMsg>::new()
                .value(task.snoozed_until)
                .placeholder("")
                .panel("Snoozed until")
                .hotkey(keys::TASK_SNOOZED_UNTIL_FIELD.hotkey())
                .on_select({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |until| {
                        patch_sink.borrow_mut().push(TaskPatch::Snooze {
                            until,
                            remember_custom: None,
                        });
                        AppMsg::Noop
                    }
                }),
            FlexItem::fill(1),
        );
    }
    let date_fields = date_fields
        .child(
            "start-date",
            DatePickerDropdown::<AppMsg>::new()
                .value(parse_date(task.start_date.as_deref()))
                .placeholder("")
                .panel("Start date")
                .hotkey(keys::TASK_START_DATE_FIELD.hotkey())
                .on_select({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |date| {
                        patch_sink
                            .borrow_mut()
                            .push(TaskPatch::StartDate(Some(date.to_string())));
                        AppMsg::Noop
                    }
                }),
            FlexItem::fill(1),
        )
        .child(
            "end-date",
            DatePickerDropdown::<AppMsg>::new()
                .value(parse_date(task.due_date.as_deref()))
                .placeholder("")
                .panel("End date")
                .hotkey(keys::TASK_END_DATE_FIELD.hotkey())
                .on_select({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |date| {
                        patch_sink
                            .borrow_mut()
                            .push(TaskPatch::EndDate(Some(date.to_string())));
                        AppMsg::Noop
                    }
                }),
            FlexItem::fill(1),
        );

    Flex::<AppMsg>::column()
        .gap(0)
        .child("save-status", save_status, FlexItem::content())
        .child(
            "title",
            TextInput::<AppMsg>::new()
                .value(task.title.clone())
                .panel("Title")
                .hotkey(keys::TASK_TITLE_FIELD.hotkey())
                .on_edit_end({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| {
                        patch_sink.borrow_mut().push(TaskPatch::Title(value));
                        AppMsg::Noop
                    }
                }),
            FlexItem::fixed(3),
        )
        .child(
            "description",
            TextareaInput::<AppMsg>::new()
                .value(task.description.clone())
                .panel("Description")
                .hotkey(keys::TASK_DESCRIPTION_FIELD.hotkey())
                .editor_hotkey(keys::TASK_DESCRIPTION_EDITOR.hotkey())
                .on_edit_end({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| {
                        patch_sink.borrow_mut().push(TaskPatch::Description(value));
                        AppMsg::Noop
                    }
                })
                .min_rows(2)
                .max_rows(6),
            FlexItem::content(),
        )
        .child("status-fields", status_fields, FlexItem::fixed(3))
        .child("date-fields", date_fields, FlexItem::fixed(3))
        .child(
            "people-projects",
            Flex::<AppMsg>::row()
                .gap(1)
                .child(
                    "people",
                    task_people_dropdown(task, people, Rc::clone(&patch_sink)),
                    FlexItem::fill(1),
                )
                .child(
                    "projects",
                    task_projects_dropdown(task, projects, Rc::clone(&patch_sink)),
                    FlexItem::fill(1),
                ),
            FlexItem::fixed(3),
        )
        .child(
            "tags",
            TaskTagsInput::new(task, tags, Rc::clone(&patch_sink)),
            FlexItem::content(),
        )
        .child(
            "links",
            TaskLinksInput::new(task, Rc::clone(&patch_sink)),
            FlexItem::content(),
        )
}

pub(super) fn parse_date(value: Option<&str>) -> Option<Date> {
    value.and_then(|value| {
        Date::parse(value, &time::format_description::well_known::Iso8601::DATE).ok()
    })
}

pub(super) fn chip_line(label: &'static str, role: ChipColorRole) -> Line<'static> {
    let theme = tuicore::theme();
    let color = match role {
        ChipColorRole::Accent => theme.accent_fg(),
        ChipColorRole::Success => theme.success_fg(),
        ChipColorRole::Warning => theme.warning_fg(),
        ChipColorRole::Error => theme.error_fg(),
        ChipColorRole::Selected => theme.selected_bg(),
        ChipColorRole::Highlight => theme.highlight_bg(),
        ChipColorRole::Muted => theme.border_fg(),
    };
    Line::from(Span::styled(
        label,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
}

pub(super) fn task_state_icon(state: TaskState) -> &'static str {
    match state {
        TaskState::Backlog => "",
        TaskState::Todo => "",
        TaskState::InProgress => "",
        TaskState::Done => "",
        TaskState::Snoozed => "󰒲",
        TaskState::Rejected => "",
    }
}

pub(super) fn priority_icon_line(priority: TaskPriority) -> Line<'static> {
    let theme = tuicore::theme();
    let color = match priority {
        TaskPriority::Low => theme.accent_fg(),
        TaskPriority::Medium => theme.warning_fg(),
        TaskPriority::High => theme.error_fg(),
    };
    Line::from(Span::styled(
        task_priority_icon(priority),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
}

pub(super) fn task_priority_icon(priority: TaskPriority) -> &'static str {
    match priority {
        TaskPriority::Low => "󰅀",
        TaskPriority::Medium => "󰇼",
        TaskPriority::High => "󰅃",
    }
}

pub(crate) fn detail_escape(event: &TuiEvent) -> bool {
    app_keymap::matches_any(event, &[keys::DETAIL_CLOSE, keys::DETAIL_CLOSE_ALT])
}

pub(super) fn detail_outcome_or_escape(
    outcome: EventOutcome,
    event: &TuiEvent,
    ctx: &mut EventCtx<AppMsg>,
) -> EventOutcome {
    if detail_escape(event) {
        focus_task_table(ctx);
        return EventOutcome::Handled;
    }
    outcome
}

pub(super) fn focus_task_table(ctx: &mut EventCtx<AppMsg>) {
    ctx.focus(initial_task_table_focus_request());
    ctx.stop_propagation();
    ctx.request_redraw();
}

pub(super) fn dropdown_single(
    label: &'static str,
    rows: Vec<Choice>,
    selected: &str,
    on_select: impl Fn(String) + 'static,
) -> Dropdown<Choice, String> {
    Dropdown::single(rows, |row| row.id.clone(), |row| row.label.clone())
        .label(label)
        .placeholder("")
        .selected_one(selected.to_string())
        .search_mode(DropdownSearchMode::Contains)
        .commit_mode(DropdownCommitMode::Explicit)
        .on_select(move |ids| {
            if let Some(id) = ids.into_iter().next() {
                on_select(id);
            }
        })
}

pub(super) fn dropdown_multi(
    label: &'static str,
    rows: Vec<Choice>,
    selected: &[String],
    on_select: impl Fn(Vec<String>) + 'static,
) -> Dropdown<Choice, String> {
    Dropdown::multi(rows, |row| row.id.clone(), |row| row.label.clone())
        .label(label)
        .placeholder("")
        .selected(selected.iter().cloned())
        .search_mode(DropdownSearchMode::Contains)
        .on_select(on_select)
}

pub(super) fn task_people_dropdown(
    task: &TaskRow,
    people: &[Person],
    patch_sink: PatchSink,
) -> Dropdown<Choice, String> {
    dropdown_multi(
        "People",
        person_choices(people),
        &task.people_ids,
        move |ids| patch_sink.borrow_mut().push(TaskPatch::People(ids)),
    )
    .hotkey(keys::TASK_PEOPLE_FIELD.hotkey())
}

pub(super) fn task_projects_dropdown(
    task: &TaskRow,
    projects: &[Project],
    patch_sink: PatchSink,
) -> Dropdown<Choice, String> {
    dropdown_multi(
        "Projects",
        project_choices(projects),
        &task.project_ids,
        move |ids| patch_sink.borrow_mut().push(TaskPatch::Projects(ids)),
    )
    .hotkey(keys::TASK_PROJECTS_FIELD.hotkey())
}

#[derive(Debug, Clone)]
pub(super) struct Choice {
    pub(super) id: String,
    pub(super) label: String,
}

pub(super) fn state_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "backlog".to_string(),
            label: "Backlog".to_string(),
        },
        Choice {
            id: "todo".to_string(),
            label: "Todo".to_string(),
        },
        Choice {
            id: "in_progress".to_string(),
            label: "In-progress".to_string(),
        },
        Choice {
            id: "done".to_string(),
            label: "Done".to_string(),
        },
        Choice {
            id: "rejected".to_string(),
            label: "Rejected".to_string(),
        },
    ]
}

pub(super) fn size_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "small".to_string(),
            label: "Small".to_string(),
        },
        Choice {
            id: "medium".to_string(),
            label: "Medium".to_string(),
        },
        Choice {
            id: "big".to_string(),
            label: "Big".to_string(),
        },
    ]
}

pub(super) fn priority_choices() -> Vec<Choice> {
    [TaskPriority::Low, TaskPriority::Medium, TaskPriority::High]
        .into_iter()
        .map(|priority| Choice {
            id: priority.id().to_string(),
            label: priority.label().to_string(),
        })
        .collect()
}

pub(super) fn person_choices(people: &[Person]) -> Vec<Choice> {
    people
        .iter()
        .map(|person| Choice {
            id: person.id.clone(),
            label: person.name.clone(),
        })
        .collect()
}

pub(super) fn project_choices(projects: &[Project]) -> Vec<Choice> {
    projects
        .iter()
        .map(|project| Choice {
            id: project.id.clone(),
            label: project.name.clone(),
        })
        .collect()
}
