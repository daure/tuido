use std::time::Duration as StdDuration;

use ratatui::{Frame, layout::Rect};
use time::{Date, Duration, OffsetDateTime, PrimitiveDateTime};
use tuicore::{
    AnimationSettings, Calendar, CalendarEntryRole, CalendarKeyBindings, CalendarSpan,
    CalendarTypedEvent, CalendarView, ChildKey, EventCtx, EventOutcome, EventRoute, FocusCtx,
    FocusId, FocusRequest, FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint,
    LifecycleCtx, RenderCtx, TickResult, TuiEvent, TuiNode,
};

use crate::app::{AppContext, AppMsg, task_detail::detail_escape};
use crate::app_keymap::keys;
use crate::domain::{Task, TaskState};
use crate::persistence_coordinator::PersistenceCommand;
use crate::ui::responsive_split::ResponsiveSplit;
use crate::ui::save_status::SaveStatusLine;
use crate::ui::task_detail::TaskDetailForm;

const SNOOZE_ICON: char = '󰒲';
pub(crate) const SHOW_WEEKENDS_SETTING: &str = "calendar.show_weekends";

#[derive(Clone)]
struct SnoozedTaskEntry {
    id: String,
    title: String,
    until: PrimitiveDateTime,
}

type TaskCalendar = Calendar<SnoozedTaskEntry, String, AppMsg>;
type CalendarPane = ResponsiveSplit<TaskCalendar, TaskDetailForm>;

pub(crate) struct CalendarWorkspace {
    context: AppContext,
    pane: CalendarPane,
    observed_version: u64,
    setting_status: SaveStatusLine,
    today: Date,
}

impl CalendarWorkspace {
    pub(crate) fn new(context: AppContext, show_weekends: bool) -> Self {
        let state = context.store.borrow();
        let observed_version = state.state().version;
        let today = current_date();
        let calendar = task_calendar(snoozed_task_entries(&state.state().tasks))
            .today(today)
            .show_weekends(show_weekends);
        let detail = TaskDetailForm::new(
            None,
            &state.state().people,
            &state.state().projects,
            &state.state().tags,
            None,
        );
        drop(state);
        Self {
            context,
            pane: ResponsiveSplit::master_detail(calendar, detail).second_visible(false),
            observed_version,
            setting_status: SaveStatusLine::new(None),
            today,
        }
    }

    fn sync_store_version(&mut self) {
        let state = self.context.store.borrow().state().clone();
        if self.observed_version == state.version {
            return;
        }
        self.observed_version = state.version;
        self.calendar_mut()
            .set_entries(snoozed_task_entries(&state.tasks));
        if let Some(value) = state.app_setting_values.get(SHOW_WEEKENDS_SETTING)
            && let Ok(show) = parse_show_weekends_setting(Some(value))
        {
            self.calendar_mut().set_show_weekends(show);
        }
        self.setting_status.set_error(
            state
                .app_setting_errors
                .get(SHOW_WEEKENDS_SETTING)
                .map(String::as_str),
        );
        self.sync_detail(&state, &mut EventCtx::default());
    }

    fn persist_weekend_visibility_change(&self, previous: bool) {
        let current = self.calendar().is_showing_weekends();
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

    fn sync_today(&mut self, today: Date) -> bool {
        if self.today == today {
            return false;
        }
        self.today = today;
        self.calendar_mut().set_today(today);
        true
    }

    fn calendar(&self) -> &TaskCalendar {
        self.pane.first()
    }

    fn calendar_mut(&mut self) -> &mut TaskCalendar {
        self.pane.first_mut()
    }

    fn detail_mut(&mut self) -> &mut TaskDetailForm {
        self.pane.second_mut()
    }

    fn highlighted_task_id(&self) -> Option<String> {
        (self.calendar().current_view() == CalendarView::Day)
            .then(|| self.calendar().highlighted_entry_id())
            .flatten()
    }

    fn sync_detail(&mut self, state: &crate::domain::AppState, ctx: &mut EventCtx<AppMsg>) -> bool {
        let task_id = self.highlighted_task_id();
        let task = task_id
            .as_deref()
            .and_then(|id| state.tasks.iter().find(|task| task.id == id));
        let save_error = task.and_then(|task| state.task_status_error(&task.id));
        let identity_changed = self.pane.second().task_id.as_deref() != task_id.as_deref()
            || self.pane.second().task_state != task.map(|task| task.state);
        if identity_changed {
            self.detail_mut().set_task(
                task,
                &state.people,
                &state.projects,
                &state.tags,
                save_error,
                ctx,
            );
        } else {
            self.detail_mut().set_save_error(save_error);
        }
        let visibility_changed = self.pane.set_second_visible(task.is_some());
        identity_changed || visibility_changed
    }

    fn sync_calendar_detail(&mut self, ctx: &mut EventCtx<AppMsg>) -> bool {
        let state = self.context.store.borrow().state().clone();
        self.sync_detail(&state, ctx)
    }

    fn drain_detail_patches(&mut self) -> bool {
        let patches = self.detail_mut().take_patches();
        let mut changed = false;
        for (task_id, patch) in patches {
            let outcome =
                self.context
                    .store
                    .borrow_mut()
                    .dispatch(crate::domain::AppEvent::PatchTask {
                        task_id: task_id.clone(),
                        patch: patch.clone(),
                    });
            if outcome.changed {
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::PatchTask(task_id, patch));
                changed = true;
            }
        }
        changed
    }

