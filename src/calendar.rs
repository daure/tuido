use std::{cell::RefCell, rc::Rc, time::Duration as StdDuration};

use ratatui::{Frame, layout::Rect, widgets::Clear};
use time::{Date, Duration, OffsetDateTime, PrimitiveDateTime};
use tuicore::{
    AnimationSettings, Calendar, CalendarEntryRole, CalendarKeyBindings, CalendarSpan,
    CalendarTypedEvent, CalendarView, ChildKey, EventCtx, EventOutcome, EventRoute, FocusCtx,
    FocusId, FocusRequest, FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint,
    LifecycleCtx, RenderCtx, SeasonalEmptyState, TickResult, TuiEvent, TuiNode,
};

use crate::app::{
    ActiveLabelFilter, ActiveProjectFilter, AppContext, AppMsg, persist_task_order,
    task_detail::detail_escape, task_ids_at_snooze_time,
};
use crate::app_keymap::keys;
use crate::domain::{Task, TaskState};
use crate::persistence_coordinator::PersistenceCommand;
use crate::ui::responsive_split::ResponsiveSplit;
use crate::ui::save_status::SaveStatusLine;
use crate::ui::task_detail::TaskDetailForm;

const SNOOZE_ICON: char = '󰒲';
pub(crate) const SHOW_WEEKENDS_SETTING: &str = "calendar.show_weekends";

#[derive(Clone)]
pub(crate) struct CalendarCreateContext {
    selected_date: Rc<RefCell<Date>>,
    pending_task_id: Rc<RefCell<Option<String>>>,
}

impl CalendarCreateContext {
    pub(crate) fn new() -> Self {
        Self {
            selected_date: Rc::new(RefCell::new(current_date())),
            pending_task_id: Rc::new(RefCell::new(None)),
        }
    }

    pub(crate) fn selected_date(&self) -> Date {
        *self.selected_date.borrow()
    }

    pub(crate) fn select_created_task(&self, task_id: String) {
        *self.pending_task_id.borrow_mut() = Some(task_id);
    }

    fn set_selected_date(&self, date: Date) {
        *self.selected_date.borrow_mut() = date;
    }

    fn take_created_task(&self) -> Option<String> {
        self.pending_task_id.borrow_mut().take()
    }
}

#[derive(Clone)]
struct SnoozedTaskEntry {
    id: String,
    title: String,
    until: PrimitiveDateTime,
    rank: i64,
}

type TaskCalendar = Calendar<SnoozedTaskEntry, String, AppMsg>;
type CalendarPane = ResponsiveSplit<TaskCalendar, TaskDetailForm>;

pub(crate) struct CalendarWorkspace {
    context: AppContext,
    create_context: CalendarCreateContext,
    pane: CalendarPane,
    visible_entries: Vec<SnoozedTaskEntry>,
    empty_state: SeasonalEmptyState,
    observed_version: u64,
    setting_status: SaveStatusLine,
    today: Date,
    reordering: bool,
    project_filter: Option<String>,
    label_filter: Vec<String>,
    active_project_filter: ActiveProjectFilter,
    active_label_filter: ActiveLabelFilter,
}

impl CalendarWorkspace {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new(context: AppContext, show_weekends: bool) -> Self {
        Self::new_with_create_context(context, show_weekends, CalendarCreateContext::new())
    }

    pub(crate) fn new_with_create_context(
        context: AppContext,
        show_weekends: bool,
        create_context: CalendarCreateContext,
    ) -> Self {
        Self::new_with_create_context_and_filters(
            context,
            show_weekends,
            create_context,
            Rc::new(RefCell::new(None)),
            Rc::new(RefCell::new(Vec::new())),
        )
    }

