use std::time::Duration;

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, Button, ChildKey, Dropdown, DropdownCommitMode, DropdownSearchMode,
    EventCtx, EventOutcome, EventRoute, FocusCtx, FocusRequest, FocusTarget, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult, TreePath,
    TuiEvent, TuiNode,
};

use crate::{
    app::AppMsg,
    app_keymap::keys,
    domain::Person,
    ui::{management::ManagementDialogKind, responsive_split::ResponsiveSplit},
};

const CREATE_BUTTON: &str = "new";

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

    #[cfg(test)]
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

    #[cfg(test)]
    pub(super) fn child_areas(&self) -> (Rect, Rect) {
        self.split.child_areas()
    }

    #[cfg(test)]
    pub(super) fn create_area(&self) -> Rect {
        self.create_area
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
    rows: Vec<Choice>,
    selected: &str,
    on_select: impl Fn(String) + 'static,
) -> Dropdown<Choice, String> {
    Dropdown::single(rows, |row| row.id.clone(), |row| row.label.clone())
        .label(label)
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
