use std::{error::Error, time::Duration};

use crate::app_keymap::{self, keys};
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
};
use tuicore::{
    ActivationMode, AnimationSettings, Button, CellContext, Chip, ChipColorRole, Column, DataView,
    DataViewTypedEvent, Dropdown, DropdownCommitMode, DropdownSearchMode, EventCtx, EventOutcome,
    EventRoute, Flex, FlexItem, FocusCtx, FocusId, FocusRequest, FocusTarget, LayoutCtx,
    LayoutResult, LifecycleCtx, RenderCtx, SelectionMode, SelectionTrigger, Separator, Split,
    StatusBar, Tab, Tabs, TabsVariant, TextInput, TextareaInput, TickResult, Toggle, TuiEvent,
    TuiNode,
};

pub fn run() -> Result<(), Box<dyn Error>> {
    tuicore::try_init()?;
    app_keymap::try_init()?;
    tuicore::run(App::new())?;
    Ok(())
}

struct App {
    root: Flex,
}

impl App {
    fn new() -> Self {
        let tabs = Tabs::new(vec![
            Tab::new("Inbox", InboxWorkspace::new()),
            Tab::new("Tasks", TaskWorkspace::new()),
            Tab::text("Notes", "Clarified reference notes live outside the board."),
            Tab::text("Snoozed", "Hidden clarified items return here when ready."),
            Tab::text("Calendar", "Time-aware planning comes later."),
            Tab::text("Projects", "Project labels are first-class context."),
            Tab::text("Entities", "People, teams, systems, tickets, and docs."),
        ])
        .selected(1)
        .variant(TabsVariant::Underline)
        .bordered(true);

        let root = Flex::column().child("tabs", tabs, FlexItem::fill(1)).child(
            "footer",
            StatusBar::new(),
            FlexItem::fixed(1),
        );

        Self { root }
    }
}

impl TuiNode for App {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.root.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.root.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.root.event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.root.dispatch_event(route, event, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.root.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.root.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.destroy(ctx);
    }
}

struct InboxWorkspace {
    root: Flex,
}

impl InboxWorkspace {
    fn new() -> Self {
        let root = Flex::column()
            .gap(1)
            .child("actions", Button::new("Process"), FlexItem::fixed(1))
            .child(
                "capture",
                TextareaInput::new()
                    .placeholder(
                        "Paste messy email, Slack note, voicemail transcript, or raw thought...",
                    )
                    .hotkey(keys::I.hotkey())
                    .min_rows(8),
                FlexItem::fill(1),
            );

        Self { root }
    }
}

impl TuiNode for InboxWorkspace {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.root.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.root.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.root.event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.root.dispatch_event(route, event, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.root.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.root.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.destroy(ctx);
    }
}

type TaskTable = DataView<TaskRow, &'static str>;
type TaskDetail = TaskDetailForm;

struct TaskWorkspace {
    tasks: Vec<TaskRow>,
    selected_id: &'static str,
    split: Split<TaskTable, TaskDetail>,
}

impl TaskWorkspace {
    fn new() -> Self {
        let tasks = sample_tasks();
        let selected_id = tasks[0].id;
        let table = task_table(tasks.clone(), selected_id);
        let detail = TaskDetailForm::new(&tasks[0]);
        let split = Split::horizontal(table, detail)
            .ratio(65, 35)
            .separator(Separator::new());

        Self {
            tasks,
            selected_id,
            split,
        }
    }

    fn sync_table_events(&mut self, ctx: &mut EventCtx<()>) {
        let events = self.split.first_mut().take_events();
        let mut focus_detail = false;
        let mut selected_changed = false;

        for event in events {
            match event {
                DataViewTypedEvent::HighlightChanged { row_id: Some(id) }
                | DataViewTypedEvent::Activated { row_id: id } => {
                    selected_changed |= self.select_task(id);
                    if matches!(event, DataViewTypedEvent::Activated { .. }) {
                        focus_detail = true;
                    }
                }
                DataViewTypedEvent::HighlightChanged { row_id: None }
                | DataViewTypedEvent::SelectionChanged { .. } => {}
            }
        }

        if selected_changed {
            ctx.request_layout();
            ctx.request_redraw();
        }

        if focus_detail {
            ctx.focus_next();
            ctx.request_redraw();
        }
    }

    fn select_task(&mut self, id: &'static str) -> bool {
        if self.selected_id == id {
            return false;
        }
        let Some(task) = self.tasks.iter().find(|task| task.id == id) else {
            return false;
        };
        self.selected_id = id;
        self.split.second_mut().set_task(task);
        true
    }
}

impl TuiNode for TaskWorkspace {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.split.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.split.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let outcome = self.split.event(event, ctx);
        self.sync_table_events(ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let outcome = self.split.dispatch_event(route, event, ctx);
        self.sync_table_events(ctx);
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.split.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.split.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.split.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.split.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.split.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.split.destroy(ctx);
    }
}

