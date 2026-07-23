use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, DialogAction, Dropdown, DropdownCommitMode, DropdownPopupDirection,
    DropdownSearchMode, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusTarget,
    Key, KeyEvent, KeySpec, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    RenderCtx, TextInput, TextareaInput, TickResult, TuiEvent, TuiNode,
};

use crate::{app::AppMsg, domain::TaskSize};

#[derive(Debug, Clone)]
pub(crate) struct CreateTaskDraft {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) size: TaskSize,
}

struct SizeChoice {
    id: String,
    label: String,
}

pub(crate) struct CreateTaskDialog {
    root: Flex<AppMsg>,
    title: Rc<RefCell<String>>,
    description: Rc<RefCell<String>>,
    size: Rc<RefCell<TaskSize>>,
}

impl CreateTaskDialog {
    pub(crate) fn new() -> Self {
        let title = Rc::new(RefCell::new(String::new()));
        let description = Rc::new(RefCell::new(String::new()));
        let size = Rc::new(RefCell::new(TaskSize::Small));
        let mut input = TextInput::new()
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
        input.event(
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        let description_input = TextareaInput::new()
            .panel("Description")
            .placeholder("Task description")
            .min_rows(4)
            .max_rows(10)
            .on_change({
                let description = Rc::clone(&description);
                move |value| {
                    *description.borrow_mut() = value;
                    AppMsg::Noop
                }
            });
        let size_input = Dropdown::single(
            size_choices(),
            |row: &SizeChoice| row.id.clone(),
            |row| row.label.clone(),
        )
        .label("Size")
        .selected_one(TaskSize::Small.id().to_string())
        .search_mode(DropdownSearchMode::Contains)
        .commit_mode(DropdownCommitMode::Explicit)
        .popup_direction(DropdownPopupDirection::Up)
        .on_select({
            let size = Rc::clone(&size);
            move |ids| {
                if let Some(value) = ids.into_iter().next().and_then(|id| TaskSize::parse(&id)) {
                    *size.borrow_mut() = value;
                }
            }
        });
        let root = Flex::column()
            .child("title", input, FlexItem::fixed(3))
            .child("description", description_input, FlexItem::content())
            .child("size", size_input, FlexItem::fixed(3));

        Self {
            root,
            title,
            description,
            size,
        }
    }

    pub(crate) fn actions(&self) -> [DialogAction<AppMsg>; 2] {
        let title = Rc::clone(&self.title);
        let description = Rc::clone(&self.description);
        let size = Rc::clone(&self.size);
        [
            DialogAction::new("OK")
                .hotkey(KeySpec::plain('o'))
                .on_trigger(move || {
                    AppMsg::CreateTaskSubmitted(CreateTaskDraft {
                        title: title.borrow().clone(),
                        description: description.borrow().clone(),
                        size: *size.borrow(),
                    })
                }),
            DialogAction::new("Cancel")
                .hotkey(KeySpec::plain('c'))
                .on_trigger(|| AppMsg::CloseDialog),
        ]
    }
}

fn size_choices() -> Vec<SizeChoice> {
    [TaskSize::Small, TaskSize::Medium, TaskSize::Big]
        .into_iter()
        .map(|size| SizeChoice {
            id: size.id().to_string(),
            label: size.label().to_string(),
        })
        .collect()
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
    use tuicore::{Key, KeyEvent, LayoutProposal};

    #[test]
    fn controls_are_adjacent_without_gap_rows() {
        let dialog = CreateTaskDialog::new();

        assert_eq!(
            dialog
                .root
                .measure(LayoutProposal::unbounded())
                .preferred
                .height,
            12
        );
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
        let area = Rect::new(0, 0, 50, 20);
        let mut dialog = CreateTaskDialog::new();
        let mut layout = LayoutCtx::new();
        dialog.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .first()
            .expect("title should be first focus target")
            .clone();
        dialog.dispatch_focus(&target, true, &mut FocusCtx::default());

        let outcome = dialog.dispatch_event(
            &EventRoute::new(target.path),
            &TuiEvent::Key(KeyEvent::from(Key::Char('a'))),
            &mut EventCtx::default(),
        );

        assert!(outcome.handled());
        assert_eq!(dialog.title.borrow().as_str(), "a");
    }
}