    pub(crate) fn new_with_create_context_and_filters(
        context: AppContext,
        show_weekends: bool,
        create_context: CalendarCreateContext,
        active_project_filter: ActiveProjectFilter,
        active_label_filter: ActiveLabelFilter,
    ) -> Self {
        let state = context.store.borrow();
        let observed_version = state.state().version;
        let today = current_date();
        let project_filter = active_project_filter.borrow().clone();
        let label_filter = active_label_filter.borrow().clone();
        let visible_entries = filtered_snoozed_task_entries(
            &state.state().tasks,
            project_filter.as_deref(),
            &label_filter,
        );
        let calendar = task_calendar(visible_entries.clone())
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
            create_context,
            pane: ResponsiveSplit::master_detail(calendar, detail).second_visible(false),
            visible_entries,
            empty_state: SeasonalEmptyState::new("No tasks scheduled for this day"),
            observed_version,
            setting_status: SaveStatusLine::new(None),
            today,
            reordering: false,
            project_filter,
            label_filter,
            active_project_filter,
            active_label_filter,
        }
    }

    fn sync_store_version(&mut self) {
        let state = self.context.store.borrow().state().clone();
        let filter_options_changed = self.sync_filter_options(&state);
        if self.observed_version == state.version && !filter_options_changed {
            return;
        }
        self.observed_version = state.version;
        let entries = filtered_snoozed_task_entries(
            &state.tasks,
            self.project_filter.as_deref(),
            &self.label_filter,
        );
        self.set_calendar_entries(entries);
        self.select_created_task();
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

    fn sync_filter_options(&mut self, state: &crate::domain::AppState) -> bool {
        let mut changed = false;
        if self
            .project_filter
            .as_ref()
            .is_some_and(|id| !state.projects.iter().any(|project| project.id == *id))
        {
            self.project_filter = None;
            *self.active_project_filter.borrow_mut() = None;
            changed = true;
        }
        let previous_labels = self.label_filter.len();
        self.label_filter
            .retain(|id| state.tags.iter().any(|tag| tag.id == *id));
        if self.label_filter.len() != previous_labels {
            *self.active_label_filter.borrow_mut() = self.label_filter.clone();
            changed = true;
        }
        changed
    }

    fn sync_filter_change(&mut self) -> bool {
        let project_filter = self.active_project_filter.borrow().clone();
        let label_filter = self.active_label_filter.borrow().clone();
        if project_filter == self.project_filter && label_filter == self.label_filter {
            return false;
        }
        self.project_filter = project_filter;
        self.label_filter = label_filter;
        let state = self.context.store.borrow().state().clone();
        let entries = filtered_snoozed_task_entries(
            &state.tasks,
            self.project_filter.as_deref(),
            &self.label_filter,
        );
        self.set_calendar_entries(entries);
        self.sync_detail(&state, &mut EventCtx::default());
        true
    }

    fn set_calendar_entries(&mut self, entries: Vec<SnoozedTaskEntry>) {
        let replacement = self.removed_entry_replacement(&entries);
        self.visible_entries = entries.clone();
        self.calendar_mut().set_entries(entries);
        if let Some(task_id) = replacement {
            self.select_task_on_current_day(&task_id);
        }
    }

    fn removed_entry_replacement(&self, entries: &[SnoozedTaskEntry]) -> Option<String> {
        if self.calendar().current_view() != CalendarView::Day {
            return None;
        }
        let selected = self.calendar().highlighted_entry_id()?;
        let date = self.calendar().cursor_date();
        let previous = day_entry_ids(&self.visible_entries, date);
        let selected_index = previous.iter().position(|id| id == &selected)?;
        let current = day_entry_ids(entries, date);
        if current.contains(&selected) {
            return None;
        }
        current
            .get(selected_index)
            .or_else(|| current.last())
            .cloned()
    }

    fn select_created_task(&mut self) {
        let Some(task_id) = self.create_context.take_created_task() else {
            return;
        };
        let scheduled_date = self
            .context
            .store
            .borrow()
            .state()
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .filter(|task| {
                task.state == TaskState::Snoozed
                    && task_matches_filters(
                        task,
                        self.project_filter.as_deref(),
                        &self.label_filter,
                    )
            })
            .and_then(|task| task.snoozed_until)
            .map(|until| until.date());
        let Some(scheduled_date) = scheduled_date else {
            return;
        };
        self.calendar_mut().on_key(tuicore::Key::Char('D'));
        self.move_cursor_to_date(scheduled_date);
        self.select_task_on_current_day(&task_id);
        self.sync_selected_date();
    }

    fn move_cursor_to_date(&mut self, date: Date) {
        let days = (date - self.calendar().cursor_date()).whole_days();
        let key = if days.is_negative() {
            tuicore::Key::Left
        } else {
            tuicore::Key::Right
        };
        for _ in 0..days.unsigned_abs() {
            self.calendar_mut().on_key(key);
        }
    }

    fn select_task_on_current_day(&mut self, task_id: &str) {
        let entry_count = day_entry_ids(&self.visible_entries, self.calendar().cursor_date()).len();
        for _ in 0..entry_count {
            if self.calendar().highlighted_entry_id().as_deref() == Some(task_id) {
                break;
            }
            self.calendar_mut().on_key(tuicore::Key::Down);
        }
    }

    fn sync_selected_date(&self) {
        self.create_context
            .set_selected_date(self.calendar().cursor_date());
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

    fn day_is_empty(&self) -> bool {
        if self.calendar().current_view() != CalendarView::Day {
            return false;
        }
        let date = self.calendar().cursor_date();
        !self
            .context
            .store
            .borrow()
            .state()
            .tasks
            .iter()
            .any(|task| {
                task.state == TaskState::Snoozed
                    && task_matches_filters(
                        task,
                        self.project_filter.as_deref(),
                        &self.label_filter,
                    )
                    && task
                        .snoozed_until
                        .is_some_and(|snoozed_until| snoozed_until.date() == date)
            })
    }

    fn empty_day_message(&self) -> &'static str {
        if self.calendar().cursor_date() == self.today {
            "No tasks scheduled for today"
        } else {
            "No tasks scheduled for this day"
        }
    }

    fn sync_empty_day_message(&mut self) {
        self.empty_state.set_message(self.empty_day_message());
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
        let message = if keys::TASK_QUICK_MENU.matches(event) {
            self.context
                .store
                .borrow()
                .state()
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .and_then(|task| task.snoozed_until)
                .map(|time| AppMsg::OpenCalendarTaskQuickMenu { task_id, time })
        } else if keys::TASK_SNOOZE.matches(event) {
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

    fn handle_move_mode(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> Option<EventOutcome> {
        if !self.reordering {
            if !keys::TASK_MOVE_MODE.matches(event) {
                return None;
            }
            let task_id = self.highlighted_task_id()?;
            let state = self.context.store.borrow();
            let task = state.state().tasks.iter().find(|task| task.id == task_id)?;
            let time = task.snoozed_until?;
            if task_ids_at_snooze_time(state.state(), time).len() < 2 {
                ctx.notify(tuicore::Notification::warning(
                    "Task cannot move",
                    "No other tasks are scheduled at the same time.",
                ));
                ctx.stop_propagation();
                return Some(EventOutcome::Handled);
            }
            drop(state);
            self.reordering = true;
            ctx.request_redraw();
            ctx.stop_propagation();
            return Some(EventOutcome::Handled);
        }

        if keys::TASK_MOVE_MODE.matches(event)
            || matches!(event, TuiEvent::Key(key) if key.code == tuicore::Key::Enter)
            || detail_escape(event)
        {
            self.reordering = false;
            ctx.request_redraw();
            ctx.stop_propagation();
            return Some(EventOutcome::Handled);
        }

        let direction = calendar_move_direction(event)?;
        self.move_highlighted_task(direction);
        self.sync_store_version();
        ctx.request_layout();
        ctx.request_redraw();
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }

    fn handle_month_escape(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> Option<EventOutcome> {
        if self.calendar().current_view() != CalendarView::Month || !detail_escape(event) {
            return None;
        }
        self.calendar_mut().on_key(tuicore::Key::Char('T'));
        self.sync_selected_date();
        ctx.request_redraw();
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }

    fn move_highlighted_task(&mut self, direction: isize) -> bool {
        let Some(task_id) = self.highlighted_task_id() else {
            return false;
        };
        let state = self.context.store.borrow().state().clone();
        let Some(time) = state
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .and_then(|task| task.snoozed_until)
        else {
            return false;
        };
        let mut ordered = task_ids_at_snooze_time(&state, time);
        let Some(index) = ordered.iter().position(|id| id == &task_id) else {
            return false;
        };
        let next = index
            .saturating_add_signed(direction)
            .min(ordered.len() - 1);
        if next == index {
            return false;
        }
        ordered.swap(index, next);
        persist_task_order(&self.context, &state, &ordered)
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

#[cfg(test)]
fn snoozed_task_entries(tasks: &[Task]) -> Vec<SnoozedTaskEntry> {
    tasks.iter().filter_map(snoozed_task_entry).collect()
}

fn filtered_snoozed_task_entries(
    tasks: &[Task],
    project_filter: Option<&str>,
    label_filter: &[String],
) -> Vec<SnoozedTaskEntry> {
    tasks
        .iter()
        .filter(|task| task_matches_filters(task, project_filter, label_filter))
        .filter_map(snoozed_task_entry)
        .collect()
}

fn snoozed_task_entry(task: &Task) -> Option<SnoozedTaskEntry> {
    (task.state == TaskState::Snoozed).then_some(SnoozedTaskEntry {
        id: task.id.clone(),
        title: task.title.clone(),
        until: task.snoozed_until?,
        rank: task.rank,
    })
}

fn task_matches_filters(
    task: &Task,
    project_filter: Option<&str>,
    label_filter: &[String],
) -> bool {
    project_filter.is_none_or(|project_id| task.project_id.as_deref() == Some(project_id))
        && label_filter
            .iter()
            .all(|tag_id| task.tag_ids.contains(tag_id))
}

fn task_calendar(entries: Vec<SnoozedTaskEntry>) -> TaskCalendar {
    Calendar::new(
        entries,
        |entry| entry.id.clone(),
        |entry| CalendarSpan::timed(entry.until, entry.until + Duration::minutes(1)),
        |entry| entry.title.clone(),
    )
    .bordered(false)
    .entry_order(compare_snoozed_task_entries)
    .role(|_| Some(CalendarEntryRole::Muted))
    .event_marker(|_| SNOOZE_ICON)
}

fn compare_snoozed_task_entries(
    left: &SnoozedTaskEntry,
    right: &SnoozedTaskEntry,
) -> std::cmp::Ordering {
    left.rank
        .cmp(&right.rank)
        .then_with(|| left.title.cmp(&right.title))
}

fn day_entry_ids(entries: &[SnoozedTaskEntry], date: Date) -> Vec<String> {
    let mut entries = entries
        .iter()
        .filter(|entry| entry.until.date() == date)
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| compare_snoozed_task_entries(left, right));
    entries.into_iter().map(|entry| entry.id.clone()).collect()
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

fn calendar_move_direction(event: &TuiEvent) -> Option<isize> {
    let TuiEvent::Key(key) = event else {
        return None;
    };
    let bindings = CalendarKeyBindings::default();
    if bindings.up.iter().any(|binding| binding.matches(*key)) {
        Some(-1)
    } else if bindings.down.iter().any(|binding| binding.matches(*key)) {
        Some(1)
    } else {
        None
    }
}

fn unbordered_calendar_content_area(area: Rect) -> Rect {
    Rect::new(
        area.x,
        area.y.saturating_add(1),
        area.width,
        area.height.saturating_sub(1),
    )
}

impl TuiNode<AppMsg> for CalendarWorkspace {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.pane.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync_store_version();
        self.sync_filter_change();
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
        if self.day_is_empty() {
            self.sync_empty_day_message();
            <SeasonalEmptyState as TuiNode<AppMsg>>::layout(
                &mut self.empty_state,
                unbordered_calendar_content_area(calendar_area),
                ctx,
            );
        }
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
        if self.day_is_empty() {
            let empty_area = unbordered_calendar_content_area(calendar_area);
            frame.render_widget(Clear, empty_area);
            <SeasonalEmptyState as TuiNode<AppMsg>>::render(
                &self.empty_state,
                frame,
                empty_area,
                ctx,
            );
        }
        if has_error && area.height > 0 {
            self.setting_status.render(
                frame,
                Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
                ctx,
            );
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        if let Some(outcome) = self.handle_month_escape(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_move_mode(event, ctx) {
            return outcome;
        }
        let previous = self.calendar().is_showing_weekends();
        let outcome = self.calendar_mut().event(event, ctx);
        self.sync_selected_date();
        self.sync_empty_day_message();
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
        if let Some(outcome) = self.handle_month_escape(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_move_mode(event, ctx) {
            return outcome;
        }
        let previous = self.calendar().is_showing_weekends();
        let detail_route = route.path.keys().first() == Some(&ChildKey::second());
        let calendar_event = !detail_route || is_calendar_view_hotkey(event);
        let outcome = if calendar_event {
            self.calendar_mut().event(event, ctx)
        } else {
            self.pane.dispatch_event(route, event, ctx)
        };
        self.sync_selected_date();
        self.sync_empty_day_message();
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
        if self.day_is_empty() {
            result = result.merge(<SeasonalEmptyState as TuiNode<AppMsg>>::tick(
                &mut self.empty_state,
                dt,
                settings,
            ));
        }
        if self.sync_today(current_date()) {
            self.sync_selected_date();
            self.sync_empty_day_message();
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
    use crate::domain::{AppEvent, Project, Tag, TaskPriority, TaskSize, WorkspaceSnapshot};
    use ratatui::{Terminal, backend::TestBackend};
    use time::{Date, Month, Time};
    use tuicore::{
        AnimationSettings, FocusRequest, Key, KeyEvent, KeyModifiers, Propagation, TreeDispatcher,
    };

    fn task(id: &str, title: &str, state: TaskState, until: Option<PrimitiveDateTime>) -> Task {
        Task {
            id: id.to_string(),
            rank: 1,
            created_at: String::new(),
            updated_at: String::new(),
            title: title.to_string(),
            state,
            size: TaskSize::Small,
            priority: TaskPriority::Medium,
            snoozed_until: until,
            people_ids: Vec::new(),
            project_id: None,
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
    fn calendar_applies_shared_project_and_label_filters() {
        let until = current_date().with_time(Time::from_hms(8, 0, 0).unwrap());
        let mut matching = task("matching", "Matching", TaskState::Snoozed, Some(until));
        matching.project_id = Some("project-2".into());
        matching.tag_ids = vec!["api".into(), "urgent".into()];
        let mut wrong_labels = task(
            "wrong-labels",
            "Wrong labels",
            TaskState::Snoozed,
            Some(until),
        );
        wrong_labels.project_id = Some("project-2".into());
        wrong_labels.tag_ids = vec!["api".into()];
        let mut wrong_project = task(
            "wrong-project",
            "Wrong project",
            TaskState::Snoozed,
            Some(until),
        );
        wrong_project.project_id = Some("project-1".into());
        wrong_project.tag_ids = vec!["api".into(), "urgent".into()];
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![matching, wrong_labels, wrong_project],
            people: Vec::new(),
            projects: vec![
                Project::new(
                    "project-1".into(),
                    "ONE".into(),
                    "One".into(),
                    String::new(),
                ),
                Project::new(
                    "project-2".into(),
                    "TWO".into(),
                    "Two".into(),
                    String::new(),
                ),
            ],
            tags: vec![
                Tag::new("api".into(), "API".into()),
                Tag::new("urgent".into(), "Urgent".into()),
            ],
        });
        let project_filter = Rc::new(RefCell::new(Some("project-2".into())));
        let label_filter = Rc::new(RefCell::new(vec!["api".into(), "urgent".into()]));
        let mut workspace = CalendarWorkspace::new_with_create_context_and_filters(
            context,
            true,
            CalendarCreateContext::new(),
            Rc::clone(&project_filter),
            Rc::clone(&label_filter),
        );
        workspace.calendar_mut().on_key(Key::Char('D'));
        let area = Rect::new(0, 0, 80, 20);
        workspace.layout(area, &mut LayoutCtx::new());

        let filtered = rendered_text(&workspace, area);
        assert!(filtered.contains("Matching"));
        assert!(!filtered.contains("Wrong labels"));
        assert!(!filtered.contains("Wrong project"));

        *project_filter.borrow_mut() = None;
        label_filter.borrow_mut().clear();
        assert!(workspace.sync_filter_change());
        let unfiltered = rendered_text(&workspace, area);
        assert!(unfiltered.contains("Matching"));
        assert!(unfiltered.contains("Wrong labels"));
        assert!(unfiltered.contains("Wrong project"));
    }

    #[test]
    fn empty_day_shows_scheduled_task_empty_state_without_hiding_calendar() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context, true);
        let area = Rect::new(0, 0, 80, 20);
        workspace.calendar_mut().on_key(Key::Char('D'));
        workspace.layout(area, &mut LayoutCtx::new());

        let text = rendered_text(&workspace, area);

        assert!(text.contains(&workspace.today.to_string()));
        assert!(text.contains("Day |D| · Week |W| · Month |M|"));
        assert!(text.contains("No tasks scheduled for today"));
        assert!(!text.contains("No entries"));

        workspace.event(&TuiEvent::Key(Key::Left.into()), &mut EventCtx::default());
        let previous_day_text = rendered_text(&workspace, area);
        assert!(previous_day_text.contains("No tasks scheduled for this day"));

        workspace.event(&TuiEvent::Key(Key::Right.into()), &mut EventCtx::default());
        let today_text = rendered_text(&workspace, area);
        assert!(today_text.contains("No tasks scheduled for today"));

        workspace.calendar_mut().on_key(Key::Char('M'));
        let month_text = rendered_text(&workspace, area);
        assert!(!month_text.contains("No tasks scheduled for today"));
        assert!(!month_text.contains("No tasks scheduled for this day"));
    }

    #[test]
    fn created_calendar_task_opens_day_view_and_becomes_highlighted() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let create_context = CalendarCreateContext::new();
        let mut workspace = CalendarWorkspace::new_with_create_context(
            context.clone(),
            true,
            create_context.clone(),
        );
        workspace.event(&TuiEvent::Key(Key::Right.into()), &mut EventCtx::default());
        let selected_date = create_context.selected_date();
        let scheduled_date = selected_date + Duration::weeks(1);
        let until = scheduled_date.with_time(Time::from_hms(8, 0, 0).unwrap());
        let created = task("created", "Created", TaskState::Snoozed, Some(until));
        create_context.select_created_task(created.id.clone());
        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(created));

        workspace.sync_store_version();

        assert_eq!(workspace.calendar().current_view(), CalendarView::Day);
        assert_eq!(workspace.calendar().cursor_date(), scheduled_date);
        assert_eq!(
            workspace.calendar().highlighted_entry_id().as_deref(),
            Some("created")
        );
        assert_eq!(workspace.pane.second().task_id.as_deref(), Some("created"));
    }

    #[test]
    fn removing_day_view_tasks_selects_next_then_previous_then_nothing() {
        let until = current_date().with_time(Time::from_hms(8, 0, 0).unwrap());
        let tasks = [
            ("first", "First", 1),
            ("second", "Second", 2),
            ("third", "Third", 3),
        ]
        .into_iter()
        .map(|(id, title, rank)| {
            let mut task = task(id, title, TaskState::Snoozed, Some(until));
            task.rank = rank;
            task
        })
        .collect();
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks,
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context.clone(), true);
        workspace.calendar_mut().on_key(Key::Char('D'));
        workspace.calendar_mut().on_key(Key::Down);
        assert_eq!(workspace.highlighted_task_id().as_deref(), Some("second"));

        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskDeleted("second".into()));
        workspace.sync_store_version();
        assert_eq!(workspace.highlighted_task_id().as_deref(), Some("third"));

        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskDeleted("third".into()));
        workspace.sync_store_version();
        assert_eq!(workspace.highlighted_task_id().as_deref(), Some("first"));

        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskDeleted("first".into()));
        workspace.sync_store_version();
        assert_eq!(workspace.highlighted_task_id(), None);
        assert!(!workspace.pane.is_second_visible());
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
    fn escape_in_month_view_returns_to_today_without_unfocusing_calendar() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context, true);
        let close_keys = [
            KeyEvent::from(Key::Esc),
            KeyEvent {
                code: Key::Char('['),
                modifiers: KeyModifiers::CONTROL,
            },
        ];

        for key in close_keys {
            workspace.calendar_mut().on_key(Key::Right);
            assert_ne!(workspace.calendar().cursor_date(), workspace.today);
            let mut ctx = EventCtx::default();

            let outcome = workspace.event(&TuiEvent::Key(key), &mut ctx);

            assert!(outcome.handled());
            assert_eq!(workspace.calendar().current_view(), CalendarView::Month);
            assert_eq!(workspace.calendar().cursor_date(), workspace.today);
            assert_eq!(ctx.propagation(), Propagation::Stopped);
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

        workspace.calendar_mut().on_key(Key::Char('D'));
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

        workspace.calendar_mut().on_key(Key::Char('M'));
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
        workspace.calendar_mut().on_key(Key::Char('D'));
        workspace.sync_calendar_detail(&mut EventCtx::default());
        assert_eq!(workspace.pane.second().task_id.as_deref(), Some("first"));

        workspace.event(&TuiEvent::Key(Key::Down.into()), &mut EventCtx::default());

        assert_eq!(workspace.pane.second().task_id.as_deref(), Some("second"));
    }

    #[test]
    fn day_view_quick_menu_targets_the_highlighted_task() {
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
                "highlighted",
                "Highlighted",
                TaskState::Snoozed,
                Some(until),
            )));
        workspace.sync_store_version();
        workspace.calendar_mut().on_key(Key::Char('D'));
        workspace.sync_calendar_detail(&mut EventCtx::default());
        let mut ctx = EventCtx::default();

        let outcome = workspace.event(&TuiEvent::Key(Key::Char('.').into()), &mut ctx);

        assert!(outcome.handled());
        assert!(matches!(
            ctx.messages(),
            [AppMsg::OpenCalendarTaskQuickMenu { task_id, time }] if task_id == "highlighted" && *time == until
        ));
    }

    #[test]
    fn day_view_move_mode_reorders_only_tasks_at_the_same_time() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let eight = workspace_time(8);
        let nine = workspace_time(9);
        for (id, title, rank, until) in [
            ("first", "First", 1, eight),
            ("second", "Second", 2, eight),
            ("third", "Third", 3, eight),
            ("later", "Later", 4, nine),
        ] {
            let mut entry = task(id, title, TaskState::Snoozed, Some(until));
            entry.rank = rank;
            context
                .store
                .borrow_mut()
                .dispatch(AppEvent::TaskCreated(entry));
        }
        let mut workspace = CalendarWorkspace::new(context, true);
        workspace.calendar_mut().on_key(Key::Char('D'));
        workspace.calendar_mut().on_key(KeyEvent::from(Key::Down));

        for key in [
            KeyEvent {
                code: Key::Char('m'),
                modifiers: KeyModifiers::CONTROL,
            },
            KeyEvent::from(Key::Down),
            KeyEvent::from(Key::Enter),
        ] {
            workspace.event(&TuiEvent::Key(key), &mut EventCtx::default());
        }

        let state = store.borrow();
        let rank = |id: &str| {
            state
                .state()
                .tasks
                .iter()
                .find(|task| task.id == id)
                .unwrap()
                .rank
        };
        assert_eq!(rank("first"), 1);
        assert_eq!(rank("second"), 3);
        assert_eq!(rank("third"), 2);
        assert_eq!(rank("later"), 4);
        assert_eq!(workspace.highlighted_task_id().as_deref(), Some("second"));
        let text = rendered_text(workspace.calendar(), Rect::new(0, 0, 80, 20));
        assert!(text.find("Third").unwrap() < text.find("Second").unwrap());
    }

    fn workspace_time(hour: u8) -> PrimitiveDateTime {
        current_date().with_time(Time::from_hms(hour, 0, 0).unwrap())
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
        workspace.calendar_mut().on_key(Key::Char('D'));
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
        workspace.calendar_mut().on_key(Key::Char('D'));
        workspace.sync_calendar_detail(&mut EventCtx::default());
        let mut layout = LayoutCtx::new();
        workspace.layout(Rect::new(0, 0, 120, 30), &mut layout);
        assert!(layout.focus_targets().iter().any(|target| {
            target
                .hotkey_sequences
                .contains(&keys::TASK_TAGS_FIELD.hotkey())
        }));
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
        workspace.calendar_mut().on_key(Key::Char('D'));
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
        workspace.calendar_mut().on_key(Key::Char('D'));
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
            ('M', CalendarView::Month),
            ('W', CalendarView::Week),
            ('D', CalendarView::Day),
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
        workspace.calendar_mut().on_key(Key::Char('D'));
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
