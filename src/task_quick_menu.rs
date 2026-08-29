use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect, text::Line};
use time::PrimitiveDateTime;
use tuicore::{
    AnimationSettings, Dropdown, DropdownCommitMode, DropdownLabelPosition, DropdownSearchMode,
    DropdownVariant, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusTarget, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult, TuiEvent,
    TuiNode, keybindings, line_width,
};

use crate::{
    app::AppMsg, app_keymap::keys, domain::TaskState,
    persistence_coordinator::PersistenceSelectionInvocation,
};

const MENU_HOST_WIDTH: u16 = 46;
const MENU_HOST_HEIGHT: u16 = 16;
const MENU_FIELD_WIDTH: u16 = 36;
const MENU_POPUP_WIDTH: u16 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TaskQuickAction {
    CopyExecuteCommand,
    CopyClarifyCommand,
    CopyReference,
    CompleteReject,
    ToggleProgress,
    MarkTodo,
    MarkInProgress,
    Snooze,
    Delete,
    MoveToTop,
    MoveToBottom,
}

impl TaskQuickAction {
    fn available(
        state: TaskState,
        clipboard: &TaskQuickClipboard,
        can_move_to_top: bool,
        can_move_to_bottom: bool,
    ) -> Vec<Self> {
        let mut actions = Vec::new();
        if clipboard.execute.is_some() {
            actions.push(Self::CopyExecuteCommand);
        }
        if clipboard.clarify.is_some() {
            actions.push(Self::CopyClarifyCommand);
        }
        actions.extend([
            Self::CopyReference,
            Self::CompleteReject,
            Self::ToggleProgress,
            Self::Snooze,
            Self::Delete,
        ]);
        if !matches!(state, TaskState::Done | TaskState::Rejected) {
            if can_move_to_top {
                actions.push(Self::MoveToTop);
            }
            if can_move_to_bottom {
                actions.push(Self::MoveToBottom);
            }
        }
        actions
    }

    fn available_multiple(
        task_states: &[TaskState],
        clipboard: &TaskQuickClipboard,
        can_move_to_top: bool,
        can_move_to_bottom: bool,
    ) -> Vec<Self> {
        let mut actions = Vec::new();
        if clipboard.execute.is_some() {
            actions.push(Self::CopyExecuteCommand);
        }
        if clipboard.clarify.is_some() {
            actions.push(Self::CopyClarifyCommand);
        }
        actions.extend([
            Self::CopyReference,
            Self::MarkTodo,
            Self::MarkInProgress,
            Self::Snooze,
            Self::Delete,
            Self::CompleteReject,
        ]);
        if !task_states
            .iter()
            .any(|state| matches!(state, TaskState::Done | TaskState::Rejected))
        {
            if can_move_to_top {
                actions.push(Self::MoveToTop);
            }
            if can_move_to_bottom {
                actions.push(Self::MoveToBottom);
            }
        }
        actions
    }

    fn label(self, state: TaskState) -> &'static str {
        match self {
            Self::CopyExecuteCommand => "Copy execute commands",
            Self::CopyClarifyCommand => "Copy clarify commands",
            Self::CopyReference => "Copy task references",
            Self::CompleteReject => "Complete/reject tasks",
            Self::ToggleProgress if state == TaskState::Todo => "Mark tasks as in progress",
            Self::ToggleProgress => "Mark tasks as todo",
            Self::MarkTodo => "Mark tasks as todo",
            Self::MarkInProgress => "Mark tasks as in progress",
            Self::Snooze => "Snooze tasks",
            Self::Delete => "Delete tasks",
            Self::MoveToTop => "Move tasks to top",
            Self::MoveToBottom => "Move tasks to bottom",
        }
    }

    fn menu_label(self, state: TaskState) -> String {
        let label = self.label(state);
        let Some(hotkey) = self.hotkey() else {
            return label.to_string();
        };
        let gap = usize::from(MENU_POPUP_WIDTH)
            .saturating_sub(
                line_width(&Line::from(label)) + line_width(&Line::from(hotkey.as_str())),
            )
            .max(1);
        format!("{label}{}{hotkey}", " ".repeat(gap))
    }

    fn hotkey(self) -> Option<String> {
        match self {
            Self::CopyExecuteCommand => Some(keys::TASK_AGENT_YANK.label()),
            Self::CopyClarifyCommand => Some(keys::TASK_AGENT_YANK_CLARIFY.label()),
            Self::CopyReference => clipboard_yank_label(),
            Self::CompleteReject => Some(keys::TASK_COMPLETE.label()),
            Self::ToggleProgress => Some(keys::TASK_TOGGLE_PROGRESS.label()),
            Self::MarkTodo | Self::MarkInProgress => None,
            Self::Snooze => Some(keys::TASK_SNOOZE.label()),
            Self::Delete => Some(keys::TASK_DELETE_CTRL_X.label()),
            Self::MoveToTop | Self::MoveToBottom => None,
        }
    }
}

