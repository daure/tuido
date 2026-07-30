use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusTarget, Key, KeySpec,
    LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, TextInput,
    TickResult, TuiEvent, TuiNode,
};

use super::{AppMsg, PatchSink};
use crate::{app_keymap::keys, domain::TaskPatch};

pub(super) struct TaskTitleInput {
    input: TextInput<AppMsg>,
    saved_title: Rc<RefCell<String>>,
}

impl TaskTitleInput {
    pub(super) fn new(title: &str, patch_sink: PatchSink) -> Self {
        let saved_title = Rc::new(RefCell::new(title.to_string()));
        let input = TextInput::new()
            .value(title)
            .placeholder("Task title")
            .panel("Title")
            .hotkey(keys::TASK_TITLE_FIELD.hotkey())
            .on_edit_end({
                let saved_title = Rc::clone(&saved_title);
                move |value| {
                    if !value.trim().is_empty() {
                        *saved_title.borrow_mut() = value.clone();
                        patch_sink.borrow_mut().push(TaskPatch::Title(value));
                    }
                    AppMsg::Noop
                }
            });
        Self { input, saved_title }
    }

    fn revert_empty_after_edit_end(
        &mut self,
        event: &TuiEvent,
        was_editing: bool,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let plain_enter =
            matches!(event, TuiEvent::Key(key) if KeySpec::key(Key::Enter).matches(*key));
        let canceled = keys::DETAIL_CLOSE.matches(event) || keys::DETAIL_CLOSE_ALT.matches(event);
        if (plain_enter || canceled)
            && was_editing
            && !self.input.insert_mode()
            && self.input.current_value().trim().is_empty()
        {
            self.input.set_value(self.saved_title.borrow().clone());
            ctx.request_redraw();
        }
    }
}

impl TuiNode<AppMsg> for TaskTitleInput {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.input.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.input.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        <TextInput<AppMsg> as TuiNode<AppMsg>>::render(&self.input, frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        let was_editing = self.input.insert_mode();
        let outcome = self.input.event(event, ctx);
        self.revert_empty_after_edit_end(event, was_editing, ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let was_editing = self.input.insert_mode();
        let outcome = self.input.dispatch_event(route, event, ctx);
        self.revert_empty_after_edit_end(event, was_editing, ctx);
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        self.input.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.input.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.input.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.input.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.input.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.input.destroy(ctx);
    }
}

#[cfg(test)]
mod tests {
    use tuicore::{KeyEvent, KeyModifiers};

    use super::*;

    #[test]
    fn enter_on_empty_title_restores_last_accepted_value_without_patch() {
        let patches = Rc::new(RefCell::new(Vec::new()));
        let mut input = TaskTitleInput::new("Saved title", Rc::clone(&patches));
        let mut ctx = EventCtx::default();
        input.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
        input.input.set_value("  ");

        input.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);

        assert_eq!(input.input.current_value(), "Saved title");
        assert!(patches.borrow().is_empty());
    }

    #[test]
    fn empty_title_restores_most_recent_accepted_value() {
        let patches = Rc::new(RefCell::new(Vec::new()));
        let mut input = TaskTitleInput::new("Original", Rc::clone(&patches));
        let mut ctx = EventCtx::default();
        input.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
        input.input.set_value("Updated");
        input.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
        input.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
        input.input.set_value("");

        input.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);

        assert_eq!(input.input.current_value(), "Updated");
        assert!(matches!(
            patches.borrow().as_slice(),
            [TaskPatch::Title(title)] if title == "Updated"
        ));
    }

    #[test]
    fn canceling_empty_title_restores_last_accepted_value_without_patch() {
        let close_keys = [
            KeyEvent::from(Key::Esc),
            KeyEvent {
                code: Key::Char('['),
                modifiers: KeyModifiers::CONTROL,
            },
        ];

        for key in close_keys {
            let patches = Rc::new(RefCell::new(Vec::new()));
            let mut input = TaskTitleInput::new("Saved title", Rc::clone(&patches));
            let mut ctx = EventCtx::default();
            input.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
            input.input.set_value("  ");

            input.event(&TuiEvent::Key(key), &mut ctx);

            assert_eq!(input.input.current_value(), "Saved title");
            assert!(patches.borrow().is_empty());
        }
    }
}
