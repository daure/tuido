use std::time::Duration;

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, Button, ChildKey, Dropdown, DropdownCommitMode, DropdownSearchMode,
    EventCtx, EventOutcome, EventRoute, FocusCtx, FocusRequest, FocusTarget, Key, KeyEvent,
    KeyModifiers, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx,
    SeasonalEmptyState, TextInput, TickResult, TreePath, TuiEvent, TuiNode,
};

use crate::{
    app::AppMsg,
    app_keymap::keys,
    domain::Person,
    ui::{management::ManagementDialogKind, responsive_split::ResponsiveSplit},
};

const CREATE_BUTTON: &str = "new";

type RequiredTextCommit = Box<dyn Fn(&str)>;

pub(super) struct RequiredTextInput {
    input: TextInput<AppMsg>,
    committed_value: String,
    invalid_title: &'static str,
    on_commit: RequiredTextCommit,
}

impl RequiredTextInput {
    pub(super) fn new(
        input: TextInput<AppMsg>,
        invalid_title: &'static str,
        on_commit: impl Fn(&str) + 'static,
    ) -> Self {
        let committed_value = input.current_value().to_string();
        Self {
            input,
            committed_value,
            invalid_title,
            on_commit: Box::new(on_commit),
        }
    }
}

impl TuiNode<AppMsg> for RequiredTextInput {
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
        let commit = self.input.insert_mode()
            && matches!(
                event,
                TuiEvent::Key(KeyEvent {
                    code: Key::Enter,
                    modifiers: KeyModifiers::NONE,
                })
            );
        let outcome = self.input.event(event, ctx);
        if commit {
            let value = self.input.current_value().trim().to_string();
            if value.is_empty() {
                self.input.set_value(self.committed_value.clone());
                ctx.notify(tuicore::Notification::warning(
                    self.invalid_title,
                    "Name cannot be empty.",
                ));
                ctx.request_redraw();
            } else {
                self.committed_value.clone_from(&value);
                self.input.set_value(value);
                (self.on_commit)(&self.committed_value);
            }
        }
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

pub(super) fn management_empty_state(
    kind: ManagementDialogKind,
    has_entities: bool,
) -> SeasonalEmptyState {
    let message = match (kind, has_entities) {
        (ManagementDialogKind::People, false) => "No people yet",
        (ManagementDialogKind::People, true) => "No people match your search",
        (ManagementDialogKind::Workspaces, false) => "No workspaces yet",
        (ManagementDialogKind::Workspaces, true) => "No workspaces match your search",
        (ManagementDialogKind::Tags, false) => "No tags yet",
        (ManagementDialogKind::Tags, true) => "No tags match your search",
    };
    SeasonalEmptyState::new(message)
}

pub(super) struct ManagementPane<F, S> {
    split: ResponsiveSplit<F, S>,
    create: Button<AppMsg>,
    kind: ManagementDialogKind,
    create_area: Rect,
    first_path: TreePath,
}

impl<F, S> ManagementPane<F, S> {
    pub(super) fn new(first: F, second: S, kind: ManagementDialogKind) -> Self {
        Self {
            split: ResponsiveSplit::master_detail(first, second),
            create: Button::new("New")
                .hotkey(keys::MANAGEMENT_CREATE.hotkey())
                .on_press(move || AppMsg::OpenCreateManagement(kind)),
            kind,
            create_area: Rect::default(),
            first_path: TreePath::new(),
        }
    }

    pub(super) fn detail_visible(mut self, visible: bool) -> Self {
        self.split = self.split.second_visible(visible);
        self
    }