    fn sync_after_event(&mut self, calendar_handled_event: bool, ctx: &mut EventCtx<AppMsg>) {
        let focus_detail = calendar_handled_event
            && self
                .calendar_mut()
                .take_events()
                .into_iter()
                .any(|event| matches!(event, CalendarTypedEvent::EntryActivated { .. }));
        let detail_changed = self.sync_calendar_detail(ctx);
        let patches_changed = self.drain_detail_patches();
        if detail_changed || patches_changed {
            ctx.request_layout();
            ctx.request_redraw();
        }
        if focus_detail && self.pane.is_second_visible() {
            ctx.focus_next();
            ctx.request_redraw();
        }
    }

    fn handle_task_shortcut(
        &self,
        outcome: EventOutcome,
        event: &TuiEvent,
        return_focus: Option<tuicore::TreePath>,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        if outcome.handled() {
            return outcome;
        }
        let Some(task_id) = self.highlighted_task_id() else {
            return outcome;
        };
        let message = if keys::TASK_SNOOZE.matches(event) {
            Some(AppMsg::OpenTaskSnooze {
                task_id,
                return_focus,
            })
        } else if keys::TASK_DELETE_CTRL_X.matches(event) {
            Some(AppMsg::OpenDeleteTask {
                task_id,
                return_focus,
            })
        } else if keys::TASK_COMPLETE.matches(event) {
            Some(AppMsg::OpenCompleteTask {
                task_id,
                return_focus,
            })
        } else if keys::TASK_TOGGLE_PROGRESS.matches(event) {
            Some(AppMsg::ToggleTaskProgress(task_id))
        } else {
            None
        };
        if let Some(message) = message {
            ctx.emit(message);
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        outcome
    }

    fn focus_calendar(route: &EventRoute, ctx: &mut EventCtx<AppMsg>) {
        let workspace_path = ctx
            .current_path()
            .strip_suffix(&route.path)
            .unwrap_or_default();
        ctx.focus(FocusRequest::TargetAt {
            path: workspace_path.child(ChildKey::first()),
            id: FocusId::new("calendar"),
        });
        ctx.stop_propagation();
        ctx.request_redraw();
    }
}

fn current_date() -> Date {
    OffsetDateTime::now_local()
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .date()
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
    .bordered(false)
    .role(|_| Some(CalendarEntryRole::Muted))
    .event_marker(|_| SNOOZE_ICON)
}

fn is_calendar_view_hotkey(event: &TuiEvent) -> bool {
    let TuiEvent::Key(key) = event else {
        return false;
    };
    let bindings = CalendarKeyBindings::default();
    bindings
        .month_view
        .iter()
        .chain(&bindings.week_view)
        .chain(&bindings.day_view)
        .any(|binding| binding.matches(*key))
}

impl TuiNode<AppMsg> for CalendarWorkspace {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.pane.measure(proposal)
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
        self.pane.layout(calendar_area, ctx);
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
        self.pane.render(frame, calendar_area, ctx);
        if has_error && area.height > 0 {
            self.setting_status.render(
                frame,
                Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
                ctx,
            );
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        let previous = self.calendar().is_showing_weekends();
        let outcome = self.calendar_mut().event(event, ctx);
        self.persist_weekend_visibility_change(previous);
        self.sync_after_event(true, ctx);
        self.handle_task_shortcut(outcome, event, None, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let previous = self.calendar().is_showing_weekends();
        let detail_route = route.path.keys().first() == Some(&ChildKey::second());
        let calendar_event = !detail_route || is_calendar_view_hotkey(event);
        let outcome = if calendar_event {
            self.calendar_mut().event(event, ctx)
        } else {
            self.pane.dispatch_event(route, event, ctx)
        };
        self.persist_weekend_visibility_change(previous);
        self.sync_after_event(calendar_event, ctx);
        if detail_route && detail_escape(event) {
            Self::focus_calendar(route, ctx);
            return EventOutcome::Handled;
        }
        self.handle_task_shortcut(outcome, event, Some(ctx.current_path()), ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        self.pane.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: StdDuration, settings: AnimationSettings) -> TickResult {
        let mut result = self.pane.tick(dt, settings);
        if self.sync_today(current_date()) {
            result = result.merge(TickResult {
                changed: true,
                layout: false,
                active: false,
                next_tick: None,
            });
        }
        result
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.pane.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.pane.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.pane.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.pane.destroy(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{rendered_text, test_context};
    use crate::domain::{AppEvent, TaskPriority, TaskSize, WorkspaceSnapshot};
    use ratatui::{Terminal, backend::TestBackend};
    use time::{Date, Month, Time};
    use tuicore::{AnimationSettings, FocusRequest, Key, KeyEvent, KeyModifiers, TreeDispatcher};

    fn task(id: &str, title: &str, state: TaskState, until: Option<PrimitiveDateTime>) -> Task {
        Task {
            id: id.to_string(),
            rank: 1,
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
            checklist: Vec::new(),
            links: Vec::new(),
            description: String::new(),
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
    fn calendar_workspace_updates_today_without_recreation() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context, true);
        let next_day = workspace.today.next_day().unwrap();

        assert!(workspace.sync_today(next_day));
        assert_eq!(workspace.today, next_day);
        assert!(!workspace.sync_today(next_day));
    }

    #[test]
    fn calendar_preference_defaults_and_rejects_invalid_values() {
        for (value, expected) in [(None, Ok(true)), (Some("weekdays"), Err(()))] {
            assert_eq!(parse_show_weekends_setting(value).map_err(|_| ()), expected);
        }
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
        assert!(!workspace.calendar().is_showing_weekends());
        assert!(context.coordinator.borrow().has_pending());
    }

    #[test]
    fn day_view_shows_highlighted_task_detail_in_responsive_pane() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context.clone(), true);
        let until = workspace.today.with_time(Time::from_hms(8, 0, 0).unwrap());
        let mut snoozed = task("snoozed", "Follow up", TaskState::Snoozed, Some(until));
        snoozed.description = "Calendar task detail".into();
        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(snoozed));
        workspace.sync_store_version();

        workspace.calendar_mut().on_key(Key::Char('d'));
        assert!(workspace.sync_calendar_detail(&mut EventCtx::default()));
        assert_eq!(workspace.pane.second().task_id.as_deref(), Some("snoozed"));
        assert!(workspace.pane.is_second_visible());

        let wide = Rect::new(0, 0, 120, 30);
        workspace.layout(wide, &mut LayoutCtx::new());
        let (wide_calendar, wide_detail) = workspace.pane.child_areas();
        assert_eq!(wide_calendar, Rect::new(0, 0, 72, 30));
        assert_eq!(wide_detail, Rect::new(72, 0, 48, 30));

        let narrow = Rect::new(0, 0, 80, 30);
        workspace.layout(narrow, &mut LayoutCtx::new());
        let (narrow_calendar, narrow_detail) = workspace.pane.child_areas();
        assert_eq!(narrow_calendar.x, narrow_detail.x);
        assert_eq!(narrow_calendar.width, narrow_detail.width);
        assert_eq!(narrow_detail.y, narrow_calendar.bottom());

        let text = rendered_text(&workspace, narrow);
        assert!(text.contains("Calendar task detail"));

        workspace.calendar_mut().on_key(Key::Char('m'));
        workspace.sync_calendar_detail(&mut EventCtx::default());
        assert!(!workspace.pane.is_second_visible());
    }

    #[test]
    fn day_view_updates_detail_when_highlight_moves_between_tasks() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context.clone(), true);
        let first = workspace.today.with_time(Time::from_hms(8, 0, 0).unwrap());
        let second = workspace.today.with_time(Time::from_hms(9, 0, 0).unwrap());
        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(task(
                "first",
                "First",
                TaskState::Snoozed,
                Some(first),
            )));
        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(task(
                "second",
                "Second",
                TaskState::Snoozed,
                Some(second),
            )));
        workspace.sync_store_version();
        workspace.calendar_mut().on_key(Key::Char('d'));
        workspace.sync_calendar_detail(&mut EventCtx::default());
        assert_eq!(workspace.pane.second().task_id.as_deref(), Some("first"));

        workspace.event(&TuiEvent::Key(Key::Down.into()), &mut EventCtx::default());

        assert_eq!(workspace.pane.second().task_id.as_deref(), Some("second"));
    }

    #[test]
    fn activating_day_view_task_moves_focus_into_shared_detail() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context.clone(), true);
        let until = workspace.today.with_time(Time::from_hms(8, 0, 0).unwrap());
        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(task(
                "snoozed",
                "Follow up",
                TaskState::Snoozed,
                Some(until),
            )));
        workspace.sync_store_version();
        workspace.calendar_mut().on_key(Key::Char('d'));
        workspace.sync_calendar_detail(&mut EventCtx::default());
        let mut ctx = EventCtx::default();

        workspace.event(&TuiEvent::Key(Key::Enter.into()), &mut ctx);

        assert_eq!(ctx.focus_request(), Some(&tuicore::FocusRequest::Next));
    }

    #[test]
    fn calendar_task_detail_keeps_task_action_hotkeys() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context.clone(), true);
        let until = workspace.today.with_time(Time::from_hms(8, 0, 0).unwrap());
        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(task(
                "snoozed",
                "Follow up",
                TaskState::Snoozed,
                Some(until),
            )));
        workspace.sync_store_version();
        workspace.calendar_mut().on_key(Key::Char('d'));
        workspace.sync_calendar_detail(&mut EventCtx::default());
        let mut layout = LayoutCtx::new();
        workspace.layout(Rect::new(0, 0, 120, 30), &mut layout);
        let detail_path = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target
                    .path
                    .keys()
                    .iter()
                    .any(|part| part.as_str() == "title")
            })
            .expect("calendar task title should be focusable")
            .path
            .clone();
        let route = EventRoute::new(detail_path.clone());
        let shortcut = |character| {
            TuiEvent::Key(KeyEvent {
                code: Key::Char(character),
                modifiers: KeyModifiers::CONTROL,
            })
        };

        let mut snooze_ctx = EventCtx::default();
        let snooze = workspace.dispatch_event(&route, &shortcut('z'), &mut snooze_ctx);
        assert!(snooze.handled());
        assert!(matches!(
            snooze_ctx.messages(),
            [AppMsg::OpenTaskSnooze { task_id, return_focus: Some(_) }]
                if task_id == "snoozed"
        ));

        let mut delete_ctx = EventCtx::default();
        let delete = workspace.dispatch_event(&route, &shortcut('x'), &mut delete_ctx);
        assert!(delete.handled());
        assert!(matches!(
            delete_ctx.messages(),
            [AppMsg::OpenDeleteTask { task_id, return_focus: Some(_) }]
                if task_id == "snoozed"
        ));

        let mut complete_ctx = EventCtx::default();
        let complete = workspace.dispatch_event(&route, &shortcut('c'), &mut complete_ctx);
        assert!(complete.handled());
        assert!(matches!(
            complete_ctx.messages(),
            [AppMsg::OpenCompleteTask { task_id, return_focus: Some(_) }]
                if task_id == "snoozed"
        ));

        let mut progress_ctx = EventCtx::default();
        let progress = workspace.dispatch_event(&route, &shortcut('t'), &mut progress_ctx);
        assert!(progress.handled());
        assert!(matches!(
            progress_ctx.messages(),
            [AppMsg::ToggleTaskProgress(task_id)] if task_id == "snoozed"
        ));
    }

    #[test]
    fn calendar_task_dialog_shortcuts_preserve_calendar_focus_path() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context.clone(), true);
        let until = workspace.today.with_time(Time::from_hms(8, 0, 0).unwrap());
        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(task(
                "snoozed",
                "Follow up",
                TaskState::Snoozed,
                Some(until),
            )));
        workspace.sync_store_version();
        workspace.calendar_mut().on_key(Key::Char('d'));
        workspace.sync_calendar_detail(&mut EventCtx::default());
        let mut layout = LayoutCtx::new();
        workspace.layout(Rect::new(0, 0, 120, 30), &mut layout);
        let calendar_path = layout
            .focus_targets()
            .iter()
            .find(|target| target.id.as_str() == "calendar")
            .expect("calendar should be focusable")
            .path
            .clone();
        let route = EventRoute::new(calendar_path.clone());

        for key in ['z', 'x', 'c'] {
            let effects = TreeDispatcher::new().dispatch_event(
                &mut workspace,
                &route,
                &TuiEvent::Key(KeyEvent {
                    code: Key::Char(key),
                    modifiers: KeyModifiers::CONTROL,
                }),
                AnimationSettings::default(),
            );
            let return_focus = match (key, effects.messages.as_slice()) {
                ('z', [AppMsg::OpenTaskSnooze { return_focus, .. }])
                | ('x', [AppMsg::OpenDeleteTask { return_focus, .. }])
                | ('c', [AppMsg::OpenCompleteTask { return_focus, .. }]) => return_focus,
                _ => panic!("calendar shortcut should open its task dialog"),
            };
            assert_eq!(return_focus.as_ref(), Some(&calendar_path));
        }
    }

    #[test]
    fn calendar_view_hotkeys_work_while_task_detail_is_focused() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context.clone(), true);
        let until = workspace.today.with_time(Time::from_hms(8, 0, 0).unwrap());
        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(task(
                "snoozed",
                "Follow up",
                TaskState::Snoozed,
                Some(until),
            )));
        workspace.sync_store_version();
        workspace.calendar_mut().on_key(Key::Char('d'));
        workspace.sync_calendar_detail(&mut EventCtx::default());
        let mut layout = LayoutCtx::new();
        workspace.layout(Rect::new(0, 0, 120, 30), &mut layout);
        let detail_path = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target
                    .path
                    .keys()
                    .iter()
                    .any(|part| part.as_str() == "title")
            })
            .expect("calendar task title should be focusable")
            .path
            .clone();
        let route = EventRoute::new(detail_path);

        for (key, expected) in [
            ('m', CalendarView::Month),
            ('w', CalendarView::Week),
            ('d', CalendarView::Day),
        ] {
            workspace.dispatch_event(
                &route,
                &TuiEvent::Key(Key::Char(key).into()),
                &mut EventCtx::default(),
            );
            assert_eq!(workspace.calendar().current_view(), expected);
        }
    }

    #[test]
    fn closing_calendar_task_detail_focuses_calendar_directly() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context.clone(), true);
        let until = workspace.today.with_time(Time::from_hms(8, 0, 0).unwrap());
        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(task(
                "snoozed",
                "Follow up",
                TaskState::Snoozed,
                Some(until),
            )));
        workspace.sync_store_version();
        workspace.calendar_mut().on_key(Key::Char('d'));
        workspace.sync_calendar_detail(&mut EventCtx::default());
        let mut layout = LayoutCtx::new();
        workspace.layout(Rect::new(0, 0, 120, 30), &mut layout);
        let detail_target = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target
                    .path
                    .keys()
                    .iter()
                    .any(|part| part.as_str() == "title")
            })
            .expect("calendar task title should be focusable");
        let calendar_target = layout
            .focus_targets()
            .iter()
            .find(|target| target.id.as_str() == "calendar")
            .expect("calendar should be focusable");
        let route = EventRoute::new(detail_target.path.clone());
        let expected = FocusRequest::TargetAt {
            path: calendar_target.path.clone(),
            id: calendar_target.id.clone(),
        };
        let close_keys = [
            KeyEvent::from(Key::Esc),
            KeyEvent {
                code: Key::Char('['),
                modifiers: KeyModifiers::CONTROL,
            },
        ];

        for key in close_keys {
            let effects = TreeDispatcher::new().dispatch_event(
                &mut workspace,
                &route,
                &TuiEvent::Key(key),
                AnimationSettings::default(),
            );

            assert!(effects.outcome.handled());
            assert_eq!(effects.focus_request, Some(expected.clone()));
        }
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
