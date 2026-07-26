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
use crate::persistence_coordinator::PersistenceCommand;
use crate::ui::save_status::SaveStatusLine;

const SNOOZE_ICON: char = '󰒲';
pub(crate) const SHOW_WEEKENDS_SETTING: &str = "calendar.show_weekends";

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
    setting_status: SaveStatusLine,
}

impl CalendarWorkspace {
    pub(crate) fn new(context: AppContext, show_weekends: bool) -> Self {
        let state = context.store.borrow();
        let observed_version = state.state().version;
        let calendar =
            task_calendar(snoozed_task_entries(&state.state().tasks)).show_weekends(show_weekends);
        drop(state);
        Self {
            context,
            calendar,
            observed_version,
            setting_status: SaveStatusLine::new(None),
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
        if let Some(value) = state.state().app_setting_values.get(SHOW_WEEKENDS_SETTING)
            && let Ok(show) = parse_show_weekends_setting(Some(value))
        {
            self.calendar.set_show_weekends(show);
        }
        self.setting_status.set_error(
            state
                .state()
                .app_setting_errors
                .get(SHOW_WEEKENDS_SETTING)
                .map(String::as_str),
        );
    }

    fn persist_weekend_visibility_change(&self, previous: bool) {
        let current = self.calendar.is_showing_weekends();
        if current != previous {
            self.context
                .coordinator
                .borrow_mut()
                .submit(PersistenceCommand::SetAppSetting {
                    key: SHOW_WEEKENDS_SETTING.to_string(),
                    value: current.to_string(),
                    generation: 0,
                });
        }
    }
}

pub(crate) fn parse_show_weekends_setting(value: Option<&str>) -> Result<bool, String> {
    match value {
        None | Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(value) => Err(format!(
            "invalid value for {SHOW_WEEKENDS_SETTING}: {value}"
        )),
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
        let has_error = self
            .context
            .store
            .borrow()
            .state()
            .app_setting_errors
            .contains_key(SHOW_WEEKENDS_SETTING);
        let calendar_area = Rect::new(
            area.x,
            area.y,
            area.width,
            area.height.saturating_sub(u16::from(has_error)),
        );
        self.calendar.layout(calendar_area, ctx);
        if has_error && area.height > 0 {
            self.setting_status.layout(
                Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
                ctx,
            );
        }
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        let has_error = self
            .context
            .store
            .borrow()
            .state()
            .app_setting_errors
            .contains_key(SHOW_WEEKENDS_SETTING);
        let calendar_area = Rect::new(
            area.x,
            area.y,
            area.width,
            area.height.saturating_sub(u16::from(has_error)),
        );
        <TaskCalendar as TuiNode<AppMsg>>::render(&self.calendar, frame, calendar_area, ctx);
        if has_error && area.height > 0 {
            self.setting_status.render(
                frame,
                Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
                ctx,
            );
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        let previous = self.calendar.is_showing_weekends();
        let outcome = self.calendar.event(event, ctx);
        self.persist_weekend_visibility_change(previous);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let previous = self.calendar.is_showing_weekends();
        let outcome = self.calendar.dispatch_event(route, event, ctx);
        self.persist_weekend_visibility_change(previous);
        outcome
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
    use crate::app::tests::{rendered_text, test_context};
    use crate::domain::{AppEvent, TaskPriority, TaskSize, WorkspaceSnapshot};
    use ratatui::{Terminal, backend::TestBackend};
    use time::{Date, Month, Time};
    use tuicore::{Key, KeyEvent, KeyModifiers};

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
            links: Vec::new(),
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

    #[test]
    fn missing_calendar_preference_defaults_to_showing_weekends() {
        assert_eq!(parse_show_weekends_setting(None), Ok(true));
    }

    #[test]
    fn calendar_preference_rejects_invalid_values() {
        assert!(parse_show_weekends_setting(Some("weekdays")).is_err());
    }

    #[test]
    fn toggling_weekends_queues_the_calendar_preference_for_persistence() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context.clone(), true);
        let event = TuiEvent::Key(KeyEvent {
            code: Key::Char('w'),
            modifiers: KeyModifiers::CONTROL,
        });

        let outcome = workspace.event(&event, &mut EventCtx::default());

        assert_eq!(outcome, EventOutcome::Handled);
        assert!(!workspace.calendar.is_showing_weekends());
        assert!(context.coordinator.borrow().has_pending());
    }

    #[test]
    fn zero_height_calendar_with_setting_error_lays_out_and_renders() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        store
            .borrow_mut()
            .dispatch(AppEvent::AppSettingChangeRequested {
                key: SHOW_WEEKENDS_SETTING.into(),
                value: "false".into(),
                generation: 1,
            });
        store
            .borrow_mut()
            .dispatch(AppEvent::AppSettingSaveCompleted {
                key: SHOW_WEEKENDS_SETTING.into(),
                value: "false".into(),
                generation: 1,
                error: Some("Setting save failed".into()),
            });
        let mut workspace = CalendarWorkspace::new(context, true);
        let area = Rect::new(0, 0, 1, 0);

        workspace.layout(area, &mut LayoutCtx::new());
        let mut terminal = Terminal::new(TestBackend::new(1, 1)).unwrap();
        terminal
            .draw(|frame| workspace.render(frame, area, &mut RenderCtx::new()))
            .unwrap();
    }
}
