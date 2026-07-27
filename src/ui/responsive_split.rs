use std::time::Duration;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};
use tuicore::{
    AnimationSettings, AxisProposal, ChildKey, EventCtx, EventOutcome, EventRoute, FocusCtx,
    FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, TickResult,
    TuiEvent, TuiNode,
};

const NARROW_MASTER_MIN_HEIGHT: u16 = 3;

pub(crate) struct ResponsiveSplit<F, S> {
    first: F,
    second: S,
    breakpoint: u16,
    wide_ratio: (u16, u16),
    narrow_second_content: bool,
    first_area: Rect,
    second_area: Rect,
}

impl<F, S> ResponsiveSplit<F, S> {
    pub(crate) fn master_detail(first: F, second: S) -> Self {
        Self::new(first, second, 100)
            .wide_ratio(60, 40)
            .narrow_second_content()
    }

    pub(crate) fn new(first: F, second: S, breakpoint: u16) -> Self {
        Self {
            first,
            second,
            breakpoint,
            wide_ratio: (50, 50),
            narrow_second_content: false,
            first_area: Rect::default(),
            second_area: Rect::default(),
        }
    }

    pub(crate) fn wide_ratio(mut self, first: u16, second: u16) -> Self {
        self.wide_ratio = (first, second);
        self
    }

    pub(crate) fn narrow_second_content(mut self) -> Self {
        self.narrow_second_content = true;
        self
    }

    pub(crate) fn first(&self) -> &F {
        &self.first
    }

    pub(crate) fn first_mut(&mut self) -> &mut F {
        &mut self.first
    }

    pub(crate) fn second(&self) -> &S {
        &self.second
    }

    pub(crate) fn second_mut(&mut self) -> &mut S {
        &mut self.second
    }

    #[cfg(test)]
    pub(crate) fn child_areas(&self) -> (Rect, Rect) {
        (self.first_area, self.second_area)
    }

    fn is_stacked(&self, width: u16) -> bool {
        width < self.breakpoint
    }

    fn ratio_constraints((first, second): (u16, u16)) -> [Constraint; 2] {
        let denominator = u32::from(first).saturating_add(u32::from(second)).max(1);
        [
            Constraint::Ratio(first.into(), denominator),
            Constraint::Ratio(second.into(), denominator),
        ]
    }
}