struct TaskDetailForm {
    root: Flex,
}

impl TaskDetailForm {
    fn new(task: &TaskRow) -> Self {
        Self {
            root: detail_form(task),
        }
    }

    fn set_task(&mut self, task: &TaskRow) {
        self.root = detail_form(task);
    }
}

impl TuiNode for TaskDetailForm {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.root.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.root.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let outcome = self.root.event(event, ctx);
        if outcome == EventOutcome::Ignored && detail_escape(event) {
            focus_task_table(ctx);
            return EventOutcome::Handled;
        }
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let outcome = self.root.dispatch_event(route, event, ctx);
        if outcome == EventOutcome::Ignored && detail_escape(event) {
            focus_task_table(ctx);
            return EventOutcome::Handled;
        }
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.root.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.root.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.destroy(ctx);
    }
}

fn task_table(rows: Vec<TaskRow>, selected_id: &'static str) -> DataView<TaskRow, &'static str> {
    DataView::new(rows, |row| row.id)
        .headers(true)
        .focused(true)
        .activation_mode(ActivationMode::OnActivateKey)
        .selection_mode(SelectionMode::Single)
        .selection_trigger(SelectionTrigger::OnNavigate)
        .selected([selected_id])
        .columns(vec![
            Column::rich(
                "state",
                "State",
                Constraint::Percentage(18),
                |row: &TaskRow, _: &CellContext<&'static str>| {
                    chip_line(row.state.label(), row.state.role())
                },
            ),
            Column::text(
                "title",
                "Task",
                Constraint::Percentage(42),
                |row: &TaskRow| row.title.to_string(),
            )
            .sortable(|row| row.title.to_string()),
            Column::rich(
                "size",
                "Size",
                Constraint::Percentage(13),
                |row: &TaskRow, _: &CellContext<&'static str>| {
                    chip_line(row.size.label(), row.size.role())
                },
            ),
            Column::text("due", "Due", Constraint::Percentage(14), |row: &TaskRow| {
                row.due_date.unwrap_or("—").to_string()
            }),
            Column::text(
                "ctx",
                "Context",
                Constraint::Percentage(13),
                |row: &TaskRow| row.project.to_string(),
            ),
        ])
}

fn detail_form(task: &TaskRow) -> Flex {
    let context = Flex::row()
        .gap(1)
        .child(
            "type-chip",
            Chip::new(task.kind.label()).color_role(task.kind.role()),
            FlexItem::fit_content(),
        )
        .child(
            "subtype-chip",
            Chip::new(task.subtype.label()).color_role(task.subtype.role()),
            FlexItem::fit_content(),
        )
        .child(
            "state-chip",
            Chip::new(task.state.label()).color_role(task.state.role()),
            FlexItem::fit_content(),
        )
        .child(
            "size-chip",
            Chip::new(task.size.label()).color_role(task.size.role()),
            FlexItem::fit_content(),
        );

    Flex::column()
        .gap(1)
        .child(
            "title",
            TextInput::new().value(task.title).hotkey(keys::T.hotkey()),
            FlexItem::fixed(1),
        )
        .child("chips", context, FlexItem::fixed(1))
        .child(
            "type",
            dropdown_single("Type", type_choices(), task.kind.id()),
            FlexItem::fixed(3),
        )
        .child(
            "subtype",
            dropdown_single("Subtype", subtype_choices(), task.subtype.id()),
            FlexItem::fixed(3),
        )
        .child(
            "state",
            dropdown_single("State", state_choices(), task.state.id()),
            FlexItem::fixed(3),
        )
        .child(
            "size",
            dropdown_single("Size", size_choices(), task.size.id()),
            FlexItem::fixed(3),
        )
        .child(
            "context",
            dropdown_multi("Context", context_choices(), task.context_ids),
            FlexItem::fixed(3),
        )
        .child(
            "daily",
            Toggle::new("Selected for today / frog candidate")
                .checked(task.focus_today || task.frog_candidate)
                .hotkey(keys::F.hotkey()),
            FlexItem::fixed(1),
        )
        .child(
            "dates",
            TextInput::new()
                .value(format!(
                    "start: {} · due: {}",
                    task.start_date.unwrap_or("—"),
                    task.due_date.unwrap_or("—")
                ))
                .hotkey(keys::D.hotkey()),
            FlexItem::fixed(1),
        )
        .child(
            "notes",
            TextareaInput::new()
                .value(format!(
                    "{}\n\nAI rationale: {}\nTradeoff: {}",
                    task.detail, task.ai_rationale, task.swap_note
                ))
                .hotkey(keys::N.hotkey())
                .min_rows(4)
                .max_rows(8),
            FlexItem::fill(1),
        )
}

