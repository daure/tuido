use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusTarget, Key,
    KeyModifiers, KeySpec, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    RenderCtx, TextInput, TickResult, TuiEvent, TuiNode,
};

use crate::task_title::format_title;

pub(crate) struct TitleInput<M> {
    input: TextInput<M>,
    title: Rc<RefCell<String>>,
    on_ctrl_enter: Option<Box<dyn Fn(String) -> M>>,
}

impl<M> TitleInput<M> {
    pub(crate) fn new(input: TextInput<M>, title: Rc<RefCell<String>>) -> Self {
        Self {
            input,
            title,
            on_ctrl_enter: None,
        }
    }

    pub(crate) fn on_ctrl_enter(mut self, callback: impl Fn(String) -> M + 'static) -> Self {
        self.on_ctrl_enter = Some(Box::new(callback));
        self
    }

    fn format_after_enter(&mut self, event: &TuiEvent, was_editing: bool, ctx: &mut EventCtx<M>) {
        let TuiEvent::Key(key) = event else {
            return;
        };
        let plain_enter = KeySpec::key(Key::Enter).matches(*key);
        let ctrl_enter =
            KeySpec::key_with_modifiers(Key::Enter, KeyModifiers::CONTROL).matches(*key);
        if (!plain_enter && !ctrl_enter) || !was_editing || self.input.insert_mode() {
            return;
        }

        let formatted = format_title(self.input.current_value());
        self.input.set_value(formatted.clone());
        *self.title.borrow_mut() = formatted.clone();
        ctx.request_redraw();
        if ctrl_enter && let Some(callback) = &self.on_ctrl_enter {
            ctx.emit(callback(formatted));
        }
    }
}

impl<M> TuiNode<M> for TitleInput<M> {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.input.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.input.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        TuiNode::render(&self.input, frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<M>) -> EventOutcome {
        let was_editing = self.input.insert_mode();
        let outcome = self.input.event(event, ctx);
        self.format_after_enter(event, was_editing, ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<M>,
    ) -> EventOutcome {
        let was_editing = self.input.insert_mode();
        let outcome = self.input.dispatch_event(route, event, ctx);
        self.format_after_enter(event, was_editing, ctx);
        outcome
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.input.tick(dt, settings)
    }

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<M>) {
        self.input.focus(target, focused, ctx);
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<M>) {
        self.input.dispatch_focus(target, focused, ctx);
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.input.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.input.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.input.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.input.destroy(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tuicore::KeyEvent;

    #[test]
    fn enter_activating_inactive_input_does_not_format_title() {
        let raw = "fix   dont crash...";
        let title = Rc::new(RefCell::new(raw.to_string()));
        let input = TextInput::<()>::new().value(raw).focused(true);
        let mut input = TitleInput::new(input, Rc::clone(&title));

        input.event(
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );

        assert!(input.input.insert_mode());
        assert_eq!(input.input.current_value(), raw);
        assert_eq!(title.borrow().as_str(), raw);
    }
}
