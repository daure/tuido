use std::time::Duration as StdDuration;

use ratatui::{Frame, layout::Rect};
use time::{Duration, PrimitiveDateTime};
use tuicore::{
    AnimationSettings, Calendar, CalendarEntryRole, CalendarSpan, EventCtx, EventOutcome,
    EventRoute, FocusCtx, FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint,
    LifecycleCtx, RenderCtx, TickResult, TuiEvent, TuiNode,
};

use crate::app::{AppContext, AppMsg};
use crate::domain::{Task, TaskState};

const SNOOZE_ICON: char = '󰒲';

#[derive(Clone)]
struct SnoozedTaskEntry {
    id: String,
    title: String,
    until: PrimitiveDateTime,
}

type TaskCalendar = Calendar<SnoozedTaskEntry, String, AppMsg>;

pub(crate) struct CalendarWorkspace {
    context: AppContext,
    calendar: TaskCalendar,
    observed_version: u64,
}

impl CalendarWorkspace {
    pub(crate) fn new(context: AppContext) -> Self {
        let state = context.store.borrow();
        let observed_version = state.state().version;
        let calendar = task_calendar(snoozed_task_entries(&state.state().tasks));
        drop(state);
        Self {
            context,
            calendar,
            observed_version,
        }
    }

    fn sync_store_version(&mut self) {
        let state = self.context.store.borrow();
        if self.observed_version == state.state().version {
            return;
        }
        self.observed_version = state.state().version;
        self.calendar
            .set_entries(snoozed_task_entries(&state.state().tasks));
    }
}

fn snoozed_task_entries(tasks: &[Task]) -> Vec<SnoozedTaskEntry> {
    tasks
        .iter()
        .filter_map(|task| {
            (task.state == TaskState::Snoozed).then_some(SnoozedTaskEntry {
                id: task.id.clone(),
                title: task.title.clone(),
                until: task.snoozed_until?,
            })
        })
        .collect()
}

fn task_calendar(entries: Vec<SnoozedTaskEntry>) -> TaskCalendar {
    Calendar::new(
        entries,
        |entry| entry.id.clone(),
        |entry| CalendarSpan::timed(entry.until, entry.until + Duration::minutes(1)),
        |entry| entry.title.clone(),
    )
    .role(|_| Some(CalendarEntryRole::Muted))
    .event_marker(|_| SNOOZE_ICON)
}

impl TuiNode<AppMsg> for CalendarWorkspace {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.calendar.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync_store_version();
        self.calendar.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        <TaskCalendar as TuiNode<AppMsg>>::render(&self.calendar, frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        self.calendar.event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        self.calendar.dispatch_event(route, event, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        self.calendar.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: StdDuration, settings: AnimationSettings) -> TickResult {
        self.calendar.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.calendar.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.calendar.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.calendar.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.calendar.destroy(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::rendered_text;
    use crate::domain::{TaskPriority, TaskSize};
    use time::{Date, Month, Time};

    fn task(id: &str, title: &str, state: TaskState, until: Option<PrimitiveDateTime>) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            state,
            size: TaskSize::Small,
            priority: TaskPriority::Medium,
            start_date: None,
            due_date: None,
            snoozed_until: until,
            people_ids: Vec::new(),
            project_ids: Vec::new(),
            tag_ids: Vec::new(),
            detail: String::new(),
        }
    }

    #[test]
    fn calendar_shows_dated_snoozed_tasks_with_snooze_icons() {
        let date = Date::from_calendar_date(2026, Month::July, 24).unwrap();
        let until = date.with_time(Time::from_hms(8, 0, 0).unwrap());
        let entries = snoozed_task_entries(&[
            task("snoozed", "Follow up", TaskState::Snoozed, Some(until)),
            task("todo", "Still active", TaskState::Todo, Some(until)),
            task("undated", "Missing return date", TaskState::Snoozed, None),
        ]);
        let mut calendar = task_calendar(entries).cursor(date);
        let area = Rect::new(0, 0, 100, 28);
        calendar.layout(area, &mut LayoutCtx::new());

        let text = rendered_text(&calendar, area);

        assert!(text.contains(&format!("{SNOOZE_ICON} Follow up")));
        assert!(!text.contains("Still active"));
        assert!(!text.contains("Missing return date"));
    }
}
