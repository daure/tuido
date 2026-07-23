use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, DialogAction, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx,
    FocusTarget, KeySpec, LayoutCtx, LayoutResult, LifecycleCtx, Padding, RenderCtx, TextInput,
    TickResult, TuiEvent, TuiNode,
};

use crate::app::AppMsg;

pub(crate) struct CreateTaskDialog {
    root: Flex<AppMsg>,
    title: Rc<RefCell<String>>,
}

impl CreateTaskDialog {
    pub(crate) fn new() -> Self {
        let title = Rc::new(RefCell::new(String::new()));
        let input = TextInput::new()
            .panel("Title")
            .placeholder("Task title")
            .focused(true)
            .on_change({
                let title = Rc::clone(&title);
                move |value| {
                    *title.borrow_mut() = value;
                    AppMsg::Noop
                }
            });
        let root =
            Flex::column()
                .padding(Padding::all(1))
                .child("title", input, FlexItem::fixed(3));

        Self { root, title }
    }

    pub(crate) fn actions(&self) -> [DialogAction<AppMsg>; 2] {
        let title = Rc::clone(&self.title);
        [
            DialogAction::new("OK")
                .hotkey(KeySpec::plain('o'))
                .on_trigger(move || AppMsg::CreateTaskSubmitted(title.borrow().clone())),
            DialogAction::new("Cancel")
                .hotkey(KeySpec::plain('c'))
                .on_trigger(|| AppMsg::CloseDialog),
        ]
    }
}

impl TuiNode<AppMsg> for CreateTaskDialog {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.root.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.root.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        self.root.event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        self.root.dispatch_event(route, event, ctx)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_is_first_focus_target() {
        let area = Rect::new(0, 0, 50, 10);
        let mut dialog = CreateTaskDialog::new();
        let mut layout = LayoutCtx::new();
        dialog.layout(area, &mut layout);

        assert_eq!(
            layout
                .focus_targets()
                .first()
                .map(|target| target.id.as_str()),
            Some("input")
        );
    }
}