impl<F, S, M> TuiNode<M> for ResponsiveSplit<F, S>
where
    F: TuiNode<M>,
    S: TuiNode<M>,
{
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let first = self.first.measure(proposal);
        let second = self.second.measure(proposal);
        let stacked = match proposal.width {
            AxisProposal::Exact(width) | AxisProposal::AtMost(width) => self.is_stacked(width),
            AxisProposal::Unbounded => false,
        };
        let (width, height) = if stacked {
            (
                first.preferred.width.max(second.preferred.width),
                first
                    .preferred
                    .height
                    .saturating_add(second.preferred.height),
            )
        } else {
            (
                first.preferred.width.saturating_add(second.preferred.width),
                first.preferred.height.max(second.preferred.height),
            )
        };
        LayoutSizeHint::content(width, height).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        let stacked = self.is_stacked(area.width);
        let direction = if stacked {
            Direction::Vertical
        } else {
            Direction::Horizontal
        };
        let constraints = if stacked && self.narrow_second_content {
            let preferred_second_height = self
                .second
                .measure(LayoutProposal::at_most(area.width, area.height))
                .preferred
                .height;
            let second_height =
                preferred_second_height.min(area.height.saturating_sub(NARROW_MASTER_MIN_HEIGHT));
            [Constraint::Fill(1), Constraint::Length(second_height)]
        } else if stacked {
            Self::ratio_constraints((50, 50))
        } else {
            Self::ratio_constraints(self.wide_ratio)
        };
        let [first_area, second_area] = Layout::default()
            .direction(direction)
            .constraints(constraints)
            .areas(area);
        self.first_area = first_area;
        self.second_area = second_area;
        ctx.push_slot(ChildKey::first(), first_area, |ctx| {
            self.first.layout(first_area, ctx);
        });
        ctx.push_slot(ChildKey::second(), second_area, |ctx| {
            self.second.layout(second_area, ctx);
        });
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, _area: Rect, ctx: &mut tuicore::RenderCtx<'a>) {
        self.first.render(frame, self.first_area, ctx);
        self.second.render(frame, self.second_area, ctx);
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<M>,
    ) -> EventOutcome {
        if route.path.is_empty() {
            return self.event(event, ctx);
        }
        if let Some(route) = route
            .path
            .without_first_if(&ChildKey::first())
            .map(EventRoute::new)
        {
            return self
                .first
                .dispatch_event(&route, event, ctx)
                .bubble(ctx, |ctx| self.event(event, ctx));
        }
        if let Some(route) = route
            .path
            .without_first_if(&ChildKey::second())
            .map(EventRoute::new)
        {
            return self
                .second
                .dispatch_event(&route, event, ctx)
                .bubble(ctx, |ctx| self.event(event, ctx));
        }
        EventOutcome::Ignored
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<M>) {
        if let Some(target) = target.for_child(&ChildKey::first()) {
            self.first.dispatch_focus(&target, focused, ctx);
        } else if let Some(target) = target.for_child(&ChildKey::second()) {
            self.second.dispatch_focus(&target, focused, ctx);
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.first
            .tick(dt, settings)
            .merge(self.second.tick(dt, settings))
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.first.init(ctx);
        self.second.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.first.mount(ctx);
        self.second.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.second.unmount(ctx);
        self.first.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.second.destroy(ctx);
        self.first.destroy(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tuicore::Paragraph;

    fn split() -> ResponsiveSplit<Paragraph, Paragraph> {
        ResponsiveSplit::master_detail(Paragraph::new("first"), Paragraph::new("second"))
    }

    #[test]
    fn wide_layout_places_children_side_by_side_at_sixty_forty() {
        let mut split = split();

        <ResponsiveSplit<_, _> as TuiNode<()>>::layout(
            &mut split,
            Rect::new(0, 0, 120, 50),
            &mut LayoutCtx::new(),
        );

        let (first, second) = split.child_areas();
        assert_eq!(first, Rect::new(0, 0, 72, 50));
        assert_eq!(second, Rect::new(72, 0, 48, 50));
    }

    #[test]
    fn narrow_layout_sizes_second_child_to_content_and_gives_first_child_the_remainder() {
        let mut split = split();

        <ResponsiveSplit<_, _> as TuiNode<()>>::layout(
            &mut split,
            Rect::new(0, 0, 80, 50),
            &mut LayoutCtx::new(),
        );

        let (first, second) = split.child_areas();
        assert_eq!(first, Rect::new(0, 0, 80, 49));
        assert_eq!(second, Rect::new(0, 49, 80, 1));
    }

    #[test]
    fn short_narrow_layout_reserves_master_rows_below_detail_preference() {
        let mut split = ResponsiveSplit::master_detail(
            Paragraph::new("master"),
            Paragraph::new("detail\nline 2\nline 3\nline 4\nline 5"),
        );

        <ResponsiveSplit<_, _> as TuiNode<()>>::layout(
            &mut split,
            Rect::new(0, 0, 80, 5),
            &mut LayoutCtx::new(),
        );

        let (master, detail) = split.child_areas();
        assert_eq!(master.height, NARROW_MASTER_MIN_HEIGHT);
        assert_eq!(detail.height, 2);
    }

    #[test]
    fn very_short_narrow_layout_gives_all_available_rows_to_master() {
        let mut split = ResponsiveSplit::master_detail(
            Paragraph::new("master"),
            Paragraph::new("detail\nline 2\nline 3"),
        );

        <ResponsiveSplit<_, _> as TuiNode<()>>::layout(
            &mut split,
            Rect::new(0, 0, 80, 2),
            &mut LayoutCtx::new(),
        );

        let (master, detail) = split.child_areas();
        assert_eq!(master.height, 2);
        assert_eq!(detail.height, 0);
    }
}