fn chip_line(label: &'static str, role: ChipColorRole) -> Line<'static> {
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
        format!(" {label} "),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
}

fn detail_escape(event: &TuiEvent) -> bool {
    app_keymap::matches_any(event, &[keys::ESC, keys::CTRL_LEFT_BRACKET])
}

fn focus_task_table(ctx: &mut EventCtx<()>) {
    ctx.focus(FocusRequest::Target(FocusId::new("data-view")));
    ctx.stop_propagation();
    ctx.request_redraw();
}

fn dropdown_single(
    label: &'static str,
    rows: Vec<Choice>,
    selected: &'static str,
) -> Dropdown<Choice, &'static str> {
    Dropdown::single(rows, |row| row.id, |row| row.label.to_string())
        .label(label)
        .selected_one(selected)
        .search_mode(DropdownSearchMode::Contains)
        .commit_mode(DropdownCommitMode::Immediate)
}

fn dropdown_multi(
    label: &'static str,
    rows: Vec<Choice>,
    selected: &'static [&'static str],
) -> Dropdown<Choice, &'static str> {
    Dropdown::multi(rows, |row| row.id, |row| row.label.to_string())
        .label(label)
        .placeholder("Select context")
        .selected(selected.iter().copied())
        .search_mode(DropdownSearchMode::Contains)
}

#[derive(Debug, Clone)]
struct TaskRow {
    id: &'static str,
    title: &'static str,
    kind: ItemKind,
    subtype: ActionSubtype,
    state: TaskState,
    size: TaskSize,
    start_date: Option<&'static str>,
    due_date: Option<&'static str>,
    project: &'static str,
    context_ids: &'static [&'static str],
    focus_today: bool,
    frog_candidate: bool,
    detail: &'static str,
    ai_rationale: &'static str,
    swap_note: &'static str,
}

#[derive(Debug, Clone, Copy)]
enum ItemKind {
    Action,
    Note,
}

impl ItemKind {
    fn id(self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::Note => "note",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Action => "ACTION",
            Self::Note => "NOTE",
        }
    }

    fn role(self) -> ChipColorRole {
        match self {
            Self::Action => ChipColorRole::Accent,
            Self::Note => ChipColorRole::Muted,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ActionSubtype {
    Task,
    Waiting,
    FollowUp,
    ArtifactUpdate,
}

impl ActionSubtype {
    fn id(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Waiting => "waiting",
            Self::FollowUp => "follow_up",
            Self::ArtifactUpdate => "artifact_update",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Task => "TASK",
            Self::Waiting => "WAITING",
            Self::FollowUp => "FOLLOW-UP",
            Self::ArtifactUpdate => "ARTIFACT",
        }
    }

    fn role(self) -> ChipColorRole {
        match self {
            Self::Task => ChipColorRole::Accent,
            Self::Waiting => ChipColorRole::Warning,
            Self::FollowUp => ChipColorRole::Success,
            Self::ArtifactUpdate => ChipColorRole::Highlight,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum TaskState {
    Clarify,
    Next,
    Doing,
    Waiting,
    Snoozed,
}

impl TaskState {
    fn id(self) -> &'static str {
        match self {
            Self::Clarify => "clarify",
            Self::Next => "next",
            Self::Doing => "doing",
            Self::Waiting => "waiting",
            Self::Snoozed => "snoozed",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Clarify => "CLARIFY",
            Self::Next => "NEXT",
            Self::Doing => "DOING",
            Self::Waiting => "WAITING",
            Self::Snoozed => "SNOOZED",
        }
    }

    fn role(self) -> ChipColorRole {
        match self {
            Self::Clarify => ChipColorRole::Warning,
            Self::Next => ChipColorRole::Accent,
            Self::Doing => ChipColorRole::Success,
            Self::Waiting => ChipColorRole::Warning,
            Self::Snoozed => ChipColorRole::Muted,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum TaskSize {
    Small,
    Medium,
    Big,
}

impl TaskSize {
    fn id(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Big => "big",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Small => "SMALL",
            Self::Medium => "MED",
            Self::Big => "BIG",
        }
    }

    fn role(self) -> ChipColorRole {
        match self {
            Self::Small => ChipColorRole::Success,
            Self::Medium => ChipColorRole::Accent,
            Self::Big => ChipColorRole::Error,
        }
    }
}

#[derive(Debug, Clone)]
struct Choice {
    id: &'static str,
    label: &'static str,
}

fn type_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "action",
            label: "Action",
        },
        Choice {
            id: "note",
            label: "Note",
        },
    ]
}

fn subtype_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "task",
            label: "Task",
        },
        Choice {
            id: "waiting",
            label: "Waiting",
        },
        Choice {
            id: "follow_up",
            label: "Follow-up",
        },
        Choice {
            id: "artifact_update",
            label: "Artifact update",
        },
    ]
}