pub(crate) fn clipboard_yank_label() -> Option<String> {
    let bindings = keybindings();
    let sequences = bindings.clipboard().yank_sequences();
    (!sequences.is_empty()).then(|| sequences.join(" / "))
}

pub(crate) struct TaskQuickClipboard {
    pub(crate) execute: Option<String>,
    pub(crate) clarify: Option<String>,
    pub(crate) reference: Option<String>,
}

struct TaskQuickOption {
    action: TaskQuickAction,
    label: String,
}

pub(crate) struct TaskQuickMenu {
    task_ids: Vec<String>,
    multiple: bool,
    dropdown: Dropdown<TaskQuickOption, TaskQuickAction>,
    actions: Rc<RefCell<Vec<TaskQuickAction>>>,
    time: Option<PrimitiveDateTime>,
    clipboard: TaskQuickClipboard,
    selection_invocation: Option<PersistenceSelectionInvocation>,
    field_area: Rect,
}

impl TaskQuickMenu {
    pub(crate) fn new(
        task_id: String,
        state: TaskState,
        clipboard: TaskQuickClipboard,
        can_move_to_top: bool,
        can_move_to_bottom: bool,
    ) -> Self {
        Self::new_with_time(
            task_id,
            state,
            clipboard,
            None,
            can_move_to_top,
            can_move_to_bottom,
        )
    }

    fn new_with_time(
        task_id: String,
        state: TaskState,
        clipboard: TaskQuickClipboard,
        time: Option<PrimitiveDateTime>,
        can_move_to_top: bool,
        can_move_to_bottom: bool,
    ) -> Self {
        let actions = Rc::new(RefCell::new(Vec::new()));
        let selected_actions = Rc::clone(&actions);
        let options =
            TaskQuickAction::available(state, &clipboard, can_move_to_top, can_move_to_bottom)
                .into_iter()
                .map(|action| TaskQuickOption {
                    action,
                    label: action.menu_label(state),
                })
                .collect::<Vec<_>>();
        let mut dropdown = Dropdown::single(
            options,
            |option| option.action,
            |option| option.label.clone(),
        )
        .variant(DropdownVariant::Filled)
        .label("Task actions")
        .label_position(DropdownLabelPosition::Inline)
        .search_mode(DropdownSearchMode::Fuzzy)
        .commit_mode(DropdownCommitMode::Explicit)
        .centered(true)
        .backdrop_amount(0.0)
        .tab_stop(false)
        .max_popup_height(12)
        .on_select(move |ids| {
            if let Some(action) = ids.first() {
                selected_actions.borrow_mut().push(*action);
            }
        });
        dropdown.open();
        Self {
            task_ids: vec![task_id],
            multiple: false,
            dropdown,
            actions,
            time,
            clipboard,
            selection_invocation: None,
            field_area: Rect::default(),
        }
    }

    pub(crate) fn new_multiple(
        task_ids: Vec<String>,
        task_states: Vec<TaskState>,
        clipboard: TaskQuickClipboard,
        can_move_to_top: bool,
        can_move_to_bottom: bool,
    ) -> Self {
        Self::new_multiple_with_time(
            task_ids,
            task_states,
            clipboard,
            None,
            can_move_to_top,
            can_move_to_bottom,
        )
    }

