use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect, text::Line};
use time::PrimitiveDateTime;
use tuicore::{
    AnimationSettings, Dropdown, DropdownCommitMode, DropdownLabelPosition, DropdownSearchMode,
    DropdownVariant, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusTarget, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult, TuiEvent,
    TuiNode, keybindings, line_width,
};

use crate::{app::AppMsg, app_keymap::keys, domain::TaskState};

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

    fn label(self, state: TaskState) -> &'static str {
        match self {
            Self::CopyExecuteCommand => "Copy execute command",
            Self::CopyClarifyCommand => "Copy clarify command",
            Self::CopyReference => "Copy task reference",
            Self::CompleteReject => "Complete/reject",
            Self::ToggleProgress if state == TaskState::Todo => "Start progress",
            Self::ToggleProgress => "Mark as todo",
            Self::Snooze => "Snooze",
            Self::Delete => "Delete",
            Self::MoveToTop => "Move to top",
            Self::MoveToBottom => "Move to bottom",
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
    task_id: String,
    dropdown: Dropdown<TaskQuickOption, TaskQuickAction>,
    actions: Rc<RefCell<Vec<TaskQuickAction>>>,
    time: Option<PrimitiveDateTime>,
    clipboard: TaskQuickClipboard,
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

    pub(crate) fn new_at_time(
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
            task_id,
            dropdown,
            actions,
            time,
            clipboard,
            field_area: Rect::default(),
        }
    }

    fn drain_actions(&mut self, ctx: &mut EventCtx<AppMsg>) -> bool {
        let actions = self.actions.borrow_mut().drain(..).collect::<Vec<_>>();
        let handled = !actions.is_empty();
        for action in actions {
            let task_id = self.task_id.clone();
            let message = match action {
                TaskQuickAction::CopyExecuteCommand => {
                    if let Some(payload) = &self.clipboard.execute {
                        ctx.copy_to_clipboard(payload.clone());
                    }
                    Some(AppMsg::CloseDialog)
                }
                TaskQuickAction::CopyClarifyCommand => {
                    if let Some(payload) = &self.clipboard.clarify {
                        ctx.copy_to_clipboard(payload.clone());
                    }
                    Some(AppMsg::CloseDialog)
                }
                TaskQuickAction::CopyReference => {
                    if let Some(payload) = &self.clipboard.reference {
                        ctx.copy_to_clipboard(payload.clone());
                    }
                    Some(AppMsg::CloseDialog)
                }
                TaskQuickAction::CompleteReject => Some(self.time.map_or(
                    AppMsg::OpenCompleteTask {
                        task_id: task_id.clone(),
                        return_focus: None,
                    },
                    |_| AppMsg::OpenCalendarQuickCompleteTask(task_id),
                )),
                TaskQuickAction::ToggleProgress => Some(AppMsg::ToggleTaskProgress(task_id)),
                TaskQuickAction::Snooze => Some(AppMsg::OpenTaskSnooze {
                    task_id,
                    return_focus: None,
                }),
                TaskQuickAction::Delete => Some(AppMsg::OpenDeleteTask {
                    task_id,
                    return_focus: None,
                }),
                TaskQuickAction::MoveToTop => Some(
                    self.time
                        .map_or(AppMsg::MoveTaskToTop(task_id.clone()), |time| {
                            AppMsg::MoveCalendarTaskToTop { task_id, time }
                        }),
                ),
                TaskQuickAction::MoveToBottom => Some(
                    self.time
                        .map_or(AppMsg::MoveTaskToBottom(task_id.clone()), |time| {
                            AppMsg::MoveCalendarTaskToBottom { task_id, time }
                        }),
                ),
            };
            if let Some(message) = message {
                ctx.emit(message);
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

    fn finish_event(
        &mut self,
        was_open: bool,
        outcome: EventOutcome,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let activated = self.drain_actions(ctx);
        if was_open && !self.dropdown.is_open() && !activated {
            ctx.emit(AppMsg::CloseDialog);
        }
        outcome
    }
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
        if let TuiEvent::Key(key) = event
            && keybindings().focus().unfocus_matches(*key)
        {
            ctx.emit(AppMsg::CloseDialog);
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

        assert_eq!(ctx.clipboard_request(), Some("execute"));
        assert!(matches!(ctx.messages(), [AppMsg::CloseDialog]));
    }

    #[test]
    fn calendar_quick_menu_scopes_move_actions_to_the_entry_time() {
        let time = time::macros::datetime!(2026-07-31 8:00);
        let mut menu = TaskQuickMenu::new_at_time(
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
            [AppMsg::MoveCalendarTaskToTop {
                task_id,
                time: entry_time,
            }] if task_id == "task-1" && *entry_time == time
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
        assert!(!calendar_actions.contains(&TaskQuickAction::MoveToTop));
        assert!(calendar_actions.contains(&TaskQuickAction::MoveToBottom));
    }

    #[test]
    fn quick_menu_progress_label_matches_task_state() {
        assert_eq!(
            TaskQuickAction::ToggleProgress.label(TaskState::Todo),
            "Start progress"
        );
        for state in [
            TaskState::Backlog,
            TaskState::InProgress,
            TaskState::Snoozed,
            TaskState::Done,
            TaskState::Rejected,
        ] {
            assert_eq!(TaskQuickAction::ToggleProgress.label(state), "Mark as todo");
        }
    }

    #[test]
    fn calendar_quick_menu_opens_completion_with_calendar_return_focus() {
        let mut menu = TaskQuickMenu::new_at_time(
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
            [AppMsg::OpenCalendarQuickCompleteTask(task_id)] if task_id == "task-1"
        ));
    }
}
