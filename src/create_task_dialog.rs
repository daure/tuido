use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, DialogAction, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx,
    FocusTarget, Key, KeyEvent, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint,
    LifecycleCtx, RenderCtx, TextInput, TickResult, TuiEvent, TuiNode,
};

use crate::{
    app::AppMsg, app_keymap::keys, title_feedback::TitleFeedback, title_input::TitleInput,
};

#[derive(Debug, Clone)]
pub(crate) struct CreateTaskDraft {
    pub(crate) title: String,
}

pub(crate) struct CreateTaskDialog {
    root: Flex<AppMsg>,
    title: Rc<RefCell<String>>,
}

impl CreateTaskDialog {
    pub(crate) fn new() -> Self {
        let title = Rc::new(RefCell::new(String::new()));
        let mut input = TextInput::new()
            .placeholder("Title")
            .focused(true)
            .on_change({
                let title = Rc::clone(&title);
                move |value| {
                    *title.borrow_mut() = value;
                    AppMsg::Noop
                }
            });
        input.event(
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        let input = TitleInput::new(input, Rc::clone(&title))
            .submit_hotkey(keys::DIALOG_SUBMIT.key_spec())
            .on_ctrl_enter(|title| AppMsg::CreateTaskSubmitted(CreateTaskDraft { title }));
        let feedback = TitleFeedback::new(Rc::clone(&title));
        let root = Flex::column()
            .gap(1)
            .child("title", input, FlexItem::content())
            .child("feedback", feedback, FlexItem::content());

        Self { root, title }
    }

    pub(crate) fn actions(&self) -> [DialogAction<AppMsg>; 2] {
        let title = Rc::clone(&self.title);
        [
            DialogAction::new("OK")
                .hotkey(keys::DIALOG_OK.key_spec())
                .on_trigger(move || {
                    AppMsg::CreateTaskSubmitted(CreateTaskDraft {
                        title: title.borrow().clone(),
                    })
                }),
            DialogAction::new("Cancel")
                .hotkey(keys::DIALOG_CANCEL.key_spec())
                .on_trigger(|| AppMsg::CloseDialog),
        ]
    }
}

impl TuiNode<AppMsg> for CreateTaskDialog {
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
    use ratatui::{Terminal, backend::TestBackend};
    use tuicore::{ChildKey, FocusTarget, Key, KeyEvent, KeyModifiers, LayoutProposal};

    const AREA: Rect = Rect::new(0, 0, 50, 20);

    fn focus_title(dialog: &mut CreateTaskDialog) -> FocusTarget {
        let mut layout = LayoutCtx::new();
        dialog.layout(AREA, &mut layout);
        let target = layout
            .focus_targets()
            .first()
            .expect("title should be first focus target")
            .clone();
        dialog.dispatch_focus(&target, true, &mut FocusCtx::default());
        target
    }

    fn type_title(dialog: &mut CreateTaskDialog, target: &FocusTarget, value: &str) {
        for character in value.chars() {
            let outcome = dialog.dispatch_event(
                &EventRoute::new(target.path.clone()),
                &TuiEvent::Key(KeyEvent::from(Key::Char(character))),
                &mut EventCtx::default(),
            );
            assert!(outcome.handled());
        }
    }