    pub(crate) fn new_calendar_multiple(
        task_ids: Vec<String>,
        task_states: Vec<TaskState>,
        clipboard: TaskQuickClipboard,
        can_move_to_top: bool,
        can_move_to_bottom: bool,
    ) -> Self {
        Self::new_multiple_with_time(
            task_ids,
            task_states,
            clipboard,
            None,
            can_move_to_top,
            can_move_to_bottom,
        )
    }

    pub(crate) fn new_calendar_at_time(
        task_id: String,
        state: TaskState,
        clipboard: TaskQuickClipboard,
        time: PrimitiveDateTime,
        can_move_to_top: bool,
        can_move_to_bottom: bool,
    ) -> Self {
        Self::new_with_time(
            task_id,
            state,
            clipboard,
            Some(time),
            can_move_to_top,
            can_move_to_bottom,
        )
    }

    pub(crate) fn new_calendar_multiple_at_time(
        task_ids: Vec<String>,
        task_states: Vec<TaskState>,
        clipboard: TaskQuickClipboard,
        time: PrimitiveDateTime,
        can_move_to_top: bool,
        can_move_to_bottom: bool,
    ) -> Self {
        Self::new_multiple_with_time(
            task_ids,
            task_states,
            clipboard,
            Some(time),
            can_move_to_top,
            can_move_to_bottom,
        )
    }

    fn new_multiple_with_time(
        task_ids: Vec<String>,
        task_states: Vec<TaskState>,
        clipboard: TaskQuickClipboard,
        time: Option<PrimitiveDateTime>,
        can_move_to_top: bool,
        can_move_to_bottom: bool,
    ) -> Self {
        let actions = Rc::new(RefCell::new(Vec::new()));
        let selected_actions = Rc::clone(&actions);
        let options = TaskQuickAction::available_multiple(
            &task_states,
            &clipboard,
            can_move_to_top,
            can_move_to_bottom,
        )
        .into_iter()
        .map(|action| TaskQuickOption {
            action,
            label: multiple_menu_label(action),
        })
        .collect::<Vec<_>>();
        let mut dropdown = Dropdown::single(
            options,
            |option| option.action,
            |option| option.label.clone(),
        )
        .variant(DropdownVariant::Filled)
        .label("Task actions")
        .label_position(DropdownLabelPosition::Inline)
        .search_mode(DropdownSearchMode::Fuzzy)
        .commit_mode(DropdownCommitMode::Explicit)
        .centered(true)
        .backdrop_amount(0.0)
        .tab_stop(false)
        .max_popup_height(12)
        .on_select(move |ids| {
            if let Some(action) = ids.first() {
                selected_actions.borrow_mut().push(*action);
            }
        });
        dropdown.open();
        Self {
            task_ids,
            multiple: true,
            dropdown,
            actions,
            time,
            clipboard,
            selection_invocation: None,
            field_area: Rect::default(),
        }
    }

    fn copy_to_clipboard(&self, payload: Option<&String>, ctx: &mut EventCtx<AppMsg>) -> bool {
        let Some(payload) = payload else {
            return false;
        };
        self.emit_action(AppMsg::CopyTaskClipboard(payload.clone()), ctx);
        true
    }

    pub(crate) fn set_selection_invocation(
        &mut self,
        selection_invocation: Option<PersistenceSelectionInvocation>,
    ) {
        self.selection_invocation = selection_invocation;
    }

    fn emit_action(&self, action: AppMsg, ctx: &mut EventCtx<AppMsg>) {
        ctx.emit(crate::app::selection_action(
            self.selection_invocation,
            action,
        ));
    }

