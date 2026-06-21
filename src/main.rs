use std::{error::Error, time::Duration};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusTarget,
    Key, KeyEvent, KeyModifiers, LayoutCtx, LayoutResult, LifecycleCtx, Panel, TextInput,
    TickResult, TuiEvent, TuiNode,
};

fn main() -> Result<(), Box<dyn Error>> {
    tuicore::init();
    tuicore::run(App::new())?;
    Ok(())
}

struct App {
    root: Flex<()>,
}

impl App {
    fn new() -> Self {
        let input = TextInput::new().placeholder("Type something...");
        let input_panel = Panel::new().top_left("Input").host(input);
        
        let bottom_panel = Panel::new().top_left("Output").content(["Hello, World!"]);
        
        let root = Flex::column()
            .gap(1)
            .child("input", input_panel, FlexItem::fixed(3))
            .child("bottom", bottom_panel, FlexItem::fill(1));
            
        Self { root }
    }

    fn quit_key(event: &TuiEvent) -> bool {
        let TuiEvent::Key(KeyEvent { code, modifiers }) = event else {
            return false;
        };

        matches!(*code, Key::Char(value) if value.eq_ignore_ascii_case(&'q'))
            && modifiers.contains(KeyModifiers::CONTROL)
    }
}

impl TuiNode for App {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.root.layout(area, ctx)
    }

    fn render(&self, frame: &mut Frame, area: Rect) {
        self.root.render(frame, area);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if Self::quit_key(event) {
            ctx.request_quit();
            ctx.stop_propagation();
            EventOutcome::Handled
        } else {
            self.root.event(event, ctx)
        }
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if Self::quit_key(event) {
            ctx.request_quit();
            ctx.stop_propagation();
            EventOutcome::Handled
        } else {
            self.root.dispatch_event(route, event, ctx)
        }
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
