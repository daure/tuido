use std::{cell::RefCell, rc::Rc, time::Duration as StdDuration};

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Clear,
};
use time::{Date, Duration, OffsetDateTime, PrimitiveDateTime};
use tuicore::{
    AnimationSettings, Calendar, CalendarKeyBindings, CalendarSpan, CalendarTypedEvent,
    CalendarView, ChildKey, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId, FocusRequest,
    FocusTarget, Key, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    Propagation, RenderCtx, SeasonalEmptyState, TickResult, TuiEvent, TuiNode,
};

use crate::app::{
    ActiveLabelFilter, ActiveWorkspaceFilter, AppContext, AppMsg, SnoozeReturnFocus,
    TransientSelectionSource, persist_task_order, task_agent_commands_for,
    task_detail::{chip_line, detail_escape, priority_icon_line, task_title_prefix_line},
    task_references_for,
};
use crate::app_keymap::keys;
use crate::domain::{Task, TaskPriority, TaskSize, TaskState, Workspace};
use crate::persistence_coordinator::{PersistenceCommand, PersistenceSelectionInvocation};
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
    display_id: String,
    priority: TaskPriority,
    size: TaskSize,
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
    workspace_filter: Option<String>,
    label_filter: Vec<String>,
    active_workspace_filter: ActiveWorkspaceFilter,
    active_label_filter: ActiveLabelFilter,
    active_selection_invocation: Option<PersistenceSelectionInvocation>,
}

impl CalendarWorkspace {
    #[cfg(test)]
    pub(crate) fn set_active_selection_invocation_for_test(
        &mut self,
        invocation: Option<PersistenceSelectionInvocation>,
    ) {
        self.active_selection_invocation = invocation;
    }

