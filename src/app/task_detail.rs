use super::*;

#[cfg(test)]
use tuicore::SeasonalGlyphs;

const DATE_TIME_PLACEHOLDER: &str = "YYYY-MM-DD HH:MM";

pub(super) fn task_toolbar(
    pending_view: TaskViewChange,
    active_view: ActiveTaskView,
) -> Flex<AppMsg> {
    let view = TaskViewMenu::new(pending_view, active_view);

    Flex::row()
        .align(CrossAlign::Center)
        .gap(1)
        .child("view", view, FlexItem::content())
        .child("space", Paragraph::new(""), FlexItem::fill(1))
}

pub(super) fn project_filter_dropdown(
    projects: &[Project],
    active_filter: ActiveProjectFilter,
) -> Dropdown<Project, String> {
    let selected = active_filter.borrow().iter().cloned().collect::<Vec<_>>();
    Dropdown::single(
        projects.to_vec(),
        |project| project.id.clone(),
        |project| project.name.clone(),
    )
    .placeholder("󰲋 Project")
    .no_selection_text("None")
    .hotkey(keys::TASK_PROJECT_FILTER.hotkey())
    .selected(selected)
    .search_mode(DropdownSearchMode::Contains)
    .commit_mode(DropdownCommitMode::Explicit)
    .variant(DropdownVariant::Filled)
    .max_popup_height(12)
    .on_select(move |ids| *active_filter.borrow_mut() = ids.into_iter().next())
}

pub(super) fn label_filter_dropdown(
    tags: &[Tag],
    active_filter: ActiveLabelFilter,
) -> Dropdown<Tag, String> {
    let selected = active_filter.borrow().clone();
    Dropdown::multi(tags.to_vec(), |tag| tag.id.clone(), |tag| tag.label.clone())
        .placeholder(" Labels")
        .hotkey(keys::TASK_LABEL_FILTER.hotkey())
        .selected(selected)
        .search_mode(DropdownSearchMode::Contains)
        .commit_mode(DropdownCommitMode::Explicit)
        .variant(DropdownVariant::Filled)
        .max_popup_height(12)
        .on_select(move |ids| *active_filter.borrow_mut() = ids)
}

pub(super) fn task_workspace_layout(
    toolbar: Flex<AppMsg>,
    store: &AppStore,
    task_view: TaskView,
    project_filter: Option<&str>,
    label_filter: &[String],
) -> TaskWorkspaceLayout {
    let store_ref = store.borrow();
    let state = store_ref.state();
    let rows = task_rows_for_view(&state.tasks, task_view, project_filter, label_filter);
    let selected = state
        .selected_task_id
        .as_deref()
        .filter(|id| rows.iter().any(|task| task.id == **id));
    let copy_context = TaskCopyContext::new(&state.people, &state.projects, &state.tags);
    let table = task_table_with_copy_context(rows, selected, copy_context)
        .empty_state(task_empty_state(&state.tasks, task_view));
    let selected_task = selected.and_then(|id| state.tasks.iter().find(|task| task.id == id));
    let save_error = selected_task.and_then(|task| state.task_status_error(&task.id));
    let detail = TaskDetailForm::new(
        selected_task,
        &state.tasks,
        &state.people,
        &state.projects,
        &state.tags,
        save_error,
    );
    let master =
        Split::vertical(toolbar, table).constraints(Constraint::Length(1), Constraint::Min(1));
    ResponsiveSplit::master_detail(master, detail).second_visible(selected_task.is_some())
}

pub(super) fn task_rows_for_view(
    tasks: &[Task],
    task_view: TaskView,
    project_filter: Option<&str>,
    label_filter: &[String],
) -> Vec<TaskRow> {
    let mut rows = tasks
        .iter()
        .filter(|task| {
            task_view.contains(task)
                && project_filter
                    .is_none_or(|project_id| task.project_id.as_deref() == Some(project_id))
                && label_filter
                    .iter()
                    .all(|tag_id| task.tag_ids.contains(tag_id))
        })
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
    task_table_with_copy_context_and_empty(
        rows,
        selected_id,
        copy_context,
        SeasonalEmptyState::new("No tasks match your filters"),
    )
}

#[cfg(test)]
pub(super) fn task_table_with_copy_context_on(
    rows: Vec<TaskRow>,
    selected_id: Option<&str>,
    copy_context: TaskCopyContext,
    date: Date,
) -> TaskTable {
    task_table_with_copy_context_and_empty(
        rows,
        selected_id,
        copy_context,
        SeasonalEmptyState::new("No tasks match your filters")
            .date(date)
            .glyphs(SeasonalGlyphs::NerdFont),
    )
}