    fn rendered(dialog: &CreateTaskDialog) -> String {
        let mut terminal = Terminal::new(TestBackend::new(AREA.width, AREA.height)).unwrap();
        terminal
            .draw(|frame| dialog.render(frame, AREA, &mut RenderCtx::new()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn preferred_height_includes_one_row_between_title_and_feedback() {
        let mut dialog = CreateTaskDialog::new();
        let preferred = dialog.root.measure(LayoutProposal::unbounded()).preferred;
        dialog.layout(
            Rect::new(0, 0, preferred.width, preferred.height),
            &mut LayoutCtx::new(),
        );
        let title = dialog.root.child_rect(&ChildKey::from("title")).unwrap();
        let feedback = dialog.root.child_rect(&ChildKey::from("feedback")).unwrap();

        assert_eq!(feedback.y, title.y + title.height + 1);
        assert_eq!(title.height, 1);
        assert_eq!(feedback.height, 3);
        assert_eq!(preferred.height, title.height + 1 + feedback.height);
        assert_eq!(preferred.height, 5);
    }

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

    #[test]
    fn title_accepts_typing_immediately_after_focus() {
        let mut dialog = CreateTaskDialog::new();
        let target = focus_title(&mut dialog);

        let outcome = dialog.dispatch_event(
            &EventRoute::new(target.path),
            &TuiEvent::Key(KeyEvent::from(Key::Char('a'))),
            &mut EventCtx::default(),
        );

        assert!(outcome.handled());
        assert_eq!(dialog.title.borrow().as_str(), "a");
    }

    #[test]
    fn title_feedback_tracks_raw_input_while_typing() {
        let mut dialog = CreateTaskDialog::new();
        let target = focus_title(&mut dialog);

        type_title(&mut dialog, &target, "fix   dont crash...");

        assert_eq!(dialog.title.borrow().as_str(), "fix   dont crash...");
        assert_eq!(rendered(&dialog).matches("Perfect").count(), 3);
    }

    #[test]
    fn enter_formats_shared_and_rendered_title_after_editing() {
        let mut dialog = CreateTaskDialog::new();
        let target = focus_title(&mut dialog);
        type_title(&mut dialog, &target, "Theres five and ill do it");
        let mut ctx = EventCtx::default();

        let outcome = dialog.dispatch_event(
            &EventRoute::new(target.path),
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut ctx,
        );

        assert!(outcome.handled());
        assert!(ctx.redraw_requested());
        assert!(ctx.messages().is_empty());
        assert_eq!(
            dialog.title.borrow().as_str(),
            "There's five and I'll do it"
        );
        let rendered = rendered(&dialog);
        assert!(rendered.contains("There's five and I'll do it"));
        assert!(!rendered.contains("Theres five and ill do it"));
    }

    #[test]
    fn ctrl_enter_formats_title_and_submits_once_after_editing() {
        let mut dialog = CreateTaskDialog::new();
        let target = focus_title(&mut dialog);
        type_title(&mut dialog, &target, "Theres five and ill do it");
        let mut ctx = EventCtx::default();

        let outcome = dialog.dispatch_event(
            &EventRoute::new(target.path),
            &TuiEvent::Key(KeyEvent {
                code: Key::Enter,
                modifiers: KeyModifiers::CONTROL,
            }),
            &mut ctx,
        );

        assert!(outcome.handled());
        assert!(ctx.redraw_requested());
        assert_eq!(
            dialog.title.borrow().as_str(),
            "There's five and I'll do it"
        );
        assert!(matches!(
            ctx.messages(),
            [AppMsg::CreateTaskSubmitted(CreateTaskDraft { title })]
                if title == "There's five and I'll do it"
        ));
        assert!(rendered(&dialog).contains("There's five and I'll do it"));
    }

    #[test]
    fn ctrl_enter_on_inactive_title_submits_dialog() {
        let mut dialog = CreateTaskDialog::new();
        let target = focus_title(&mut dialog);
        type_title(&mut dialog, &target, "fix   dont crash...");
        dialog.dispatch_event(
            &EventRoute::new(target.path.clone()),
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        let mut ctx = EventCtx::default();

        dialog.dispatch_event(
            &EventRoute::new(target.path),
            &TuiEvent::Key(KeyEvent {
                code: Key::Enter,
                modifiers: KeyModifiers::CONTROL,
            }),
            &mut ctx,
        );

        assert_eq!(dialog.title.borrow().as_str(), "Fix don't crash");
        assert!(matches!(
            ctx.messages(),
            [AppMsg::CreateTaskSubmitted(CreateTaskDraft { title })]
                if title == "Fix don't crash"
        ));
    }
}