    fn drain_actions(&mut self, ctx: &mut EventCtx<AppMsg>) -> bool {
        let actions = self.actions.borrow_mut().drain(..).collect::<Vec<_>>();
        let handled = !actions.is_empty();
        for action in actions {
            let task_id = self.task_ids[0].clone();
            let task_ids = self.task_ids.clone();
            let message = match action {
                TaskQuickAction::CopyExecuteCommand => {
                    self.copy_to_clipboard(self.clipboard.execute.as_ref(), ctx);
                    None
                }
                TaskQuickAction::CopyClarifyCommand => {
                    self.copy_to_clipboard(self.clipboard.clarify.as_ref(), ctx);
                    None
                }
                TaskQuickAction::CopyReference => {
                    self.copy_to_clipboard(self.clipboard.reference.as_ref(), ctx);
                    None
                }
                TaskQuickAction::CompleteReject if self.multiple => {
                    Some(AppMsg::OpenCompleteTasks(task_ids))
                }
                TaskQuickAction::CompleteReject => Some(AppMsg::OpenCompleteTask {
                    task_id,
                    return_focus: None,
                }),
                TaskQuickAction::ToggleProgress => Some(AppMsg::ToggleTaskProgress(task_id)),
                TaskQuickAction::MarkTodo => Some(AppMsg::CompleteTasks {
                    task_ids,
                    state: TaskState::Todo,
                }),
                TaskQuickAction::MarkInProgress => Some(AppMsg::CompleteTasks {
                    task_ids,
                    state: TaskState::InProgress,
                }),
                TaskQuickAction::Snooze if self.multiple => Some(AppMsg::OpenTasksSnooze(task_ids)),
                TaskQuickAction::Snooze => Some(AppMsg::OpenTaskSnooze {
                    task_id,
                    return_focus: None,
                }),
                TaskQuickAction::Delete if self.multiple => Some(AppMsg::OpenDeleteTasks(task_ids)),
                TaskQuickAction::Delete => Some(AppMsg::OpenDeleteTask {
                    task_id,
                    return_focus: None,
                }),
                TaskQuickAction::MoveToTop if self.multiple => Some(
                    self.time
                        .map_or(AppMsg::MoveTasksToTop(task_ids.clone()), |time| {
                            AppMsg::MoveTasksToTopAtTime { task_ids, time }
                        }),
                ),
                TaskQuickAction::MoveToTop => Some(
                    self.time
                        .map_or(AppMsg::MoveTaskToTop(task_id.clone()), |time| {
                            AppMsg::MoveTaskToTopAtTime { task_id, time }
                        }),
                ),
                TaskQuickAction::MoveToBottom if self.multiple => Some(
                    self.time
                        .map_or(AppMsg::MoveTasksToBottom(task_ids.clone()), |time| {
                            AppMsg::MoveTasksToBottomAtTime { task_ids, time }
                        }),
                ),
                TaskQuickAction::MoveToBottom => Some(
                    self.time
                        .map_or(AppMsg::MoveTaskToBottom(task_id.clone()), |time| {
                            AppMsg::MoveTaskToBottomAtTime { task_id, time }
                        }),
                ),
            };
            if let Some(message) = message {
                self.emit_action(message, ctx);
            }
            if action == TaskQuickAction::ToggleProgress {
                ctx.emit(AppMsg::CloseDialog);
            }
        }
        handled
    }

    fn centered_field_area(&self, area: Rect) -> Rect {
        let width = MENU_FIELD_WIDTH.min(area.width);
        let hint = <Dropdown<TaskQuickOption, TaskQuickAction> as TuiNode<AppMsg>>::measure(
            &self.dropdown,
            LayoutProposal::at_most(width, area.height),
        );
        let height = hint.preferred.height.min(area.height);
        Rect::new(
            area.x.saturating_add(area.width.saturating_sub(width) / 2),
            area.y
                .saturating_add(area.height.saturating_sub(height) / 2),
            width,
            height,
        )
    }

