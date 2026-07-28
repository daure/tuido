use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, Dropdown, DropdownCommitMode, DropdownLabelPosition, DropdownSearchMode,
    DropdownVariant, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusTarget, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult, TuiEvent,
    TuiNode, keybindings,
};

use crate::app::AppMsg;

const MENU_HOST_WIDTH: u16 = 46;
const MENU_HOST_HEIGHT: u16 = 10;
const MENU_FIELD_WIDTH: u16 = 36;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TaskQuickAction {
    Snooze,
    Delete,
    MoveToTop,
    MoveToBottom,
}

impl TaskQuickAction {
    const ALL: [Self; 4] = [
        Self::MoveToTop,
        Self::MoveToBottom,
        Self::Snooze,
        Self::Delete,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Snooze => "Snooze",
            Self::Delete => "Delete",
            Self::MoveToTop => "Move to top",
            Self::MoveToBottom => "Move to bottom",
        }
    }
}

struct TaskQuickOption {
    action: TaskQuickAction,
    label: String,
}

pub(crate) struct TaskQuickMenu {
    task_id: String,
    dropdown: Dropdown<TaskQuickOption, TaskQuickAction>,
    actions: Rc<RefCell<Vec<TaskQuickAction>>>,
    field_area: Rect,
}

impl TaskQuickMenu {
    pub(crate) fn new(task_id: String) -> Self {
        let actions = Rc::new(RefCell::new(Vec::new()));
        let selected_actions = Rc::clone(&actions);
        let options = TaskQuickAction::ALL.map(|action| TaskQuickOption {
            action,
            label: action.label().to_string(),
        });
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
        .max_popup_height(8)
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
            field_area: Rect::default(),
        }
    }

    fn drain_actions(&mut self, ctx: &mut EventCtx<AppMsg>) -> bool {
        let actions = self.actions.borrow_mut().drain(..).collect::<Vec<_>>();
        let handled = !actions.is_empty();
        for action in actions {
            let task_id = self.task_id.clone();
            ctx.emit(match action {
                TaskQuickAction::Snooze => AppMsg::OpenTaskSnooze(task_id),
                TaskQuickAction::Delete => AppMsg::OpenDeleteTask(task_id),
                TaskQuickAction::MoveToTop => AppMsg::MoveTaskToTop(task_id),
                TaskQuickAction::MoveToBottom => AppMsg::MoveTaskToBottom(task_id),
            });
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

    #[test]
    fn quick_menu_is_an_open_centered_dropdown() {
        let mut menu = TaskQuickMenu::new("task-1".into());
        assert!(menu.dropdown.is_open());
        let area = Rect::new(0, 0, MENU_HOST_WIDTH, MENU_HOST_HEIGHT);
        menu.layout(area, &mut LayoutCtx::new());
        assert_eq!(menu.field_area.width, MENU_FIELD_WIDTH);
        assert_eq!(menu.field_area.x, 5);
    }

    #[test]
    fn quick_menu_emits_selected_task_action() {
        let mut menu = TaskQuickMenu::new("task-1".into());
        let mut ctx = EventCtx::default();
        menu.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);

        assert!(matches!(
            ctx.messages(),
            [AppMsg::MoveTaskToTop(task_id)] if task_id == "task-1"
        ));
    }
}