    pub(super) fn set_detail_visible(&mut self, visible: bool) -> bool {
        self.split.set_second_visible(visible)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn is_detail_visible(&self) -> bool {
        self.split.is_second_visible()
    }

    pub(super) fn first(&self) -> &F {
        self.split.first()
    }

    pub(super) fn first_mut(&mut self) -> &mut F {
        self.split.first_mut()
    }

    pub(super) fn second(&self) -> &S {
        self.split.second()
    }

    pub(super) fn second_mut(&mut self) -> &mut S {
        self.split.second_mut()
    }

    pub(super) fn return_to_table_on_unfocus(
        &self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> bool {
        if route.path.without_first_if(&ChildKey::second()).is_none()
            || !crate::app_keymap::matches_any(event, &[keys::DETAIL_CLOSE, keys::DETAIL_CLOSE_ALT])
        {
            return false;
        }
        ctx.focus(FocusRequest::Path(self.first_path.clone()));
        ctx.stop_propagation();
        true
    }
}

impl<F, S> TuiNode<AppMsg> for ManagementPane<F, S>
where
    F: TuiNode<AppMsg>,
    S: TuiNode<AppMsg>,
{
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.split.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.first_path = ctx.current_path().child(ChildKey::first());
        self.split.layout(area, ctx);
        let size = self
            .create
            .measure(LayoutProposal::at_most(area.width, area.height))
            .preferred;
        let width = size.width.min(area.width);
        let height = size.height.min(area.height);
        self.create_area = Rect::new(area.right().saturating_sub(width), area.y, width, height);
        ctx.push_slot(ChildKey::new(CREATE_BUTTON), self.create_area, |ctx| {
            self.create.layout(self.create_area, ctx);
        });
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.split.render(frame, area, ctx);
        <Button<AppMsg> as TuiNode<AppMsg>>::render(&self.create, frame, self.create_area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        if keys::MANAGEMENT_CREATE.matches(event) {
            ctx.emit(AppMsg::OpenCreateManagement(self.kind));
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        let outcome = self.create.event(event, ctx);
        if outcome.handled() {
            outcome
        } else {
            self.split.event(event, ctx)
        }
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        if let Some(route) = route
            .path
            .without_first_if(&ChildKey::new(CREATE_BUTTON))
            .map(EventRoute::new)
        {
            return self.create.dispatch_event(&route, event, ctx);
        }
        self.split.dispatch_event(route, event, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        if let Some(target) = target.for_child(&ChildKey::new(CREATE_BUTTON)) {
            self.create.dispatch_focus(&target, focused, ctx);
        } else {
            self.split.dispatch_focus(target, focused, ctx);
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.split
            .tick(dt, settings)
            .merge(self.create.tick(dt, settings))
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.split.init(ctx);
        self.create.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.split.mount(ctx);
        self.create.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.create.unmount(ctx);
        self.split.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.create.destroy(ctx);
        self.split.destroy(ctx);
    }
}

#[derive(Debug, Clone)]
pub(super) struct Choice {
    id: String,
    label: String,
}

pub(super) fn dropdown_single(
    label: &'static str,
    placeholder: &'static str,
    rows: Vec<Choice>,
    selected: &str,
    on_select: impl Fn(String) + 'static,
) -> Dropdown<Choice, String> {
    Dropdown::single(rows, |row| row.id.clone(), |row| row.label.clone())
        .label(label)
        .placeholder(placeholder)
        .selected_one(selected.to_string())
        .search_mode(DropdownSearchMode::Contains)
        .commit_mode(DropdownCommitMode::Explicit)
        .on_select(move |ids| {
            if let Some(id) = ids.into_iter().next() {
                on_select(id);
            }
        })
}

pub(super) fn dropdown_single_optional(
    label: &'static str,
    placeholder: &'static str,
    mut rows: Vec<Choice>,
    selected: Option<&str>,
    on_select: impl Fn(Option<String>) + 'static,
) -> Dropdown<Choice, String> {
    rows.insert(
        0,
        Choice {
            id: String::new(),
            label: "None".to_string(),
        },
    );
    Dropdown::single(rows, |row| row.id.clone(), |row| row.label.clone())
        .label(label)
        .placeholder(placeholder)
        .selected_one(selected.unwrap_or_default().to_string())
        .search_mode(DropdownSearchMode::Contains)
        .commit_mode(DropdownCommitMode::Explicit)
        .on_select(move |ids| {
            if let Some(id) = ids.into_iter().next() {
                on_select((!id.is_empty()).then_some(id));
            }
        })
}

pub(super) fn active_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "true".to_string(),
            label: "Active".to_string(),
        },
        Choice {
            id: "false".to_string(),
            label: "Inactive".to_string(),
        },
    ]
}

pub(super) fn person_choices(people: &[Person]) -> Vec<Choice> {
    people
        .iter()
        .map(|person| Choice {
            id: person.id.clone(),
            label: person.name.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::*;

    #[test]
    fn required_text_commit_restores_original_and_notifies_when_empty() {
        let commits = Rc::new(RefCell::new(Vec::new()));
        let mut input = RequiredTextInput::new(
            TextInput::new().value("Ada").focused(true),
            "Invalid person name",
            {
                let commits = Rc::clone(&commits);
                move |value| commits.borrow_mut().push(value.to_string())
            },
        );
        input.event(
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        for _ in 0..3 {
            input.event(
                &TuiEvent::Key(KeyEvent::from(Key::Backspace)),
                &mut EventCtx::default(),
            );
        }
        let mut ctx = EventCtx::default();

        let outcome = input.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
        let effects = tuicore::DispatchEffects::from_event_ctx(outcome, ctx);

        assert_eq!(input.input.current_value(), "Ada");
        assert!(commits.borrow().is_empty());
        assert_eq!(
            effects.notifications,
            vec![tuicore::Notification::warning(
                "Invalid person name",
                "Name cannot be empty.",
            )]
        );
    }
}