fn state_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "clarify",
            label: "Clarify",
        },
        Choice {
            id: "next",
            label: "Next",
        },
        Choice {
            id: "doing",
            label: "Doing",
        },
        Choice {
            id: "waiting",
            label: "Waiting",
        },
        Choice {
            id: "snoozed",
            label: "Snoozed",
        },
        Choice {
            id: "done",
            label: "Done",
        },
    ]
}

fn size_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "small",
            label: "Small",
        },
        Choice {
            id: "medium",
            label: "Medium",
        },
        Choice {
            id: "big",
            label: "Big",
        },
    ]
}

fn context_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "alice",
            label: "Alice",
        },
        Choice {
            id: "carter",
            label: "Carter",
        },
        Choice {
            id: "sales",
            label: "Sales",
        },
        Choice {
            id: "legal",
            label: "Legal",
        },
        Choice {
            id: "crm",
            label: "CRM",
        },
        Choice {
            id: "mail",
            label: "Mail",
        },
        Choice {
            id: "docs",
            label: "Docs",
        },
        Choice {
            id: "launch",
            label: "Launch",
        },
        Choice {
            id: "renewal",
            label: "Renewal",
        },
        Choice {
            id: "audit",
            label: "Audit",
        },
    ]
}

fn sample_tasks() -> Vec<TaskRow> {
    vec![
        TaskRow {
            id: "T-101",
            title: "Email Carter for contract redlines",
            kind: ItemKind::Action,
            subtype: ActionSubtype::FollowUp,
            state: TaskState::Next,
            size: TaskSize::Small,
            start_date: Some("today"),
            due_date: Some("Fri"),
            project: "Renewal",
            context_ids: &["carter", "legal", "mail", "renewal"],
            focus_today: true,
            frog_candidate: false,
            detail: "Clarified from a messy Sales note. Needs one concise email asking Carter for redline status and blockers.",
            ai_rationale: "Due date plus named person makes this a concrete follow-up, not a reference note.",
            swap_note: "Small enough to add without removing a big item.",
        },
        TaskRow {
            id: "T-102",
            title: "Draft launch cutover checklist",
            kind: ItemKind::Action,
            subtype: ActionSubtype::ArtifactUpdate,
            state: TaskState::Doing,
            size: TaskSize::Big,
            start_date: Some("today"),
            due_date: Some("Tue"),
            project: "Launch",
            context_ids: &["docs", "launch"],
            focus_today: true,
            frog_candidate: true,
            detail: "Create the first useful checklist pass. Include owners, rollback trigger, comms, and validation steps.",
            ai_rationale: "Large, time-relevant, cross-system artifact: good frog candidate if uninterrupted time exists.",
            swap_note: "If urgent work enters, move a medium item out rather than silently overloading today.",
        },
        TaskRow {
            id: "T-103",
            title: "Wait for Sales owner on pricing question",
            kind: ItemKind::Action,
            subtype: ActionSubtype::Waiting,
            state: TaskState::Waiting,
            size: TaskSize::Small,
            start_date: Some("today"),
            due_date: None,
            project: "CRM",
            context_ids: &["sales", "crm"],
            focus_today: false,
            frog_candidate: false,
            detail: "Track dependency without letting it pollute active doing. Follow up only if no owner appears by tomorrow.",
            ai_rationale: "Waiting is still actionable context, but not a separate top-level item type.",
            swap_note: "Can be snoozed until follow-up date once clarified.",
        },
        TaskRow {
            id: "T-104",
            title: "Clarify voicemail about audit evidence",
            kind: ItemKind::Action,
            subtype: ActionSubtype::Task,
            state: TaskState::Clarify,
            size: TaskSize::Medium,
            start_date: None,
            due_date: Some("next week"),
            project: "Audit",
            context_ids: &["audit"],
            focus_today: false,
            frog_candidate: false,
            detail: "Raw voicemail mentions evidence but lacks owner/system. Needs user review before board pull or snooze.",
            ai_rationale: "Insufficient trust: it has action shape, but missing context prevents silent organization.",
            swap_note: "Do not pull until clarified.",
        },
        TaskRow {
            id: "T-105",
            title: "Review returned docs reminder",
            kind: ItemKind::Note,
            subtype: ActionSubtype::Task,
            state: TaskState::Snoozed,
            size: TaskSize::Medium,
            start_date: Some("tomorrow"),
            due_date: None,
            project: "Docs",
            context_ids: &["docs"],
            focus_today: false,
            frog_candidate: false,
            detail: "Returned-from-snooze marker should appear before user decides whether this is action or reference.",
            ai_rationale: "Snoozed clarified items return to the clarified list, not straight to the board.",
            swap_note: "Hidden work stays safe without cluttering today's focus.",
        },
    ]
}
