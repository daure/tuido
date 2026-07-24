use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, DialogAction, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx,
    FocusTarget, Key, KeyEvent, KeySpec, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint,
    LifecycleCtx, RenderCtx, TextInput, TextareaInput, TickResult, TuiEvent, TuiNode,
};

use crate::{app::AppMsg, ui::management::ManagementDialogKind};

#[derive(Debug, Clone)]
pub(crate) enum ManagementEntityDraft {
    Person {
        name: String,
        email: String,
    },
    Project {
        key: String,
        name: String,
        description: String,
    },
    Tag {
        label: String,
    },
}

pub(crate) struct CreateManagementDialog {
    kind: ManagementDialogKind,
    root: Flex<AppMsg>,
    primary: Rc<RefCell<String>>,
    secondary: Rc<RefCell<String>>,
    description: Rc<RefCell<String>>,
}

impl CreateManagementDialog {
    pub(crate) fn new(kind: ManagementDialogKind) -> Self {
        let primary = Rc::new(RefCell::new(String::new()));
        let secondary = Rc::new(RefCell::new(String::new()));
        let description = Rc::new(RefCell::new(String::new()));
        let mut first = TextInput::new()
            .panel(primary_label(kind))
            .placeholder(primary_placeholder(kind))
            .focused(true)
            .on_change({
                let primary = Rc::clone(&primary);
                move |value| {
                    *primary.borrow_mut() = value;
                    AppMsg::Noop
                }
            });
        first.event(
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        let mut root = Flex::column().child("primary", first, FlexItem::fixed(3));
        if kind != ManagementDialogKind::Tags {
            root = root.child(
                "secondary",
                TextInput::new()
                    .panel(secondary_label(kind))
                    .placeholder(secondary_placeholder(kind))
                    .on_change({
                        let secondary = Rc::clone(&secondary);
                        move |value| {
                            *secondary.borrow_mut() = value;
                            AppMsg::Noop
                        }
                    }),
                FlexItem::fixed(3),
            );
        }
        if kind == ManagementDialogKind::Projects {
            root = root.child(
                "description",
                TextareaInput::new()
                    .panel("Description")
                    .placeholder("Project description")
                    .min_rows(4)
                    .max_rows(8)
                    .on_change({
                        let description = Rc::clone(&description);
                        move |value| {
                            *description.borrow_mut() = value;
                            AppMsg::Noop
                        }
                    }),
                FlexItem::content(),
            );
        }
        Self {
            kind,
            root,
            primary,
            secondary,
            description,
        }
    }

    pub(crate) fn actions(&self) -> [DialogAction<AppMsg>; 2] {
        let kind = self.kind;
        let primary = Rc::clone(&self.primary);
        let secondary = Rc::clone(&self.secondary);
        let description = Rc::clone(&self.description);
        [
            DialogAction::new("OK")
                .hotkey(KeySpec::plain('o'))
                .on_trigger(move || {
                    let primary = primary.borrow().clone();
                    let secondary = secondary.borrow().clone();
                    let draft = match kind {
                        ManagementDialogKind::People => ManagementEntityDraft::Person {
                            name: primary,
                            email: secondary,
                        },
                        ManagementDialogKind::Projects => ManagementEntityDraft::Project {
                            key: primary,
                            name: secondary,
                            description: description.borrow().clone(),
                        },
                        ManagementDialogKind::Tags => ManagementEntityDraft::Tag { label: primary },
                    };
                    AppMsg::CreateManagementSubmitted(draft)
                }),
            DialogAction::new("Cancel")
                .hotkey(KeySpec::plain('c'))
                .on_trigger(|| AppMsg::CloseManagementOverlay),
        ]
    }
}

fn primary_label(kind: ManagementDialogKind) -> &'static str {
    match kind {
        ManagementDialogKind::People => "Name",
        ManagementDialogKind::Projects => "Key",
        ManagementDialogKind::Tags => "Label",
    }
}

fn primary_placeholder(kind: ManagementDialogKind) -> &'static str {
    match kind {
        ManagementDialogKind::People => "Person name",
        ManagementDialogKind::Projects => "Project key",
        ManagementDialogKind::Tags => "Tag label",
    }
}

fn secondary_label(kind: ManagementDialogKind) -> &'static str {
    match kind {
        ManagementDialogKind::People => "Email",
        ManagementDialogKind::Projects => "Name",
        ManagementDialogKind::Tags => "",
    }
}

fn secondary_placeholder(kind: ManagementDialogKind) -> &'static str {
    match kind {
        ManagementDialogKind::People => "Email address",
        ManagementDialogKind::Projects => "Project name",
        ManagementDialogKind::Tags => "",
    }
}

impl TuiNode<AppMsg> for CreateManagementDialog {
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

    #[test]
    fn each_entity_uses_its_editable_creation_fields() {
        let person = CreateManagementDialog::new(ManagementDialogKind::People);
        let project = CreateManagementDialog::new(ManagementDialogKind::Projects);
        let tag = CreateManagementDialog::new(ManagementDialogKind::Tags);

        assert_eq!(
            person
                .root
                .measure(LayoutProposal::unbounded())
                .preferred
                .height,
            6
        );
        assert_eq!(
            project
                .root
                .measure(LayoutProposal::unbounded())
                .preferred
                .height,
            12
        );
        assert_eq!(
            tag.root
                .measure(LayoutProposal::unbounded())
                .preferred
                .height,
            3
        );
    }
}