    fn copy_hotkey(&self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> bool {
        match event {
            TuiEvent::Yank => self.copy_to_clipboard(self.clipboard.reference.as_ref(), ctx),
            TuiEvent::Hotkey(tuicore::HotkeyEvent::Commit(sequence))
                if sequence == &keys::TASK_AGENT_YANK.hotkey() =>
            {
                self.copy_to_clipboard(self.clipboard.execute.as_ref(), ctx)
            }
            TuiEvent::Hotkey(tuicore::HotkeyEvent::Commit(sequence))
                if sequence == &keys::TASK_AGENT_YANK_CLARIFY.hotkey() =>
            {
                self.copy_to_clipboard(self.clipboard.clarify.as_ref(), ctx)
            }
            _ => false,
        }
    }

    fn finish_event(
        &mut self,
        was_open: bool,
        outcome: EventOutcome,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let activated = self.drain_actions(ctx);
        if was_open && !self.dropdown.is_open() && !activated {
            self.emit_action(AppMsg::CloseDialog, ctx);
        }
        outcome
    }
}

fn multiple_menu_label(action: TaskQuickAction) -> String {
    action.label(TaskState::Todo).to_string()
}

impl TuiNode<AppMsg> for TaskQuickMenu {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        LayoutSizeHint::content(MENU_HOST_WIDTH, MENU_HOST_HEIGHT).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.field_area = self.centered_field_area(area);
        <Dropdown<TaskQuickOption, TaskQuickAction> as TuiNode<AppMsg>>::layout(
            &mut self.dropdown,
            self.field_area,
            ctx,
        );
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, _area: Rect, ctx: &mut RenderCtx<'a>) {
        self.dropdown.render(frame, self.field_area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        if self.copy_hotkey(event, ctx) {
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        if let TuiEvent::Key(key) = event
            && keybindings().focus().unfocus_matches(*key)
        {
            self.emit_action(AppMsg::CloseDialog, ctx);
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        let was_open = self.dropdown.is_open();
        let outcome = self.dropdown.event(event, ctx);
        self.finish_event(was_open, outcome, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        if self.copy_hotkey(event, ctx) {
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        let was_open = self.dropdown.is_open();
        let outcome = self.dropdown.dispatch_event(route, event, ctx);
        self.finish_event(was_open, outcome, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        self.dropdown.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        <Dropdown<TaskQuickOption, TaskQuickAction> as TuiNode<AppMsg>>::tick(
            &mut self.dropdown,
            dt,
            settings,
        )
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.dropdown.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.dropdown.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.dropdown.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.dropdown.destroy(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tuicore::{Key, KeyEvent};

    fn clipboard() -> TaskQuickClipboard {
        TaskQuickClipboard {
            execute: Some("execute".into()),
            clarify: Some("clarify".into()),
            reference: Some("reference".into()),
        }
    }

    #[test]
    fn quick_menu_aligns_task_shortcuts_at_popup_edge() {
        for action in [TaskQuickAction::Snooze, TaskQuickAction::Delete] {
            let label = action.menu_label(TaskState::Todo);
            let hotkey = action.hotkey().expect("action should have a shortcut");

            assert!(label.starts_with(action.label(TaskState::Todo)));
            assert!(label.ends_with(&hotkey));
            assert_eq!(
                line_width(&Line::from(label.as_str())),
                usize::from(MENU_POPUP_WIDTH)
            );
        }
        assert_eq!(
            TaskQuickAction::Delete.hotkey().as_deref(),
            Some(keys::TASK_DELETE_CTRL_X.label().as_str())
        );
    }

    #[test]
    fn quick_menu_copies_selected_task_execute_command() {
        let mut menu =
            TaskQuickMenu::new("task-1".into(), TaskState::Todo, clipboard(), true, true);
        let mut ctx = EventCtx::default();
        menu.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);

        assert_eq!(ctx.clipboard_request(), None);
        assert!(matches!(
            ctx.messages(),
            [AppMsg::CopyTaskClipboard(payload)] if payload == "execute"
        ));
    }

    #[test]
    fn transient_group_copy_emits_a_semantic_clipboard_request() {
        let mut menu = TaskQuickMenu::new_multiple(
            vec!["task-1".into()],
            vec![TaskState::Todo],
            clipboard(),
            false,
            false,
        );
        menu.actions
            .borrow_mut()
            .push(TaskQuickAction::CopyReference);
        let mut ctx = EventCtx::default();

        assert!(menu.drain_actions(&mut ctx));
        assert_eq!(ctx.clipboard_request(), None);
        assert!(matches!(
            ctx.messages(),
            [AppMsg::CopyTaskClipboard(payload)]
                if payload == "reference"
        ));
    }

    #[test]
    fn quick_menu_yank_emits_the_same_semantic_reference_request() {
        let mut menu =
            TaskQuickMenu::new("task-1".into(), TaskState::Todo, clipboard(), true, true);
        let mut ctx = EventCtx::default();

        assert!(menu.event(&TuiEvent::Yank, &mut ctx).handled());

        assert!(matches!(
            ctx.messages(),
            [AppMsg::CopyTaskClipboard(payload)] if payload == "reference"
        ));
    }

    #[test]
    fn quick_menu_agent_hotkeys_emit_semantic_clipboard_requests() {
        for (event, expected) in [
            (
                TuiEvent::Hotkey(tuicore::HotkeyEvent::Commit(keys::TASK_AGENT_YANK.hotkey())),
                "execute",
            ),
            (
                TuiEvent::Hotkey(tuicore::HotkeyEvent::Commit(
                    keys::TASK_AGENT_YANK_CLARIFY.hotkey(),
                )),
                "clarify",
            ),
        ] {
            let mut menu =
                TaskQuickMenu::new("task-1".into(), TaskState::Todo, clipboard(), true, true);
            let mut ctx = EventCtx::default();

            assert!(menu.event(&event, &mut ctx).handled());
            assert!(matches!(
                ctx.messages(),
                [AppMsg::CopyTaskClipboard(payload)] if payload == expected
            ));
        }
    }

    #[test]
    fn calendar_quick_menu_scopes_move_actions_to_the_entry_time() {
        let time = time::macros::datetime!(2026-07-31 8:00);
        let mut menu = TaskQuickMenu::new_calendar_at_time(
            "task-1".into(),
            TaskState::Snoozed,
            clipboard(),
            time,
            true,
            true,
        );
        menu.actions.borrow_mut().push(TaskQuickAction::MoveToTop);
        let mut ctx = EventCtx::default();

        menu.drain_actions(&mut ctx);

        assert!(matches!(
            ctx.messages(),
            [AppMsg::MoveTaskToTopAtTime {
                task_id,
                time: entry_time,
            }] if task_id == "task-1" && *entry_time == time
        ));
    }

    #[test]
    fn calendar_multi_quick_menu_scopes_group_moves_to_the_entry_time() {
        let time = time::macros::datetime!(2026-07-31 8:00);
        let mut menu = TaskQuickMenu::new_calendar_multiple_at_time(
            vec!["first".into(), "second".into()],
            vec![TaskState::Snoozed, TaskState::Snoozed],
            clipboard(),
            time,
            true,
            true,
        );
        menu.actions
            .borrow_mut()
            .push(TaskQuickAction::MoveToBottom);
        let mut ctx = EventCtx::default();

        menu.drain_actions(&mut ctx);

        assert!(matches!(
            ctx.messages(),
            [AppMsg::MoveTasksToBottomAtTime {
                task_ids,
                time: entry_time,
            }] if task_ids == &vec!["first".to_string(), "second".to_string()] && *entry_time == time
        ));
    }

    #[test]
    fn calendar_transient_single_selection_uses_group_progress_action() {
        let mut menu = TaskQuickMenu::new_calendar_multiple_at_time(
            vec!["task-1".into()],
            vec![TaskState::Snoozed],
            clipboard(),
            time::macros::datetime!(2026-07-31 8:00),
            false,
            false,
        );
        menu.actions
            .borrow_mut()
            .push(TaskQuickAction::MarkInProgress);
        let mut ctx = EventCtx::default();

        assert!(menu.drain_actions(&mut ctx));

        assert!(matches!(
            ctx.messages(),
            [AppMsg::CompleteTasks { task_ids, state: TaskState::InProgress }]
                if task_ids == &vec!["task-1".to_string()]
        ));
    }

    #[test]
    fn quick_menu_delete_has_no_detail_return_focus() {
        let mut menu =
            TaskQuickMenu::new("task-1".into(), TaskState::Todo, clipboard(), true, true);
        menu.actions.borrow_mut().push(TaskQuickAction::Delete);
        let mut ctx = EventCtx::default();

        assert!(menu.drain_actions(&mut ctx));

        assert!(matches!(
            ctx.messages(),
            [AppMsg::OpenDeleteTask {
                task_id,
                return_focus: None,
            }] if task_id == "task-1"
        ));
    }

    #[test]
    fn calendar_quick_menu_emits_a_source_independent_delete_request() {
        let mut menu = TaskQuickMenu::new_calendar_at_time(
            "task-1".into(),
            TaskState::Snoozed,
            clipboard(),
            time::macros::datetime!(2026-07-31 8:00),
            true,
            true,
        );
        menu.actions.borrow_mut().push(TaskQuickAction::Delete);
        let mut ctx = EventCtx::default();

        assert!(menu.drain_actions(&mut ctx));

        assert!(matches!(
            ctx.messages(),
            [AppMsg::OpenDeleteTask { task_id, return_focus: None }]
                if task_id == "task-1"
        ));
    }

    #[test]
    fn multi_task_quick_menu_copies_joined_references_and_marks_the_group() {
        let mut menu = TaskQuickMenu::new_multiple(
            vec!["first".into(), "second".into()],
            vec![TaskState::Todo, TaskState::Todo],
            TaskQuickClipboard {
                execute: Some("Tuido execute FIRST-1 \"First\"; SECOND-2 \"Second\"".into()),
                clarify: Some("Tuido clarify FIRST-1 \"First\"; SECOND-2 \"Second\"".into()),
                reference: Some("Tuido FIRST-1 \"First\"; SECOND-2 \"Second\"".into()),
            },
            true,
            true,
        );
        menu.actions
            .borrow_mut()
            .push(TaskQuickAction::CopyExecuteCommand);
        let mut execute_ctx = EventCtx::default();

        assert!(menu.drain_actions(&mut execute_ctx));
        assert!(matches!(
            execute_ctx.messages(),
            [AppMsg::CopyTaskClipboard(payload)]
                if payload == "Tuido execute FIRST-1 \"First\"; SECOND-2 \"Second\""
        ));

        menu.actions
            .borrow_mut()
            .push(TaskQuickAction::CopyClarifyCommand);
        let mut clarify_ctx = EventCtx::default();

        assert!(menu.drain_actions(&mut clarify_ctx));
        assert!(matches!(
            clarify_ctx.messages(),
            [AppMsg::CopyTaskClipboard(payload)]
                if payload == "Tuido clarify FIRST-1 \"First\"; SECOND-2 \"Second\""
        ));

        menu.actions
            .borrow_mut()
            .push(TaskQuickAction::CopyReference);
        let mut copy_ctx = EventCtx::default();

        assert!(menu.drain_actions(&mut copy_ctx));
        assert!(matches!(
            copy_ctx.messages(),
            [AppMsg::CopyTaskClipboard(payload)]
                if payload == "Tuido FIRST-1 \"First\"; SECOND-2 \"Second\""
        ));

        menu.actions
            .borrow_mut()
            .push(TaskQuickAction::MarkInProgress);
        let mut state_ctx = EventCtx::default();

        assert!(menu.drain_actions(&mut state_ctx));
        assert!(matches!(
            state_ctx.messages(),
            [AppMsg::CompleteTasks { task_ids, state: TaskState::InProgress }]
                if task_ids == &vec!["first".to_string(), "second".to_string()]
        ));
    }

    #[test]
    fn selection_menu_action_carries_its_invocation_and_original_ids() {
        let invocation = PersistenceSelectionInvocation {
            token: 7,
            source: crate::persistence_coordinator::PersistenceSelectionSource::TaskList,
        };
        let mut menu = TaskQuickMenu::new_multiple(
            vec!["first".into(), "second".into()],
            vec![TaskState::Todo],
            clipboard(),
            false,
            false,
        );
        menu.set_selection_invocation(Some(invocation));
        menu.actions
            .borrow_mut()
            .push(TaskQuickAction::MarkInProgress);
        let mut ctx = EventCtx::default();

        assert!(menu.drain_actions(&mut ctx));

        assert!(matches!(
            ctx.messages(),
            [AppMsg::SelectionAction {
                invocation: message_invocation,
                action,
            }]
                if *message_invocation == invocation
                    && matches!(
                        action.as_ref(),
                        AppMsg::CompleteTasks {
                            task_ids,
                            state: TaskState::InProgress,
                        } if task_ids == &vec!["first".to_string(), "second".to_string()]
                    )
        ));
    }

    #[test]
    fn quick_menu_hides_unavailable_copy_and_edge_actions() {
        let no_commands = TaskQuickClipboard {
            execute: None,
            clarify: None,
            reference: Some("reference".into()),
        };
        let done_actions = TaskQuickAction::available(TaskState::Done, &clipboard(), true, true);
        let calendar_actions =
            TaskQuickAction::available(TaskState::Snoozed, &no_commands, false, true);

        assert!(!calendar_actions.contains(&TaskQuickAction::CopyExecuteCommand));
        assert!(!calendar_actions.contains(&TaskQuickAction::CopyClarifyCommand));
        assert!(!done_actions.contains(&TaskQuickAction::MoveToTop));
        assert!(!done_actions.contains(&TaskQuickAction::MoveToBottom));
        let multi_actions = TaskQuickAction::available_multiple(
            &[TaskState::Todo, TaskState::Done],
            &clipboard(),
            true,
            true,
        );
        assert!(!multi_actions.contains(&TaskQuickAction::MoveToTop));
        assert!(!multi_actions.contains(&TaskQuickAction::MoveToBottom));
        assert!(!calendar_actions.contains(&TaskQuickAction::MoveToTop));
        assert!(calendar_actions.contains(&TaskQuickAction::MoveToBottom));
    }

    #[test]
    fn quick_menu_progress_label_matches_task_state() {
        assert_eq!(
            TaskQuickAction::ToggleProgress.label(TaskState::Todo),
            "Mark tasks as in progress"
        );
        for state in [
            TaskState::Backlog,
            TaskState::InProgress,
            TaskState::Snoozed,
            TaskState::Done,
            TaskState::Rejected,
        ] {
            assert_eq!(
                TaskQuickAction::ToggleProgress.label(state),
                "Mark tasks as todo"
            );
        }
    }

    #[test]
    fn quick_menu_uses_shared_plural_action_labels() {
        assert_eq!(
            TaskQuickAction::CopyExecuteCommand.label(TaskState::Todo),
            "Copy execute commands"
        );
        assert_eq!(
            TaskQuickAction::CopyClarifyCommand.label(TaskState::Todo),
            "Copy clarify commands"
        );
        assert_eq!(
            TaskQuickAction::CopyReference.label(TaskState::Todo),
            "Copy task references"
        );
        assert_eq!(
            TaskQuickAction::CompleteReject.label(TaskState::Todo),
            "Complete/reject tasks"
        );
        assert_eq!(
            TaskQuickAction::Snooze.label(TaskState::Todo),
            "Snooze tasks"
        );
        assert_eq!(
            TaskQuickAction::Delete.label(TaskState::Todo),
            "Delete tasks"
        );
        assert_eq!(
            TaskQuickAction::MoveToTop.label(TaskState::Todo),
            "Move tasks to top"
        );
        assert_eq!(
            TaskQuickAction::MoveToBottom.label(TaskState::Todo),
            "Move tasks to bottom"
        );
    }

    #[test]
    fn calendar_quick_menu_opens_completion_with_calendar_return_focus() {
        let mut menu = TaskQuickMenu::new_calendar_at_time(
            "task-1".into(),
            TaskState::Snoozed,
            clipboard(),
            time::macros::datetime!(2026-07-31 8:00),
            true,
            true,
        );
        menu.actions
            .borrow_mut()
            .push(TaskQuickAction::CompleteReject);
        let mut ctx = EventCtx::default();

        assert!(menu.drain_actions(&mut ctx));

        assert!(matches!(
            ctx.messages(),
            [AppMsg::OpenCompleteTask { task_id, return_focus: None }] if task_id == "task-1"
        ));
    }

    #[test]
    fn calendar_quick_menu_emits_a_source_independent_progress_request() {
        let mut menu = TaskQuickMenu::new_calendar_at_time(
            "task-1".into(),
            TaskState::Snoozed,
            clipboard(),
            time::macros::datetime!(2026-07-31 8:00),
            true,
            true,
        );
        menu.actions
            .borrow_mut()
            .push(TaskQuickAction::ToggleProgress);
        let mut ctx = EventCtx::default();

        assert!(menu.drain_actions(&mut ctx));

        assert!(matches!(
            ctx.messages(),
            [AppMsg::ToggleTaskProgress(task_id), AppMsg::CloseDialog]
                if task_id == "task-1"
        ));
    }
}
