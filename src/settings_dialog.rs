use std::time::Duration;

use ratatui::{Frame, layout::Rect};
use time::Time;
use tuicore::{
    AnimationSettings, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusTarget,
    LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult,
    TimePicker, Toggle, TuiEvent, TuiNode,
};

use crate::app::AppMsg;

pub(crate) struct SettingsDialog {
    root: Flex<AppMsg>,
}

impl SettingsDialog {
    pub(crate) fn new(show_calendar_weekends: bool, default_snooze_time: Time) -> Self {
        let calendar_view = Toggle::new("Show weekends in calendar")
            .checked(show_calendar_weekends)
            .focused(true)
            .on_change(AppMsg::SetShowCalendarWeekends);
        let snooze_time = TimePicker::new()
            .panel("Default snooze time")
            .value(default_snooze_time)
            .on_select(AppMsg::SetDefaultSnoozeTime);
        let root = Flex::column()
            .gap(1)
            .child("calendar-view", calendar_view, FlexItem::content())
            .child("snooze-time", snooze_time, FlexItem::fixed(3));
        Self { root }
    }
}

impl TuiNode<AppMsg> for SettingsDialog {
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
    use time::macros::time;
    use tuicore::{Key, KeyEvent};

    #[test]
    fn settings_expose_calendar_view_and_default_snooze_time() {
        let mut dialog = SettingsDialog::new(false, time!(8:15));
        let mut layout = LayoutCtx::new();
        dialog.layout(Rect::new(0, 0, 40, 5), &mut layout);

        let targets = layout.focus_targets();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].id.as_str(), "toggle");
        assert_eq!(targets[1].id.as_str(), "time-picker");
    }

    #[test]
    fn settings_emit_changes_for_immediate_persistence() {
        let mut dialog = SettingsDialog::new(false, time!(8:15));
        let area = Rect::new(0, 0, 40, 5);
        let mut layout = LayoutCtx::new();
        dialog.layout(area, &mut layout);
        let toggle = layout.focus_targets()[0].clone();
        dialog.dispatch_focus(&toggle, true, &mut FocusCtx::default());
        let mut ctx = EventCtx::default();

        dialog.dispatch_event(
            &EventRoute::new(toggle.path),
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut ctx,
        );

        assert!(matches!(
            ctx.messages(),
            [AppMsg::SetShowCalendarWeekends(true)]
        ));
    }

    #[test]
    fn completing_time_selection_emits_configured_snooze_default() {
        let mut dialog = SettingsDialog::new(true, time!(8:15));
        let area = Rect::new(0, 0, 40, 5);
        let mut layout = LayoutCtx::new();
        dialog.layout(area, &mut layout);
        let time_picker = layout.focus_targets()[1].clone();
        dialog.dispatch_focus(&time_picker, true, &mut FocusCtx::default());
        let mut ctx = EventCtx::default();

        for _ in 0..2 {
            dialog.dispatch_event(
                &EventRoute::new(time_picker.path.clone()),
                &TuiEvent::Key(KeyEvent::from(Key::Enter)),
                &mut ctx,
            );
        }

        assert!(matches!(
            ctx.messages(),
            [AppMsg::SetDefaultSnoozeTime(value)] if *value == time!(8:15)
        ));
    }
}