    #[cfg(test)]
    pub(crate) fn active_selection_invocation_for_test(
        &self,
    ) -> Option<PersistenceSelectionInvocation> {
        self.active_selection_invocation
    }

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
        active_workspace_filter: ActiveWorkspaceFilter,
        active_label_filter: ActiveLabelFilter,
    ) -> Self {
        let state = context.store.borrow();
        let observed_version = state.state().version;
        let today = current_date();
        let workspace_filter = active_workspace_filter.borrow().clone();
        let label_filter = active_label_filter.borrow().clone();
        let visible_entries = filtered_snoozed_task_entries(
            &state.state().tasks,
            &state.state().workspaces,
            workspace_filter.as_deref(),
            &label_filter,
        );
        let calendar = task_calendar(visible_entries.clone())
            .today(today)
            .show_weekends(show_weekends);
        let detail = TaskDetailForm::new(
            None,
            &state.state().tasks,
            &state.state().people,
            &state.state().workspaces,
            &state.state().tags,
            None,
        );
        drop(state);
        Self {
            context,
            create_context,
            pane: ResponsiveSplit::master_detail(calendar, detail)
                .wide_ratio(45, 55)
                .narrow_second_max_percent(75)
                .narrow_first_percent_space_between(25)
                .second_visible(false),
            visible_entries,
            empty_state: SeasonalEmptyState::new("No tasks scheduled for this day"),
            observed_version,
            setting_status: SaveStatusLine::new(None),
            today,
            workspace_filter,
            label_filter,
            active_workspace_filter,
            active_label_filter,
            active_selection_invocation: None,
        }
    }

    pub(crate) fn sync_store_version(&mut self) {
        self.sync_store_version_with_ctx(&mut EventCtx::default());
    }

    fn sync_store_version_with_ctx(&mut self, ctx: &mut EventCtx<AppMsg>) -> bool {
        let state = self.context.store.borrow().state().clone();
        let filter_options_changed = self.sync_filter_options(&state);
        self.context.resolve_persistence_selection_outcomes();
        if let Some(invocation) = self.active_selection_invocation
            && self.context.take_selection_clear_request(invocation)
        {
            self.calendar_mut().clear_transient_selection();
        }
        if self.observed_version == state.version && !filter_options_changed {
            return false;
        }
        let rollback_highlight = self.active_selection_invocation.and_then(|invocation| {
            self.context
                .rollback_transient_selection_highlight(invocation)
        });
        self.observed_version = state.version;
        let entries = filtered_snoozed_task_entries(
            &state.tasks,
            &state.workspaces,
            self.workspace_filter.as_deref(),
            &self.label_filter,
        );
        self.set_calendar_entries(entries);
        if let Some(task_id) = rollback_highlight {
            if self.visible_entries.iter().any(|entry| entry.id == task_id) {
                self.calendar_mut().highlight_entry_id(&task_id);
            }
        }
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
        self.sync_detail(&state, ctx)
    }

    fn sync_filter_options(&mut self, state: &crate::domain::AppState) -> bool {
        let mut changed = false;
        if self
            .workspace_filter
            .as_ref()
            .is_some_and(|id| !state.workspaces.iter().any(|workspace| workspace.id == *id))
        {
            self.workspace_filter = None;
            *self.active_workspace_filter.borrow_mut() = None;
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
        let workspace_filter = self.active_workspace_filter.borrow().clone();
        let label_filter = self.active_label_filter.borrow().clone();
        if workspace_filter == self.workspace_filter && label_filter == self.label_filter {
            return false;
        }
        self.workspace_filter = workspace_filter;
        self.label_filter = label_filter;
        let state = self.context.store.borrow().state().clone();
        let entries = filtered_snoozed_task_entries(
            &state.tasks,
            &state.workspaces,
            self.workspace_filter.as_deref(),
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
                        self.workspace_filter.as_deref(),
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

    #[cfg(test)]
    pub(crate) fn transient_selected_task_ids(&self) -> Vec<String> {
        self.calendar().transient_selected_ids()
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
                        self.workspace_filter.as_deref(),
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

    pub(crate) fn highlighted_task_id(&self) -> Option<String> {
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
                (&state.tasks, &state.people, &state.workspaces, &state.tags),
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

    fn sync_after_event(
        &mut self,
        calendar_handled_event: bool,
        calendar_path: tuicore::TreePath,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let mut reordered = false;
        let calendar_events = self.calendar_mut().take_events();
        let focus_detail = calendar_handled_event
            && calendar_events
                .iter()
                .any(|event| matches!(event, CalendarTypedEvent::EntryActivated { .. }));
        for event in calendar_events {
            if let CalendarTypedEvent::EntriesReordered { entry_ids } = event {
                let state = self.context.store.borrow().state().clone();
                reordered |= persist_task_order(&self.context, &state, &entry_ids, None);
            }
        }
        if reordered {
            self.sync_store_version();
        }
        let patches_changed = self.drain_detail_patches();
        let detail_changed = if patches_changed {
            self.sync_store_version_with_ctx(ctx)
        } else if calendar_handled_event {
            self.sync_calendar_detail(ctx)
        } else {
            false
        };
        if detail_changed || patches_changed || reordered {
            ctx.request_layout();
            ctx.request_redraw();
        }
        if patches_changed && detail_changed {
            ctx.focus(FocusRequest::TargetAt {
                path: calendar_path,
                id: FocusId::new("calendar"),
            });
        }
        if focus_detail && self.pane.is_second_visible() {
            ctx.focus_next();
            ctx.request_redraw();
        }
    }

    fn handle_task_shortcut(
        &mut self,
        outcome: EventOutcome,
        event: &TuiEvent,
        return_focus: Option<tuicore::TreePath>,
        snooze_return_focus: Option<SnoozeReturnFocus>,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        if outcome.handled() {
            return outcome;
        }
        let Some((task_ids, time, has_selection)) = self.calendar_action_selection() else {
            return outcome;
        };
        let group_action = keys::TASK_QUICK_MENU.matches(event)
            || keys::TASK_SNOOZE.matches(event)
            || keys::TASK_DELETE_CTRL_X.matches(event)
            || keys::TASK_COMPLETE.matches(event)
            || keys::TASK_TOGGLE_PROGRESS.matches(event);
        let selection_invocation = if has_selection && group_action {
            self.context.begin_transient_selection(
                TransientSelectionSource::Calendar,
                task_ids.clone(),
                self.highlighted_task_id(),
            )
        } else {
            None
        };
        self.active_selection_invocation = selection_invocation;
        let message = if keys::TASK_QUICK_MENU.matches(event) {
            Some(AppMsg::OpenCalendarTasksQuickMenu {
                task_ids,
                time,
                selection_active: has_selection,
            })
        } else if keys::TASK_SNOOZE.matches(event) {
            has_selection
                .then_some(AppMsg::OpenCalendarTasksSnooze(task_ids.clone()))
                .or_else(|| {
                    Some(AppMsg::OpenTaskSnooze {
                        task_id: task_ids[0].clone(),
                        return_focus: snooze_return_focus,
                    })
                })
        } else if keys::TASK_DELETE_CTRL_X.matches(event) {
            has_selection
                .then_some(AppMsg::OpenCalendarDeleteTasks(task_ids.clone()))
                .or_else(|| {
                    Some(AppMsg::OpenCalendarDeleteTask {
                        task_id: task_ids[0].clone(),
                        return_focus,
                    })
                })
        } else if keys::TASK_COMPLETE.matches(event) {
            has_selection
                .then_some(AppMsg::OpenCalendarCompleteTasks(task_ids.clone()))
                .or_else(|| {
                    Some(AppMsg::OpenCalendarCompleteTask {
                        task_id: task_ids[0].clone(),
                        return_focus,
                    })
                })
        } else if keys::TASK_TOGGLE_PROGRESS.matches(event) {
            if has_selection {
                let state = self.context.store.borrow();
                let selected = state
                    .state()
                    .tasks
                    .iter()
                    .filter(|task| task_ids.contains(&task.id))
                    .collect::<Vec<_>>();
                let state = if selected.iter().all(|task| task.state == TaskState::Todo) {
                    TaskState::InProgress
                } else {
                    TaskState::Todo
                };
                Some(AppMsg::CompleteCalendarTasks { task_ids, state })
            } else {
                Some(AppMsg::ToggleCalendarTaskProgress(task_ids[0].clone()))
            }
        } else {
            None
        };
        if let Some(message) = message {
            ctx.emit(crate::app::selection_action(selection_invocation, message));
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        outcome
    }

    fn calendar_action_selection(&self) -> Option<(Vec<String>, Option<PrimitiveDateTime>, bool)> {
        let selected = self.calendar().transient_selected_ids();
        if selected.is_empty() {
            let task_id = self.highlighted_task_id()?;
            let time = self
                .context
                .store
                .borrow()
                .state()
                .tasks
                .iter()
                .find(|task| task.id == task_id)
                .and_then(|task| task.snoozed_until);
            Some((vec![task_id], time, false))
        } else {
            let state = self.context.store.borrow();
            let times = selected
                .iter()
                .filter_map(|task_id| {
                    state
                        .state()
                        .tasks
                        .iter()
                        .find(|task| task.id == *task_id)
                        .and_then(|task| task.snoozed_until)
                })
                .collect::<Vec<_>>();
            let time = times.first().copied().filter(|time| {
                times.len() == selected.len() && times.iter().all(|item| item == time)
            });
            Some((selected, time, true))
        }
    }

    fn handle_task_agent_yank(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> Option<EventOutcome> {
        let TuiEvent::Hotkey(tuicore::HotkeyEvent::Commit(sequence)) = event else {
            return None;
        };
        if sequence != &keys::TASK_AGENT_YANK.hotkey()
            && sequence != &keys::TASK_AGENT_YANK_CLARIFY.hotkey()
        {
            return None;
        }
        let (task_ids, _, selected_group) = self.calendar_action_selection()?;
        let state = self.context.store.borrow();
        let tasks = crate::app::tasks_for_ids(state.state(), &task_ids);
        let command = if sequence == &keys::TASK_AGENT_YANK.hotkey() {
            task_agent_commands_for(state.state(), &tasks, "execute")
        } else {
            task_agent_commands_for(state.state(), &tasks, "clarify")
        };
        drop(state);
        if let Some(command) = command {
            let selection_invocation = if selected_group {
                self.context.begin_transient_selection(
                    TransientSelectionSource::Calendar,
                    task_ids.clone(),
                    self.highlighted_task_id(),
                )
            } else {
                None
            };
            self.active_selection_invocation = selection_invocation;
            self.context
                .accept_transient_clipboard(selection_invocation);
            ctx.copy_to_clipboard(command);
            if selection_invocation
                .is_some_and(|invocation| self.context.take_selection_clear_request(invocation))
            {
                self.calendar_mut().clear_transient_selection();
            }
        }
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }

    fn handle_task_reference_yank(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> Option<EventOutcome> {
        if !matches!(event, TuiEvent::Yank) {
            return None;
        }
        let (task_ids, _, selected_group) = self.calendar_action_selection()?;
        let state = self.context.store.borrow();
        let tasks = crate::app::tasks_for_ids(state.state(), &task_ids);
        if !tasks.is_empty() {
            let selection_invocation = if selected_group {
                self.context.begin_transient_selection(
                    TransientSelectionSource::Calendar,
                    task_ids.clone(),
                    self.highlighted_task_id(),
                )
            } else {
                None
            };
            self.active_selection_invocation = selection_invocation;
            self.context
                .accept_transient_clipboard(selection_invocation);
            ctx.copy_to_clipboard(task_references_for(state.state(), &tasks));
            drop(state);
            if selection_invocation
                .is_some_and(|invocation| self.context.take_selection_clear_request(invocation))
            {
                self.calendar_mut().clear_transient_selection();
            }
        }
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

    fn focus_calendar(route: &EventRoute, ctx: &mut EventCtx<AppMsg>) {
        let workspace_path = Self::workspace_path(route, ctx);
        ctx.focus(FocusRequest::TargetAt {
            path: workspace_path.child(ChildKey::first()),
            id: FocusId::new("calendar"),
        });
        ctx.stop_propagation();
        ctx.request_redraw();
    }

    fn workspace_path(route: &EventRoute, ctx: &EventCtx<AppMsg>) -> tuicore::TreePath {
        ctx.current_path()
            .strip_suffix(&route.path)
            .unwrap_or_default()
    }

    fn calendar_snooze_return_focus(&self, calendar_path: tuicore::TreePath) -> SnoozeReturnFocus {
        let date = self.calendar().cursor_date();
        let selected = self.highlighted_task_id();
        let has_other_tasks = day_entry_ids(&self.visible_entries, date)
            .iter()
            .any(|task_id| Some(task_id.as_str()) != selected.as_deref());
        SnoozeReturnFocus::CalendarDay {
            path: calendar_path,
            date,
            has_other_tasks,
        }
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

fn filtered_snoozed_task_entries(
    tasks: &[Task],
    workspaces: &[Workspace],
    workspace_filter: Option<&str>,
    label_filter: &[String],
) -> Vec<SnoozedTaskEntry> {
    tasks
        .iter()
        .filter(|task| task_matches_filters(task, workspace_filter, label_filter))
        .filter_map(|task| snoozed_task_entry(task, workspaces))
        .collect()
}

fn snoozed_task_entry(task: &Task, workspaces: &[Workspace]) -> Option<SnoozedTaskEntry> {
    let workspace = task.workspace_id.as_deref().and_then(|workspace_id| {
        workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
    });
    (task.state == TaskState::Snoozed).then_some(SnoozedTaskEntry {
        id: task.id.clone(),
        title: task.title.clone(),
        display_id: crate::domain::task_display_id(task, workspace),
        priority: task.priority,
        size: task.size,
        until: task.snoozed_until?,
        rank: task.rank,
    })
}

fn task_matches_filters(
    task: &Task,
    workspace_filter: Option<&str>,
    label_filter: &[String],
) -> bool {
    workspace_filter.is_none_or(|workspace_id| task.workspace_id.as_deref() == Some(workspace_id))
        && label_filter
            .iter()
            .all(|tag_id| task.tag_ids.contains(tag_id))
}

fn task_calendar(entries: Vec<SnoozedTaskEntry>) -> TaskCalendar {
    Calendar::new(
        entries,
        |entry| entry.id.clone(),
        |entry| CalendarSpan::timed(entry.until, entry.until + Duration::minutes(1)),
        |entry| format!("{} {}", entry.display_id, entry.title),
    )
    .render_entry(|entry| {
        calendar_task_title_line(entry.priority, entry.size, &entry.display_id, &entry.title)
    })
    .day_entry_wrap_continuation_indent_by(|entry| {
        tuicore::line_width(&calendar_task_metadata_line(
            entry.priority,
            entry.size,
            &entry.display_id,
        ))
    })
    .wrap_day_entries()
    .compact_summary_title(100, |entry| entry.title.clone())
    .hotkey(keys::TASK_AGENT_YANK.hotkey())
    .bordered(false)
    .entry_order(compare_snoozed_task_entries)
    .reorderable(|left, right| left.until == right.until)
    .keybindings(CalendarKeyBindings::default().reorder([keys::TASK_MOVE_MODE.key_spec()]))
    .event_marker(|_| SNOOZE_ICON)
}

fn calendar_task_title_line(
    priority: TaskPriority,
    size: TaskSize,
    display_id: &str,
    title: &str,
) -> Line<'static> {
    let mut spans = calendar_task_metadata_line(priority, size, display_id).spans;
    spans.push(Span::styled(
        title.to_string(),
        Style::default().fg(tuicore::theme().text_fg()),
    ));
    Line::from(spans)
}

fn calendar_task_metadata_line(
    priority: TaskPriority,
    size: TaskSize,
    display_id: &str,
) -> Line<'static> {
    let mut spans = priority_icon_line(priority).spans;
    spans.push(Span::raw(" "));
    spans.extend(chip_line(size.label(), size.role()).spans);
    spans.push(Span::raw(" "));
    spans.extend(task_title_prefix_line(display_id).spans);
    Line::from(spans)
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

fn is_task_detail_hotkey_prefix(event: &TuiEvent) -> bool {
    let TuiEvent::Key(key) = event else {
        return false;
    };
    let Key::Char(prefix) = key.code else {
        return false;
    };
    key.modifiers.is_empty()
        && [keys::TASK_PRIORITY_FIELD, keys::TASK_PEOPLE_FIELD]
            .into_iter()
            .any(|binding| binding.hotkey().starts_with(prefix))
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
        self.detail_mut().set_layout_limits(
            calendar_area.width < crate::ui::responsive_split::MASTER_DETAIL_NARROW_BREAKPOINT,
            calendar_area.height,
        );
        ctx.with_focus_fallback_hotkey_sequences_status(
            FocusId::new("calendar"),
            calendar_area,
            vec![keys::TASK_AGENT_YANK_CLARIFY.hotkey()],
            |ctx| self.pane.layout(calendar_area, ctx),
        );
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
        if self.pane.is_second_visible() && is_task_detail_hotkey_prefix(event) {
            return EventOutcome::Ignored;
        }
        if let Some(outcome) = self.handle_month_escape(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_task_agent_yank(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_task_reference_yank(event, ctx) {
            return outcome;
        }
        let previous = self.calendar().is_showing_weekends();
        let outcome = self.calendar_mut().event(event, ctx);
        self.sync_selected_date();
        self.sync_empty_day_message();
        self.persist_weekend_visibility_change(previous);
        let calendar_path = ctx.current_path();
        self.sync_after_event(true, calendar_path.clone(), ctx);
        let snooze_return_focus = self.calendar_snooze_return_focus(calendar_path.clone());
        self.handle_task_shortcut(
            outcome,
            event,
            Some(calendar_path),
            Some(snooze_return_focus),
            ctx,
        )
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let detail_route = route.path.keys().first() == Some(&ChildKey::second());
        if self.pane.is_second_visible() && !detail_route && is_task_detail_hotkey_prefix(event) {
            return EventOutcome::Ignored;
        }
        if let Some(outcome) = self.handle_month_escape(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_task_agent_yank(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_task_reference_yank(event, ctx) {
            return outcome;
        }
        let previous = self.calendar().is_showing_weekends();
        let mut calendar_event = !detail_route;
        let mut outcome = if detail_route {
            self.pane.dispatch_event(route, event, ctx)
        } else {
            self.calendar_mut().event(event, ctx)
        };
        if detail_route
            && !outcome.handled()
            && ctx.propagation() == Propagation::Continue
            && is_calendar_view_hotkey(event)
        {
            calendar_event = true;
            outcome = self.calendar_mut().event(event, ctx);
        }
        self.sync_selected_date();
        self.sync_empty_day_message();
        self.persist_weekend_visibility_change(previous);
        let calendar_path = Self::workspace_path(route, ctx).child(ChildKey::first());
        self.sync_after_event(calendar_event, calendar_path.clone(), ctx);
        if detail_route && detail_escape(event) {
            Self::focus_calendar(route, ctx);
            return EventOutcome::Handled;
        }
        let return_focus = ctx.current_path();
        let snooze_return_focus = self.calendar_snooze_return_focus(calendar_path);
        self.handle_task_shortcut(
            outcome,
            event,
            Some(return_focus),
            Some(snooze_return_focus),
            ctx,
        )
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
    use crate::domain::{AppEvent, Tag, TaskPriority, TaskSize, Workspace, WorkspaceSnapshot};
    use ratatui::{Terminal, backend::TestBackend};
    use time::{Date, Month, Time};
    use tuicore::{
        AnimationSettings, FocusManager, FocusRequest, HotkeyEvent, Key, KeyEvent, KeyModifiers,
        Propagation, TreeDispatcher,
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
            workspace_id: None,
            tag_ids: Vec::new(),
            checklist: Vec::new(),
            links: Vec::new(),
            relations: Vec::new(),
            description: String::new(),
        }
    }

    #[test]
    fn calendar_day_rows_use_task_dataview_priority_and_title_styling() {
        let date = Date::from_calendar_date(2026, Month::July, 24).unwrap();
        let until = date.with_time(Time::from_hms(8, 0, 0).unwrap());
        let workspace = Workspace::new(
            "workspace".into(),
            "if".into(),
            "Internal fixes".into(),
            String::new(),
        );
        let mut snoozed = task("OLD-30", "Follow up", TaskState::Snoozed, Some(until));
        snoozed.workspace_id = Some(workspace.id.clone());
        snoozed.priority = TaskPriority::High;
        snoozed.size = TaskSize::Medium;
        let entries = filtered_snoozed_task_entries(
            &[
                snoozed,
                task("todo", "Still active", TaskState::Todo, Some(until)),
                task("undated", "Missing return date", TaskState::Snoozed, None),
            ],
            &[workspace],
            None,
            &[],
        );
        let mut calendar = task_calendar(entries.clone())
            .cursor(date)
            .view(CalendarView::Day);
        let area = Rect::new(0, 0, 100, 28);
        calendar.layout(area, &mut LayoutCtx::new());

        let text = rendered_text(&calendar, area);

        assert!(text.contains(SNOOZE_ICON));
        assert!(text.contains("󰅃 MED IF-30 Follow up"));
        assert!(!text.contains("Still active"));
        assert!(!text.contains("Missing return date"));

        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        terminal.draw(|frame| calendar.render(frame, area)).unwrap();
        let cells = terminal.backend().buffer().content();
        let id_start = cells
            .windows(5)
            .position(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == "IF-30")
            .expect("calendar task display ID should render");
        let title_start = cells
            .windows(9)
            .position(|cells| {
                cells.iter().map(|cell| cell.symbol()).collect::<String>() == "Follow up"
            })
            .expect("calendar task title should render");
        let time_start = cells
            .windows(5)
            .position(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == "08:00")
            .expect("calendar task time should render");
        let priority_start = cells
            .iter()
            .position(|cell| cell.symbol() == "󰅃")
            .expect("calendar task priority glyph should render");
        let size_start = cells
            .windows(3)
            .position(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == "MED")
            .expect("calendar task size chip should render");
        let snooze_marker = SNOOZE_ICON.to_string();
        let snooze_start = cells
            .iter()
            .position(|cell| cell.symbol() == snooze_marker)
            .expect("calendar task snooze marker should render");
        assert!(
            snooze_start < priority_start,
            "snooze marker should precede the task priority glyph"
        );
        assert!(
            priority_start < size_start && size_start < id_start,
            "size chip should follow priority before the task reference"
        );
        assert_eq!(
            cells[time_start].fg,
            tuicore::theme().accent_fg(),
            "calendar task time should use the Calendar accent foreground"
        );
        assert_eq!(
            cells[snooze_start].fg,
            tuicore::theme().text_fg(),
            "calendar task snooze marker should use the normal text foreground"
        );
        assert_eq!(
            cells[id_start].fg,
            tuicore::theme().subtle_fg(),
            "task display ID should use the DataView subtle foreground"
        );
        assert_eq!(
            cells[title_start].fg,
            tuicore::theme().text_fg(),
            "task title should use the semantic text foreground"
        );
        assert_eq!(
            cells[priority_start].fg,
            tuicore::theme().error_fg(),
            "high-priority task glyph should use the DataView semantic foreground"
        );
        assert_eq!(
            cells[size_start].fg,
            tuicore::theme().accent_fg(),
            "medium size chip should use the DataView semantic foreground"
        );
        assert!(
            cells[id_start]
                .modifier
                .contains(ratatui::style::Modifier::BOLD)
        );
        assert!(
            !cells[title_start]
                .modifier
                .contains(ratatui::style::Modifier::BOLD)
        );
        assert!(
            cells[size_start]
                .modifier
                .contains(ratatui::style::Modifier::BOLD)
        );

        for view in [CalendarView::Month, CalendarView::Week] {
            let normal = Rect::new(0, 0, 120, 28);
            let mut calendar = task_calendar(entries.clone()).cursor(date).view(view);
            calendar.layout(normal, &mut LayoutCtx::new());
            let text = rendered_text(&calendar, normal);
            assert!(text.contains("IF-30"), "missing task reference in {view:?}");

            let small = Rect::new(0, 0, 80, 28);
            let mut calendar = task_calendar(entries.clone()).cursor(date).view(view);
            calendar.layout(small, &mut LayoutCtx::new());
            let text = rendered_text(&calendar, small);
            assert!(text.contains("Follow"), "missing title in compact {view:?}");
            assert!(
                !text.contains("IF-30"),
                "task reference leaked into compact {view:?}"
            );
        }
    }

    #[test]
    fn calendar_day_wraps_task_titles_under_the_title_after_metadata() {
        let date = Date::from_calendar_date(2026, Month::July, 24).unwrap();
        let until = date.with_time(Time::from_hms(8, 0, 0).unwrap());
        let mut entry = task(
            "task",
            "Support prepares production validation",
            TaskState::Snoozed,
            Some(until),
        );
        entry.priority = TaskPriority::High;
        entry.size = TaskSize::Big;
        let mut calendar = task_calendar(vec![snoozed_task_entry(&entry, &[]).unwrap()])
            .cursor(date)
            .view(CalendarView::Day);
        let area = Rect::new(0, 0, 38, 12);
        calendar.layout(area, &mut LayoutCtx::new());
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        terminal.draw(|frame| calendar.render(frame, area)).unwrap();
        let buffer = terminal.backend().buffer();
        let word_position = |word: &str| {
            (0..area.height)
                .find_map(|y| {
                    let symbols = (0..area.width)
                        .map(|x| buffer.cell((x, y)).unwrap().symbol())
                        .collect::<Vec<_>>();
                    symbols
                        .windows(word.len())
                        .position(|symbols| symbols.concat() == word)
                        .map(|x| (x, y))
                })
                .expect("wrapped task word should render")
        };

        let support = word_position("Support");
        let production = word_position("production");

        assert!(
            support.1 < production.1,
            "title should wrap: {support:?} {production:?}"
        );
        assert_eq!(
            support.0, production.0,
            "continuation should align under the task title"
        );
    }

    #[test]
    fn calendar_applies_shared_workspace_and_label_filters() {
        let until = current_date().with_time(Time::from_hms(8, 0, 0).unwrap());
        let mut matching = task("matching", "Matching", TaskState::Snoozed, Some(until));
        matching.workspace_id = Some("workspace-2".into());
        matching.tag_ids = vec!["api".into(), "urgent".into()];
        let mut wrong_labels = task(
            "wrong-labels",
            "Wrong labels",
            TaskState::Snoozed,
            Some(until),
        );
        wrong_labels.workspace_id = Some("workspace-2".into());
        wrong_labels.tag_ids = vec!["api".into()];
        let mut wrong_workspace = task(
            "wrong-workspace",
            "Wrong workspace",
            TaskState::Snoozed,
            Some(until),
        );
        wrong_workspace.workspace_id = Some("workspace-1".into());
        wrong_workspace.tag_ids = vec!["api".into(), "urgent".into()];
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![matching, wrong_labels, wrong_workspace],
            people: Vec::new(),
            workspaces: vec![
                Workspace::new(
                    "workspace-1".into(),
                    "ONE".into(),
                    "One".into(),
                    String::new(),
                ),
                Workspace::new(
                    "workspace-2".into(),
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
        let workspace_filter = Rc::new(RefCell::new(Some("workspace-2".into())));
        let label_filter = Rc::new(RefCell::new(vec!["api".into(), "urgent".into()]));
        let mut workspace = CalendarWorkspace::new_with_create_context_and_filters(
            context,
            true,
            CalendarCreateContext::new(),
            Rc::clone(&workspace_filter),
            Rc::clone(&label_filter),
        );
        workspace.calendar_mut().on_key(Key::Char('D'));
        let area = Rect::new(0, 0, 80, 20);
        workspace.layout(area, &mut LayoutCtx::new());

        let filtered = rendered_text(&workspace, area);
        assert!(filtered.contains("Matching"));
        assert!(!filtered.contains("Wrong labels"));
        assert!(!filtered.contains("Wrong workspace"));

        *workspace_filter.borrow_mut() = None;
        label_filter.borrow_mut().clear();
        assert!(workspace.sync_filter_change());
        let unfiltered = rendered_text(&workspace, area);
        assert!(unfiltered.contains("Matching"));
        assert!(unfiltered.contains("Wrong labels"));
        assert!(unfiltered.contains("Wrong workspace"));
    }

    #[test]
    fn empty_day_shows_scheduled_task_empty_state_without_hiding_calendar() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
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
            workspaces: Vec::new(),
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
    fn calendar_persists_a_shift_selected_day_block_move() {
        let until = current_date().with_time(Time::from_hms(8, 0, 0).unwrap());
        let tasks = ["first", "second", "third", "fourth"]
            .into_iter()
            .enumerate()
            .map(|(index, id)| {
                let mut task = task(id, id, TaskState::Snoozed, Some(until));
                task.rank = index as i64 + 1;
                task
            })
            .collect();
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks,
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context, true);
        workspace.calendar_mut().on_key(Key::Char('D'));

        for key in [
            KeyEvent {
                code: Key::Char('j'),
                modifiers: KeyModifiers::SHIFT,
            },
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
        let ordered = state
            .state()
            .tasks
            .iter()
            .map(|task| (task.id.as_str(), task.rank))
            .collect::<Vec<_>>();
        assert_eq!(
            ordered,
            vec![("first", 2), ("second", 3), ("third", 1), ("fourth", 4)]
        );
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
            workspaces: Vec::new(),
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
            workspaces: Vec::new(),
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
            workspaces: Vec::new(),
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
            workspaces: Vec::new(),
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
            workspaces: Vec::new(),
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
        assert_eq!(wide_calendar, Rect::new(0, 0, 54, 30));
        assert_eq!(wide_detail, Rect::new(54, 0, 66, 30));

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
    fn day_view_narrow_long_description_uses_scrollable_seventy_five_percent_detail_pane() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context.clone(), true);
        let until = workspace.today.with_time(Time::from_hms(8, 0, 0).unwrap());
        let mut snoozed = task("snoozed", "Follow up", TaskState::Snoozed, Some(until));
        snoozed.description = (1..=40)
            .map(|line| format!("Line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(snoozed));
        workspace.sync_store_version();
        workspace.calendar_mut().on_key(Key::Char('D'));
        workspace.sync_calendar_detail(&mut EventCtx::default());

        let narrow = Rect::new(0, 0, 80, 45);
        let mut layout = LayoutCtx::new();
        workspace.layout(narrow, &mut layout);

        let description = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target.id.as_str() == "textarea"
                    && target
                        .path
                        .keys()
                        .iter()
                        .any(|key| key.as_str() == "description")
            })
            .expect("description should be focusable");

        let (calendar, detail) = workspace.pane.child_areas();
        assert_eq!(description.area.height, 8);
        assert!(detail.height <= narrow.height - calendar.height);
        assert_eq!(calendar.height, narrow.height * 25 / 100);
        assert!(detail.y >= calendar.bottom());
    }

    #[test]
    fn day_view_updates_detail_when_highlight_moves_between_tasks() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
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
    fn day_view_quick_menu_opens_a_group_menu_for_the_highlighted_task() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
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
        let mut layout = LayoutCtx::new();
        workspace.layout(Rect::new(0, 0, 120, 30), &mut layout);
        assert!(layout.focus_targets().iter().any(|target| {
            target
                .hotkey_sequences
                .contains(&keys::TASK_AGENT_YANK.hotkey())
        }));
        assert!(layout.focus_targets().iter().any(|target| {
            target
                .hotkey_sequences
                .contains(&keys::TASK_AGENT_YANK_CLARIFY.hotkey())
        }));
        let mut ctx = EventCtx::default();

        let outcome = workspace.event(&TuiEvent::Key(Key::Char('.').into()), &mut ctx);

        assert!(outcome.handled());
        assert!(matches!(
            ctx.messages(),
            [AppMsg::OpenCalendarTasksQuickMenu {
                task_ids,
                time,
                selection_active: false,
            }]
                if task_ids == &vec!["highlighted".to_string()] && *time == Some(until)
        ));
    }

    #[test]
    fn day_view_yank_copies_highlighted_task_reference() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context.clone(), true);
        let until = workspace.today.with_time(Time::from_hms(8, 0, 0).unwrap());
        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(task(
                "1234",
                "Calendar task",
                TaskState::Snoozed,
                Some(until),
            )));
        workspace.sync_store_version();
        workspace.calendar_mut().on_key(Key::Char('D'));
        workspace.sync_calendar_detail(&mut EventCtx::default());
        let mut ctx = EventCtx::default();

        let outcome = workspace.event(
            &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_AGENT_YANK.hotkey())),
            &mut ctx,
        );

        let effects = tuicore::DispatchEffects::from_event_ctx(outcome, ctx);
        assert_eq!(
            effects.clipboard.as_deref(),
            Some("Tuido execute 1234 \"Calendar task\"")
        );

        let mut clarify_ctx = EventCtx::default();
        let outcome = workspace.event(
            &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_AGENT_YANK_CLARIFY.hotkey())),
            &mut clarify_ctx,
        );
        let effects = tuicore::DispatchEffects::from_event_ctx(outcome, clarify_ctx);

        assert_eq!(
            effects.clipboard.as_deref(),
            Some("Tuido clarify 1234 \"Calendar task\"")
        );

        let mut yank_ctx = EventCtx::default();
        let outcome = workspace.event(&TuiEvent::Yank, &mut yank_ctx);
        let effects = tuicore::DispatchEffects::from_event_ctx(outcome, yank_ctx);

        assert_eq!(
            effects.clipboard.as_deref(),
            Some("Tuido 1234 \"Calendar task\"")
        );
    }

    #[test]
    fn day_view_group_yanks_copy_selected_tasks_then_clear_selection() {
        let today = current_date();
        let until = today.with_time(Time::from_hms(8, 0, 0).unwrap());
        let mut first = task("1", "First \"quoted\"", TaskState::Snoozed, Some(until));
        first.rank = 1;
        let mut second = task("2", "Second \\ path", TaskState::Snoozed, Some(until));
        second.rank = 2;
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![first, second],
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context, true);
        workspace.calendar_mut().on_key(Key::Char('D'));

        for (event, expected) in [
            (
                TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_AGENT_YANK.hotkey())),
                r#"Tuido execute 1 "First \"quoted\""; 2 "Second \\ path""#,
            ),
            (
                TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_AGENT_YANK_CLARIFY.hotkey())),
                r#"Tuido clarify 1 "First \"quoted\""; 2 "Second \\ path""#,
            ),
            (
                TuiEvent::Yank,
                r#"Tuido 1 "First \"quoted\""; 2 "Second \\ path""#,
            ),
        ] {
            workspace
                .calendar_mut()
                .highlight_entry_id(&"1".to_string());
            workspace.event(
                &TuiEvent::Key(KeyEvent {
                    code: Key::Down,
                    modifiers: KeyModifiers::SHIFT,
                }),
                &mut EventCtx::default(),
            );
            let mut ctx = EventCtx::default();

            workspace.event(&event, &mut ctx);

            assert_eq!(ctx.clipboard_request(), Some(expected));
            assert!(workspace.transient_selected_task_ids().is_empty());
        }
    }

    #[test]
    fn calendar_detail_clarify_yank_copies_highlighted_task_command() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context.clone(), true);
        let until = workspace.today.with_time(Time::from_hms(8, 0, 0).unwrap());
        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(task(
                "1234",
                "Calendar task",
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
            .find(|target| target.path.keys().first() == Some(&ChildKey::second()))
            .expect("detail control should be focusable")
            .path
            .clone();

        let mut ctx = EventCtx::default();
        let outcome = workspace.dispatch_event(
            &EventRoute::new(detail_path.clone()),
            &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_AGENT_YANK_CLARIFY.hotkey())),
            &mut ctx,
        );
        let effects = tuicore::DispatchEffects::from_event_ctx(outcome, ctx);

        assert_eq!(
            effects.clipboard.as_deref(),
            Some("Tuido clarify 1234 \"Calendar task\"")
        );

        let mut yank_ctx = EventCtx::default();
        let outcome = workspace.dispatch_event(
            &EventRoute::new(detail_path),
            &TuiEvent::Yank,
            &mut yank_ctx,
        );
        let effects = tuicore::DispatchEffects::from_event_ctx(outcome, yank_ctx);

        assert_eq!(
            effects.clipboard.as_deref(),
            Some("Tuido 1234 \"Calendar task\"")
        );
    }

    #[test]
    fn day_view_move_mode_reorders_only_tasks_at_the_same_time() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
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

    #[test]
    fn day_view_move_mode_with_one_task_is_silent() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let until = workspace_time(8);
        context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(task(
                "only",
                "Only task",
                TaskState::Snoozed,
                Some(until),
            )));
        let mut workspace = CalendarWorkspace::new(context, true);
        workspace.calendar_mut().on_key(Key::Char('D'));
        let mut ctx = EventCtx::default();

        assert!(
            workspace
                .event(
                    &TuiEvent::Key(KeyEvent {
                        code: Key::Char('m'),
                        modifiers: KeyModifiers::CONTROL,
                    }),
                    &mut ctx,
                )
                .handled()
        );

        assert!(!workspace.calendar().is_reordering());
        assert!(ctx.notifications().is_empty());
    }

    fn workspace_time(hour: u8) -> PrimitiveDateTime {
        current_date().with_time(Time::from_hms(hour, 0, 0).unwrap())
    }

    #[test]
    fn activating_day_view_task_moves_focus_into_shared_detail() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
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
    fn calendar_detail_dropdown_hotkeys_follow_the_calendar_detail_lifecycle() {
        let area = Rect::new(0, 0, 80, 30);
        enum Expected {
            State(TaskState),
            Priority(TaskPriority),
            Size(TaskSize),
            Workspace(&'static str),
            People(&'static [&'static str]),
            Tags(&'static [&'static str]),
            Unchanged,
        }
        let cases = [
            (
                "state",
                keys::TASK_STATE_FIELD.hotkey(),
                'b',
                Expected::State(TaskState::Backlog),
            ),
            (
                "priority",
                keys::TASK_PRIORITY_FIELD.hotkey(),
                'h',
                Expected::Priority(TaskPriority::High),
            ),
            (
                "size",
                keys::TASK_SIZE_FIELD.hotkey(),
                'e',
                Expected::Size(TaskSize::Medium),
            ),
            (
                "workspaces",
                keys::TASK_WORKSPACES_FIELD.hotkey(),
                'a',
                Expected::Workspace("workspace"),
            ),
            (
                "people",
                keys::TASK_PEOPLE_FIELD.hotkey(),
                'a',
                Expected::People(&["person"]),
            ),
            (
                "tags",
                keys::TASK_TAGS_FIELD.hotkey(),
                'a',
                Expected::Tags(&["tag"]),
            ),
            (
                "snoozed-until",
                keys::TASK_SNOOZED_UNTIL_FIELD.hotkey(),
                ' ',
                Expected::Unchanged,
            ),
        ];

        for (field, hotkey, selection, expected) in cases {
            let until = current_date().with_time(Time::from_hms(8, 0, 0).unwrap());
            let (_runtime, context, store) = test_context(WorkspaceSnapshot {
                tasks: vec![task(
                    "snoozed",
                    "Follow up",
                    TaskState::Snoozed,
                    Some(until),
                )],
                people: vec![crate::domain::Person::new(
                    "person".into(),
                    "Ada".into(),
                    "ada@example.com".into(),
                )],
                workspaces: vec![Workspace::new(
                    "workspace".into(),
                    "APP".into(),
                    "Application".into(),
                    String::new(),
                )],
                tags: vec![Tag::new("tag".into(), "API".into())],
            });
            let mut workspace = CalendarWorkspace::new(context, true);
            workspace.calendar_mut().on_key(Key::Char('D'));
            workspace.sync_calendar_detail(&mut EventCtx::default());
            let mut layout = LayoutCtx::new();
            workspace.layout(area, &mut layout);
            let focus_id = match field {
                "tags" => "tag-input",
                "snoozed-until" => "date-time-picker-dropdown",
                _ => "field",
            };
            let dropdown = layout
                .focus_targets()
                .iter()
                .find(|target| {
                    target.id.as_str() == focus_id
                        && target.path.keys().iter().any(|key| key.as_str() == field)
                })
                .expect("calendar detail field should be focusable")
                .clone();
            let mut dispatcher = TreeDispatcher::new();

            if field == "priority" {
                let calendar = layout
                    .focus_targets()
                    .iter()
                    .find(|target| target.id.as_str() == "calendar")
                    .expect("Calendar Day should be focusable");
                assert_eq!(
                    workspace.dispatch_event(
                        &EventRoute::new(calendar.path.clone()),
                        &TuiEvent::Key(Key::Char('p').into()),
                        &mut EventCtx::default()
                    ),
                    EventOutcome::Ignored,
                    "Calendar Day must leave the priority hotkey prefix for the global matcher"
                );
            }

            let open = dispatcher.dispatch_event(
                &mut workspace,
                &EventRoute::new(dropdown.path.clone()),
                &TuiEvent::Hotkey(HotkeyEvent::Commit(hotkey.clone())),
                AnimationSettings::default(),
            );
            assert!(open.layout, "{field} hotkey should request a popup layout");
            let mut open_layout = LayoutCtx::new();
            workspace.layout(area, &mut open_layout);
            assert!(
                (field == "tags") || !open_layout.overlays().is_empty(),
                "{field} should register its popup overlay"
            );
            assert!(
                !rendered_text(&workspace, area).is_empty(),
                "{field} popup should render without terminating the workspace"
            );
            if matches!(expected, Expected::Unchanged) {
                let close = dispatcher.dispatch_event(
                    &mut workspace,
                    &EventRoute::new(dropdown.path),
                    &TuiEvent::Key(Key::Esc.into()),
                    AnimationSettings::default(),
                );
                assert!(close.layout, "{field} escape should close its popup");
                let mut closed_layout = LayoutCtx::new();
                workspace.layout(area, &mut closed_layout);
                assert!(closed_layout.overlays().is_empty());
                continue;
            }
            let focus_request = open
                .focus_request
                .as_ref()
                .expect("field hotkey should request input focus");
            let mut focus = FocusManager::new();
            let transition = focus
                .apply_request(focus_request, open_layout.focus_targets())
                .expect("popup input focus should apply");
            dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
            let popup_focus_id = match field {
                "tags" => "tag-input",
                "snoozed-until" => "date-time-picker-dropdown",
                _ => "input",
            };
            assert_eq!(
                focus.current().map(|target| target.id.as_str()),
                Some(popup_focus_id)
            );

            let close = dispatcher.dispatch_event(
                &mut workspace,
                &EventRoute::new(focus.current_path()),
                &TuiEvent::Key(Key::Esc.into()),
                AnimationSettings::default(),
            );
            assert!(close.layout, "{field} escape should close its popup");
            let mut closed_layout = LayoutCtx::new();
            workspace.layout(area, &mut closed_layout);
            assert!(
                (field == "tags") || closed_layout.overlays().is_empty(),
                "{field} close should remove its popup overlay"
            );
            if let Some(transition) = focus.validate(closed_layout.focus_targets()) {
                dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
            }
            let reopen = dispatcher.dispatch_event(
                &mut workspace,
                &EventRoute::new(dropdown.path),
                &TuiEvent::Hotkey(HotkeyEvent::Commit(hotkey)),
                AnimationSettings::default(),
            );
            let mut reopen_layout = LayoutCtx::new();
            workspace.layout(area, &mut reopen_layout);
            if let Some(transition) = focus.apply_request(
                reopen
                    .focus_request
                    .as_ref()
                    .expect("field reopen should request input focus"),
                reopen_layout.focus_targets(),
            ) {
                dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
            }

            dispatcher.dispatch_event(
                &mut workspace,
                &EventRoute::new(focus.current_path()),
                &TuiEvent::Key(Key::Char(selection).into()),
                AnimationSettings::default(),
            );
            if field == "people" {
                dispatcher.dispatch_event(
                    &mut workspace,
                    &EventRoute::new(focus.current_path()),
                    &TuiEvent::Key(Key::Enter.into()),
                    AnimationSettings::default(),
                );
            }
            let commit_key = if field == "people" {
                KeyEvent {
                    code: Key::Enter,
                    modifiers: KeyModifiers::CONTROL,
                }
            } else {
                Key::Enter.into()
            };
            let commit = dispatcher.dispatch_event(
                &mut workspace,
                &EventRoute::new(focus.current_path()),
                &TuiEvent::Key(commit_key),
                AnimationSettings::default(),
            );
            {
                let state = store.borrow();
                let task = &state.state().tasks[0];
                match expected {
                    Expected::State(state) => assert_eq!(task.state, state),
                    Expected::Priority(priority) => assert_eq!(task.priority, priority),
                    Expected::Size(size) => assert_eq!(task.size, size),
                    Expected::Workspace(id) => assert_eq!(task.workspace_id.as_deref(), Some(id)),
                    Expected::People(ids) => assert_eq!(
                        task.people_ids
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>(),
                        ids
                    ),
                    Expected::Tags(ids) => assert_eq!(
                        task.tag_ids.iter().map(String::as_str).collect::<Vec<_>>(),
                        ids
                    ),
                    Expected::Unchanged => {}
                }
            }
            let mut committed_layout = LayoutCtx::new();
            workspace.layout(area, &mut committed_layout);
            assert!(!committed_layout.focus_targets().iter().any(|target| {
                target.id.as_str() == "input"
                    && target.path.keys().iter().any(|key| key.as_str() == field)
            }));

            if field == "state" {
                assert!(matches!(
                    commit.focus_request,
                    Some(FocusRequest::TargetAt { id, .. }) if id.as_str() == "calendar"
                ));
                assert!(!workspace.pane.is_second_visible());
            }
        }
    }

    #[test]
    fn calendar_task_detail_keeps_task_action_hotkeys() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
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
            [AppMsg::OpenCalendarDeleteTask { task_id, return_focus: Some(_) }]
                if task_id == "snoozed"
        ));

        let mut complete_ctx = EventCtx::default();
        let complete = workspace.dispatch_event(&route, &shortcut('c'), &mut complete_ctx);
        assert!(complete.handled());
        assert!(matches!(
            complete_ctx.messages(),
            [AppMsg::OpenCalendarCompleteTask { task_id, return_focus: Some(_) }]
                if task_id == "snoozed"
        ));

        let mut progress_ctx = EventCtx::default();
        let progress = workspace.dispatch_event(&route, &shortcut('t'), &mut progress_ctx);
        assert!(progress.handled());
        assert!(matches!(
            progress_ctx.messages(),
            [AppMsg::ToggleCalendarTaskProgress(task_id)] if task_id == "snoozed"
        ));
    }

    #[test]
    fn calendar_task_dialog_shortcuts_preserve_calendar_focus_path() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
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
            match (key, effects.messages.as_slice()) {
                (
                    'z',
                    [
                        AppMsg::OpenTaskSnooze {
                            return_focus:
                                Some(SnoozeReturnFocus::CalendarDay {
                                    path,
                                    date,
                                    has_other_tasks,
                                }),
                            ..
                        },
                    ],
                ) => {
                    assert_eq!(path, &calendar_path);
                    assert_eq!(*date, workspace.today);
                    assert!(!has_other_tasks);
                }
                ('x', [AppMsg::OpenCalendarDeleteTask { return_focus, .. }])
                | ('c', [AppMsg::OpenCalendarCompleteTask { return_focus, .. }]) => {
                    assert_eq!(return_focus.as_ref(), Some(&calendar_path));
                }
                _ => panic!("calendar shortcut should open its task dialog"),
            }
        }
    }

    #[test]
    fn calendar_snooze_shortcut_targets_sparse_selection() {
        let until = workspace_time(8);
        let tasks = ["first", "second", "third"]
            .into_iter()
            .enumerate()
            .map(|(index, id)| {
                let mut task = task(id, id, TaskState::Snoozed, Some(until));
                task.rank = index as i64 + 1;
                task
            })
            .collect();
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks,
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = CalendarWorkspace::new(context, true);
        workspace.calendar_mut().on_key(Key::Char('D'));
        for key in [
            KeyEvent {
                code: Key::Down,
                modifiers: KeyModifiers::CONTROL,
            },
            KeyEvent {
                code: Key::Down,
                modifiers: KeyModifiers::CONTROL,
            },
            KeyEvent {
                code: Key::Char(' '),
                modifiers: KeyModifiers::CONTROL,
            },
        ] {
            workspace.event(&TuiEvent::Key(key), &mut EventCtx::default());
        }
        let mut ctx = EventCtx::default();

        workspace.event(
            &TuiEvent::Key(KeyEvent {
                code: Key::Char('z'),
                modifiers: KeyModifiers::CONTROL,
            }),
            &mut ctx,
        );

        assert!(matches!(
            ctx.messages(),
            [AppMsg::SelectionAction { action, .. }]
                if matches!(action.as_ref(), AppMsg::OpenCalendarTasksSnooze(task_ids)
                    if task_ids == &vec!["first".to_string(), "third".to_string()])
        ));
    }

    #[test]
    fn calendar_view_hotkeys_work_while_task_detail_is_focused() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
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
    fn calendar_view_hotkeys_are_typed_while_task_detail_is_editing() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
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
        let area = Rect::new(0, 0, 120, 30);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let title = layout
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
            .clone();
        let mut focus = FocusManager::new();
        let transition = focus
            .apply_request(
                &FocusRequest::TargetAt {
                    path: title.path.clone(),
                    id: title.id.clone(),
                },
                layout.focus_targets(),
            )
            .expect("task title focus should apply");
        let mut dispatcher = TreeDispatcher::new();
        dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
        let route = EventRoute::new(focus.current_path());
        dispatcher.dispatch_event(
            &mut workspace,
            &route,
            &TuiEvent::Key(Key::Enter.into()),
            AnimationSettings::default(),
        );

        for key in ['W', 'D', 'M'] {
            let effects = dispatcher.dispatch_event(
                &mut workspace,
                &route,
                &TuiEvent::Key(Key::Char(key).into()),
                AnimationSettings::default(),
            );
            assert_eq!(effects.outcome, EventOutcome::Handled);
            assert_eq!(workspace.calendar().current_view(), CalendarView::Day);
        }
        dispatcher.dispatch_event(
            &mut workspace,
            &route,
            &TuiEvent::Key(Key::Enter.into()),
            AnimationSettings::default(),
        );

        assert_eq!(store.borrow().state().tasks[0].title, "Follow upWDM");
    }

    #[test]
    fn closing_calendar_task_detail_focuses_calendar_directly() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
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
            workspaces: Vec::new(),
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