fn task_table_with_copy_context_and_empty(
    rows: Vec<TaskRow>,
    selected_id: Option<&str>,
    copy_context: TaskCopyContext,
    empty_state: SeasonalEmptyState,
) -> TaskTable {
    let display_context = copy_context.clone();
    let mut table = ListControl::new_fields(
        rows,
        |row: &TaskRow| row.id.clone(),
        [ListControlField::text("Task")],
        |_, _| unreachable!("task creation uses the task dialog"),
    )
    .copy_with(move |row| copy_context.export(row))
    .empty_state(empty_state)
    .hotkey(keys::TASK_AGENT_YANK.hotkey())
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
        Column::text(
            "title",
            "Task",
            Constraint::Fill(1),
            move |row: &TaskRow| format!("{} - {}", display_context.display_id(row), row.title),
        )
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
        let placeholder = if task.tag_ids.is_empty() {
            "No tags added"
        } else {
            "Add another tag"
        };
        let input = TagInput::with_options(
            tags.iter().cloned(),
            |tag| tag.id.clone(),
            |tag| tag.label.clone(),
        )
        .selected_existing(task.tag_ids.iter().cloned())
        .placeholder(placeholder)
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

        let placeholder = if self.input.selected_tags().is_empty() {
            "No tags added"
        } else {
            "Add another tag"
        };
        self.input.set_placeholder(placeholder);

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

pub(crate) fn detail_form(
    task: Option<&TaskRow>,
    catalogs: TaskDetailCatalogs<'_>,
    patch_sink: PatchSink,
    checklist_highlighted_id: Rc<RefCell<Option<String>>>,
    highlighted_issue_link_task_id: Option<&str>,
    save_status: SaveStatusLine,
) -> Flex<AppMsg> {
    let (tasks, people, projects, tags) = catalogs;
    let Some(task) = task else {
        return Flex::<AppMsg>::column();
    };
    let project = task
        .project_id
        .as_deref()
        .and_then(|project_id| projects.iter().find(|project| project.id == project_id));
    let display_id = Rc::new(RefCell::new(task_display_id(task, project)));

    let status_fields = Flex::<AppMsg>::row()
        .gap(1)
        .child(
            "state",
            dropdown_single("State", "Select state", state_choices(), task.state.id(), {
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
            dropdown_single(
                "Priority",
                "Select priority",
                priority_choices(),
                task.priority.id(),
                {
                    let patch_sink = Rc::clone(&patch_sink);
                    move |id| {
                        if let Some(value) = TaskPriority::parse(&id) {
                            patch_sink.borrow_mut().push(TaskPatch::Priority(value));
                        }
                    }
                },
            )
            .hotkey(keys::TASK_PRIORITY_FIELD.hotkey()),
            FlexItem::fill(1),
        )
        .child(
            "size",
            dropdown_single("Size", "Select size", size_choices(), task.size.id(), {
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

    let mut date_fields = Flex::<AppMsg>::row();
    if task.state == TaskState::Snoozed {
        date_fields = date_fields.child(
            "snoozed-until",
            DateTimePickerDropdown::<AppMsg>::new()
                .value(task.snoozed_until)
                .placeholder(DATE_TIME_PLACEHOLDER)
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
    Flex::<AppMsg>::column()
        .gap(0)
        .child("save-status", save_status, FlexItem::content())
        .child(
            "title",
            TaskTitleInput::new(&task.title, Rc::clone(&display_id), Rc::clone(&patch_sink)),
            FlexItem::fixed(3),
        )
        .child(
            "description",
            TextareaInput::<AppMsg>::new()
                .value(task.description.clone())
                .placeholder("Task description")
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
        .child("date-fields", date_fields, FlexItem::content())
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
                    task_projects_dropdown(
                        task,
                        projects,
                        Rc::clone(&display_id),
                        Rc::clone(&patch_sink),
                    ),
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
            "checklist",
            TaskChecklistInput::new(task, Rc::clone(&patch_sink), checklist_highlighted_id),
            FlexItem::content(),
        )
        .child(
            "links",
            TaskLinksInput::new(task, Rc::clone(&patch_sink)),
            FlexItem::content(),
        )
        .child(
            "relations",
            TaskRelationsInput::new(
                task,
                tasks,
                projects,
                Rc::clone(&patch_sink),
                highlighted_issue_link_task_id,
            ),
            FlexItem::content(),
        )
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

pub(super) fn focus_task_table(ctx: &mut EventCtx<AppMsg>) {
    ctx.focus(initial_task_table_focus_request());
    ctx.stop_propagation();
    ctx.request_redraw();
}

pub(super) fn dropdown_single(
    label: &'static str,
    placeholder: &'static str,
    rows: Vec<Choice>,
    selected: &str,
    on_select: impl Fn(String) + 'static,
) -> Dropdown<Choice, String> {
    Dropdown::single(rows, |row| row.id.clone(), |row| row.label.clone())
        .label(label)
        .placeholder(placeholder)
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
    placeholder: &'static str,
    rows: Vec<Choice>,
    selected: &[String],
    on_select: impl Fn(Vec<String>) + 'static,
) -> Dropdown<Choice, String> {
    Dropdown::multi(rows, |row| row.id.clone(), |row| row.label.clone())
        .label(label)
        .placeholder(placeholder)
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
        "Select people",
        person_choices(people),
        &task.people_ids,
        move |ids| patch_sink.borrow_mut().push(TaskPatch::People(ids)),
    )
    .hotkey(keys::TASK_PEOPLE_FIELD.hotkey())
}

pub(super) fn task_projects_dropdown(
    task: &TaskRow,
    projects: &[Project],
    display_id: Rc<RefCell<String>>,
    patch_sink: PatchSink,
) -> Dropdown<Choice, String> {
    let task = task.clone();
    let projects = projects.to_vec();
    Dropdown::single(
        project_choices(&projects),
        |row| row.id.clone(),
        |row| row.label.clone(),
    )
    .label("Project")
    .placeholder("Select project")
    .no_selection_text("None")
    .selected(task.project_id.iter().cloned())
    .search_mode(DropdownSearchMode::Contains)
    .commit_mode(DropdownCommitMode::Explicit)
    .on_select(move |ids| {
        let project_id = ids.into_iter().next();
        let project = project_id
            .as_deref()
            .and_then(|project_id| projects.iter().find(|project| project.id == project_id));
        *display_id.borrow_mut() = task_display_id(&task, project);
        patch_sink.borrow_mut().push(TaskPatch::Project(project_id));
    })
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
