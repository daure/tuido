use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, DialogAction, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx,
    FocusTarget, Key, KeyEvent, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint,
    LifecycleCtx, RenderCtx, TextInput, TextareaInput, TickResult, TuiEvent, TuiNode,
};

use crate::{
    app::AppMsg,
    app_keymap::keys,
    ui::management::{ManagementDialogKind, projects::ProjectKeyInput},
};

#[derive(Debug, Clone)]
pub(crate) enum ManagementEntityDraft {
    Person {
        name: String,
        email: String,
        about: String,
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
        let mut root = if kind == ManagementDialogKind::Projects {
            Flex::column().child(
                "primary",
                ProjectKeyInput::new(first).on_commit({
                    let primary = Rc::clone(&primary);
                    move |value| *primary.borrow_mut() = value.to_string()
                }),
                FlexItem::fixed(3),
            )
        } else {
            Flex::column().child("primary", first, FlexItem::fixed(3))
        };
        if kind != ManagementDialogKind::Tags {
            root = root.child(
                "secondary",
                TextInput::new().panel(secondary_label(kind)).on_change({
                    let secondary = Rc::clone(&secondary);
                    move |value| {
                        *secondary.borrow_mut() = value;
                        AppMsg::Noop
                    }
                }),
                FlexItem::fixed(3),
            );
        }
        if matches!(
            kind,
            ManagementDialogKind::People | ManagementDialogKind::Projects
        ) {
            root = root.child(
                "description",
                TextareaInput::new()
                    .panel(if kind == ManagementDialogKind::People {
                        "About"
                    } else {
                        "Description"
                    })
                    .min_rows(2)
                    .max_rows(6)
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
                .hotkey(keys::DIALOG_OK.key_spec())
                .on_trigger(move || submitted_message(kind, &primary, &secondary, &description)),
            DialogAction::new("Cancel")
                .hotkey(keys::DIALOG_CANCEL.key_spec())
                .on_trigger(|| AppMsg::CloseManagementOverlay),
        ]
    }

    fn submit_on_ctrl_enter(
        &self,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> Option<EventOutcome> {
        if !keys::DIALOG_SUBMIT.matches(event) {
            return None;
        }

        ctx.emit(submitted_message(
            self.kind,
            &self.primary,
            &self.secondary,
            &self.description,
        ));
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }
}

fn submitted_message(
    kind: ManagementDialogKind,
    primary: &Rc<RefCell<String>>,
    secondary: &Rc<RefCell<String>>,
    description: &Rc<RefCell<String>>,
) -> AppMsg {
    let primary = primary.borrow().clone();
    let secondary = secondary.borrow().clone();
    let draft = match kind {
        ManagementDialogKind::People => ManagementEntityDraft::Person {
            name: primary,
            email: secondary,
            about: description.borrow().clone(),
        },
        ManagementDialogKind::Projects => ManagementEntityDraft::Project {
            key: primary.to_uppercase(),
            name: secondary,
            description: description.borrow().clone(),
        },
        ManagementDialogKind::Tags => ManagementEntityDraft::Tag { label: primary },
    };
    AppMsg::CreateManagementSubmitted(draft)
}

fn primary_label(kind: ManagementDialogKind) -> &'static str {
    match kind {
        ManagementDialogKind::People => "Name",
        ManagementDialogKind::Projects => "Key",
        ManagementDialogKind::Tags => "Label",
    }
}

fn secondary_label(kind: ManagementDialogKind) -> &'static str {
    match kind {
        ManagementDialogKind::People => "Email",
        ManagementDialogKind::Projects => "Name",
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
        if let Some(outcome) = self.submit_on_ctrl_enter(event, ctx) {
            return outcome;
        }
        self.root.event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        if let Some(outcome) = self.submit_on_ctrl_enter(event, ctx) {
            return outcome;
        }
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
    use tuicore::KeyModifiers;

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
            10
        );
        assert_eq!(
            project
                .root
                .measure(LayoutProposal::unbounded())
                .preferred
                .height,
            10
        );
        assert_eq!(
            tag.root
                .measure(LayoutProposal::unbounded())
                .preferred
                .height,
            3
        );
    }

    #[test]
    fn ctrl_enter_submits_each_management_entity_from_a_focused_control() {
        let cases = [
            (
                ManagementDialogKind::People,
                "Ada",
                "ada@example.com",
                "Compiler expert",
            ),
            (ManagementDialogKind::Projects, "core", "Core", "Platform"),
            (ManagementDialogKind::Tags, "backend", "", ""),
        ];

        for (kind, primary, secondary, description) in cases {
            let mut dialog = CreateManagementDialog::new(kind);
            *dialog.primary.borrow_mut() = primary.into();
            *dialog.secondary.borrow_mut() = secondary.into();
            *dialog.description.borrow_mut() = description.into();
            let mut layout = LayoutCtx::new();
            dialog.layout(Rect::new(0, 0, 80, 20), &mut layout);
            let target = layout
                .focus_targets()
                .last()
                .expect("creation dialog should contain a focusable control");
            let mut ctx = EventCtx::default();

            let outcome = dialog.dispatch_event(
                &EventRoute::new(target.path.clone()),
                &TuiEvent::Key(KeyEvent {
                    code: Key::Enter,
                    modifiers: KeyModifiers::CONTROL,
                }),
                &mut ctx,
            );

            assert!(outcome.handled());
            match kind {
                ManagementDialogKind::People => assert!(matches!(
                    ctx.messages(),
                    [AppMsg::CreateManagementSubmitted(ManagementEntityDraft::Person {
                        name,
                        email,
                        about,
                    })] if name == "Ada" && email == "ada@example.com" && about == "Compiler expert"
                )),
                ManagementDialogKind::Projects => assert!(matches!(
                    ctx.messages(),
                    [AppMsg::CreateManagementSubmitted(ManagementEntityDraft::Project {
                        key,
                        name,
                        description,
                    })] if key == "CORE" && name == "Core" && description == "Platform"
                )),
                ManagementDialogKind::Tags => assert!(matches!(
                    ctx.messages(),
                    [AppMsg::CreateManagementSubmitted(ManagementEntityDraft::Tag { label })]
                        if label == "backend"
                )),
            }
        }
    }
}
