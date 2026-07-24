use std::time::Duration as StdDuration;

use std::{cell::RefCell, rc::Rc};

use ratatui::{Frame, layout::Rect};
use time::{
    Date, Duration, OffsetDateTime, PrimitiveDateTime, Time, Weekday, macros::format_description,
};
use tuicore::{
    AnimationSettings, DateTimePicker, DateTimePickerLayout, Dropdown, DropdownCommitMode,
    DropdownLabelPosition, DropdownSearchMode, DropdownVariant, EventCtx, EventOutcome, EventRoute,
    FocusCtx, FocusId, FocusRequest, FocusTarget, LayoutCtx, LayoutProposal, LayoutResult,
    LayoutSizeHint, RenderCtx, TickResult, TreePath, TuiEvent, TuiNode, keybindings,
};

use crate::app::AppMsg;

const STORAGE_FORMAT: &[time::format_description::FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");
const MENU_HOST_WIDTH: u16 = 46;
const MENU_HOST_HEIGHT: u16 = 12;
const MENU_FIELD_WIDTH: u16 = 36;

pub(crate) fn format_datetime(value: PrimitiveDateTime) -> String {
    value.format(STORAGE_FORMAT).expect("fixed datetime format")
}

pub(crate) fn parse_datetime(value: &str) -> Result<PrimitiveDateTime, time::error::Parse> {
    PrimitiveDateTime::parse(value, STORAGE_FORMAT)
}

pub(crate) fn local_now() -> Result<PrimitiveDateTime, time::error::IndeterminateOffset> {
    OffsetDateTime::now_local().map(|now| PrimitiveDateTime::new(now.date(), now.time()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QuickSnoozes {
    pub tomorrow: PrimitiveDateTime,
    pub weekend: PrimitiveDateTime,
    pub next_week: PrimitiveDateTime,
}

pub(crate) fn quick_snoozes(now: PrimitiveDateTime) -> QuickSnoozes {
    QuickSnoozes {
        tomorrow: at_eight(now.date() + Duration::days(1)),
        weekend: next_weekday_at_eight(now.date(), Weekday::Saturday),
        next_week: next_weekday_at_eight(now.date(), Weekday::Monday),
    }
}

fn next_weekday_at_eight(date: Date, target: Weekday) -> PrimitiveDateTime {
    let current = date.weekday().number_days_from_monday() as i64;
    let target = target.number_days_from_monday() as i64;
    let mut days = (target - current).rem_euclid(7);
    if days == 0 {
        days = 7;
    }
    at_eight(date + Duration::days(days))
}

fn at_eight(date: Date) -> PrimitiveDateTime {
    date.with_time(Time::from_hms(8, 0, 0).expect("08:00 is valid"))
}

fn short_weekday(value: Weekday) -> &'static str {
    match value {
        Weekday::Monday => "Mon",
        Weekday::Tuesday => "Tue",
        Weekday::Wednesday => "Wed",
        Weekday::Thursday => "Thu",
        Weekday::Friday => "Fri",
        Weekday::Saturday => "Sat",
        Weekday::Sunday => "Sun",
    }
}

fn format_time(value: PrimitiveDateTime) -> String {
    let hour = value.hour();
    let suffix = if hour < 12 { "AM" } else { "PM" };
    let display_hour = match hour % 12 {
        0 => 12,
        hour => hour,
    };
    format!("{display_hour}:{:02} {suffix}", value.minute())
}

fn quick_label(value: PrimitiveDateTime) -> String {
    format!(
        "{} · {}",
        short_weekday(value.weekday()),
        format_time(value)
    )
}

fn custom_label(value: PrimitiveDateTime) -> String {
    format!(
        "{} {}/{} · {}",
        short_weekday(value.weekday()),
        value.month() as u8,
        value.day(),
        format_time(value)
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SnoozeChoice {
    Tomorrow,
    Weekend,
    NextWeek,
    Last,
    Pick,
    Unsnooze,
}

enum SnoozeMode {
    Menu,
    Picker,
}

pub(crate) struct SnoozeDialog {
    task_id: String,
    quick: QuickSnoozes,
    last_custom: Option<PrimitiveDateTime>,
    is_snoozed: bool,
    dropdown: Dropdown<SnoozeOption, SnoozeChoice>,
    actions: Rc<RefCell<Vec<SnoozeChoice>>>,
    mode: SnoozeMode,
    picker: DateTimePicker<AppMsg>,
    focus_path: TreePath,
    picker_on_time: bool,
    menu_field_area: Rect,
}

struct SnoozeOption {
    choice: SnoozeChoice,
    label: String,
}

fn snooze_options(
    quick: QuickSnoozes,
    last_custom: Option<PrimitiveDateTime>,
    is_snoozed: bool,
) -> Vec<SnoozeOption> {
    let mut options = vec![
        SnoozeOption {
            choice: SnoozeChoice::Tomorrow,
            label: format!("Tomorrow · {}", quick_label(quick.tomorrow)),
        },
        SnoozeOption {
            choice: SnoozeChoice::Weekend,
            label: format!("This weekend · {}", quick_label(quick.weekend)),
        },
        SnoozeOption {
            choice: SnoozeChoice::NextWeek,
            label: format!("Next week · {}", quick_label(quick.next_week)),
        },
    ];
    if let Some(last_custom) = last_custom {
        options.push(SnoozeOption {
            choice: SnoozeChoice::Last,
            label: format!("Last · {}", custom_label(last_custom)),
        });
    }
    options.push(SnoozeOption {
        choice: SnoozeChoice::Pick,
        label: "◷ Pick date & time".into(),
    });
    if is_snoozed {
        options.push(SnoozeOption {
            choice: SnoozeChoice::Unsnooze,
            label: "Unsnooze".into(),
        });
    }
    options
}

fn build_snooze_dropdown(
    quick: QuickSnoozes,
    last_custom: Option<PrimitiveDateTime>,
    is_snoozed: bool,
    actions: Rc<RefCell<Vec<SnoozeChoice>>>,
) -> Dropdown<SnoozeOption, SnoozeChoice> {
    Dropdown::single(
        snooze_options(quick, last_custom, is_snoozed),
        |row| row.choice,
        |row| row.label.clone(),
    )
    .variant(DropdownVariant::Filled)
    .label("Snooze until...")
    .label_position(DropdownLabelPosition::Inline)
    .search_mode(DropdownSearchMode::Fuzzy)
    .commit_mode(DropdownCommitMode::Explicit)
    .centered(true)
    .tab_stop(false)
    .max_popup_height(12)
    .on_select(move |ids| {
        if let Some(choice) = ids.first() {
            actions.borrow_mut().push(*choice);
        }
    })
}

impl SnoozeDialog {
    pub(crate) fn new(
        task_id: String,
        now: PrimitiveDateTime,
        last_custom: Option<PrimitiveDateTime>,
        is_snoozed: bool,
    ) -> Self {
        let quick = quick_snoozes(now);
        let initial = last_custom
            .filter(|value| *value > now)
            .unwrap_or(quick.tomorrow);
        let picker_task_id = task_id.clone();
        let actions = Rc::new(RefCell::new(Vec::new()));
        let mut dropdown =
            build_snooze_dropdown(quick, last_custom, is_snoozed, Rc::clone(&actions));
        dropdown.open();
        Self {
            task_id,
            quick,
            last_custom,
            is_snoozed,
            dropdown,
            actions,
            mode: SnoozeMode::Menu,
            focus_path: TreePath::default(),
            picker_on_time: false,
            menu_field_area: Rect::default(),
            picker: DateTimePicker::new()
                .layout(DateTimePickerLayout::Stepped)
                .value(Some(initial))
                .on_select(move |until| AppMsg::SnoozeTask {
                    task_id: picker_task_id.clone(),
                    until,
                    remember_custom: Some(until),
                }),
        }
    }

    fn activate(&mut self, choice: SnoozeChoice, ctx: &mut EventCtx<AppMsg>) {
        let until = match choice {
            SnoozeChoice::Tomorrow => Some(self.quick.tomorrow),
            SnoozeChoice::Weekend => Some(self.quick.weekend),
            SnoozeChoice::NextWeek => Some(self.quick.next_week),
            SnoozeChoice::Last => self.last_custom,
            SnoozeChoice::Pick => {
                self.mode = SnoozeMode::Picker;
                self.picker_on_time = false;
                ctx.request_layout();
                ctx.request_redraw();
                ctx.focus(FocusRequest::TargetAt {
                    path: self.focus_path.clone(),
                    id: FocusId::new("date-time-picker"),
                });
                None
            }
            SnoozeChoice::Unsnooze => {
                ctx.emit(AppMsg::UnsnoozeTask(self.task_id.clone()));
                None
            }
        };
        if let Some(until) = until {
            ctx.emit(AppMsg::SnoozeTask {
                task_id: self.task_id.clone(),
                until,
                remember_custom: None,
            });
        }
    }

    fn drain_actions(&mut self, ctx: &mut EventCtx<AppMsg>) -> bool {
        let actions = self.actions.borrow_mut().drain(..).collect::<Vec<_>>();
        let handled = !actions.is_empty();
        for action in actions {
            self.activate(action, ctx);
        }
        handled
    }

    fn centered_menu_field_area(&self, area: Rect) -> Rect {
        let width = MENU_FIELD_WIDTH.min(area.width);
        let field_hint = <Dropdown<SnoozeOption, SnoozeChoice> as TuiNode<AppMsg>>::measure(
            &self.dropdown,
            LayoutProposal::at_most(width, area.height),
        );
        let height = field_hint.preferred.height.min(area.height);
        Rect::new(
            area.x.saturating_add(area.width.saturating_sub(width) / 2),
            area.y
                .saturating_add(area.height.saturating_sub(height) / 2),
            width,
            height,
        )
    }
}

impl TuiNode<AppMsg> for SnoozeDialog {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        match self.mode {
            SnoozeMode::Menu => LayoutSizeHint::content(MENU_HOST_WIDTH, MENU_HOST_HEIGHT),
            SnoozeMode::Picker => self.picker.measure(proposal),
        }
        .normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.focus_path = ctx.current_path();
        match self.mode {
            SnoozeMode::Menu => {
                self.menu_field_area = self.centered_menu_field_area(area);
                <Dropdown<SnoozeOption, SnoozeChoice> as TuiNode<AppMsg>>::layout(
                    &mut self.dropdown,
                    self.menu_field_area,
                    ctx,
                );
            }
            SnoozeMode::Picker => {
                <DateTimePicker<AppMsg> as TuiNode<AppMsg>>::layout(&mut self.picker, area, ctx);
            }
        }
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        match self.mode {
            SnoozeMode::Menu => self.dropdown.render(frame, self.menu_field_area, ctx),
            SnoozeMode::Picker => {
                <DateTimePicker<AppMsg> as TuiNode<AppMsg>>::render(&self.picker, frame, area, ctx)
            }
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        let TuiEvent::Key(key) = event else {
            let outcome = self.dropdown.event(event, ctx);
            self.drain_actions(ctx);
            return outcome;
        };
        if keybindings().focus().unfocus_matches(*key) {
            match self.mode {
                SnoozeMode::Menu => ctx.emit(AppMsg::CloseDialog),
                SnoozeMode::Picker if !self.picker_on_time => {
                    self.mode = SnoozeMode::Menu;
                    ctx.request_layout();
                    ctx.request_redraw();
                    self.dropdown = build_snooze_dropdown(
                        self.quick,
                        self.last_custom,
                        self.is_snoozed,
                        Rc::clone(&self.actions),
                    );
                    self.dropdown.open_with_context(ctx);
                    ctx.focus(FocusRequest::TargetAt {
                        path: self.focus_path.clone(),
                        id: FocusId::new("input"),
                    });
                }
                SnoozeMode::Picker => {
                    let outcome = self.picker.event(event, ctx);
                    self.picker_on_time = false;
                    return outcome;
                }
            }
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        if matches!(self.mode, SnoozeMode::Picker) {
            let message_count = ctx.messages().len();
            let outcome = self.picker.event(event, ctx);
            if keybindings().button().press_matches(*key) && outcome.handled() {
                self.picker_on_time = ctx.messages().len() == message_count;
            }
            return outcome;
        }
        let was_open = self.dropdown.is_open();
        let outcome = self.dropdown.event(event, ctx);
        let activated = self.drain_actions(ctx);
        if was_open && !self.dropdown.is_open() && !activated {
            ctx.emit(AppMsg::CloseDialog);
        }
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        if matches!(self.mode, SnoozeMode::Picker) && !matches!(event, TuiEvent::Key(_)) {
            let message_count = ctx.messages().len();
            let was_on_time = self.picker_on_time;
            let outcome = self.picker.dispatch_event(route, event, ctx);
            if let TuiEvent::ExternalEditor(response) = event {
                if ctx.messages().len() > message_count {
                    self.picker_on_time = false;
                } else if !was_on_time && editor_date_is_valid(&response.value) {
                    self.picker_on_time = true;
                }
            }
            outcome
        } else if matches!(self.mode, SnoozeMode::Menu) {
            let was_open = self.dropdown.is_open();
            let outcome = self.dropdown.dispatch_event(route, event, ctx);
            let activated = self.drain_actions(ctx);
            if was_open && !self.dropdown.is_open() && !activated {
                ctx.emit(AppMsg::CloseDialog);
            }
            outcome
        } else {
            self.event(event, ctx)
        }
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        match self.mode {
            SnoozeMode::Menu => self.dropdown.dispatch_focus(target, focused, ctx),
            SnoozeMode::Picker => self.picker.dispatch_focus(target, focused, ctx),
        }
    }

    fn tick(&mut self, dt: StdDuration, settings: AnimationSettings) -> TickResult {
        match self.mode {
            SnoozeMode::Menu => <Dropdown<SnoozeOption, SnoozeChoice> as TuiNode<AppMsg>>::tick(
                &mut self.dropdown,
                dt,
                settings,
            ),
            SnoozeMode::Picker => self.picker.tick(dt, settings),
        }
    }
}

fn editor_date_is_valid(value: &str) -> bool {
    value.trim().lines().next().is_some_and(|date| {
        Date::parse(
            date.trim(),
            &time::format_description::well_known::Iso8601::DATE,
        )
        .is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Month, macros::datetime};
    use tuicore::{FocusManager, Key, KeyEvent, KeyModifiers, TreeDispatcher};

    #[test]
    fn quick_options_are_strictly_future_across_weekday_boundaries() {
        let saturday = quick_snoozes(datetime!(2026-07-25 12:00));
        assert_eq!(saturday.tomorrow, datetime!(2026-07-26 8:00));
        assert_eq!(saturday.weekend, datetime!(2026-08-01 8:00));
        assert_eq!(saturday.next_week, datetime!(2026-07-27 8:00));

        let monday = quick_snoozes(datetime!(2026-07-27 12:00));
        assert_eq!(monday.next_week, datetime!(2026-08-03 8:00));
        assert_eq!(monday.weekend, datetime!(2026-08-01 8:00));
    }

    #[test]
    fn storage_datetime_format_round_trips_stably() {
        let value = PrimitiveDateTime::new(
            Date::from_calendar_date(2026, Month::July, 23).unwrap(),
            Time::from_hms(14, 5, 6).unwrap(),
        );
        assert_eq!(format_datetime(value), "2026-07-23T14:05:06");
        assert_eq!(parse_datetime(&format_datetime(value)).unwrap(), value);
    }

    #[test]
    fn dropdown_opens_centered_and_search_does_not_commit_until_enter() {
        let mut dialog = SnoozeDialog::new("task".into(), datetime!(2026-07-23 12:00), None, false);
        assert!(dialog.dropdown.is_open());
        let mut layout = LayoutCtx::new();
        dialog.layout(Rect::new(0, 0, 46, 1), &mut layout);
        let popup = dialog.dropdown.popup_overlay_area(Rect::new(0, 0, 100, 30));
        assert_eq!(popup.x, 30);
        assert!(popup.y > 0);

        let mut ctx = EventCtx::default();
        for key in "weekend".chars() {
            dialog.event(&TuiEvent::Key(Key::Char(key).into()), &mut ctx);
        }
        assert!(ctx.messages().is_empty());
        assert_eq!(dialog.dropdown.search_query(), "weekend");

        dialog.event(&TuiEvent::Key(Key::Enter.into()), &mut ctx);
        assert!(matches!(
            ctx.messages(),
            [AppMsg::SnoozeTask {
                task_id,
                until,
                remember_custom: None
            }] if task_id == "task" && *until == datetime!(2026-07-25 8:00)
        ));
    }

    #[test]
    fn menu_host_and_field_normalize_to_small_terminal_bounds() {
        let mut dialog = SnoozeDialog::new("task".into(), datetime!(2026-07-23 12:00), None, false);
        let proposal = LayoutProposal::at_most(20, 5);

        assert_eq!(
            dialog.measure(proposal).preferred,
            tuicore::LayoutSize::new(20, 5)
        );

        let area = Rect::new(10, 5, 20, 5);
        dialog.layout(area, &mut LayoutCtx::new());
        assert_eq!(dialog.menu_field_area, Rect::new(10, 7, 20, 1));
        assert_eq!(dialog.dropdown.popup_overlay_area(area), area);
    }

    #[test]
    fn pick_choice_switches_open_dropdown_to_stepped_picker() {
        let mut dialog = SnoozeDialog::new("task".into(), datetime!(2026-07-23 12:00), None, false);
        let mut ctx = EventCtx::default();
        for _ in 0..3 {
            dialog.event(
                &TuiEvent::Key(KeyEvent {
                    code: Key::Char('j'),
                    modifiers: KeyModifiers::CONTROL,
                }),
                &mut ctx,
            );
        }

        dialog.event(&TuiEvent::Key(Key::Enter.into()), &mut ctx);

        assert!(matches!(dialog.mode, SnoozeMode::Picker));
        assert!(ctx.messages().is_empty());
        assert!(matches!(
            ctx.focus_request(),
            Some(FocusRequest::TargetAt { id, .. }) if id.as_str() == "date-time-picker"
        ));
    }

    #[test]
    fn pick_choice_reopens_picker_after_escape_back_to_rebuilt_dropdown() {
        let mut dialog = SnoozeDialog::new("task".into(), datetime!(2026-07-23 12:00), None, false);
        let mut ctx = EventCtx::default();
        let next = TuiEvent::Key(KeyEvent {
            code: Key::Char('j'),
            modifiers: KeyModifiers::CONTROL,
        });
        for _ in 0..3 {
            dialog.event(&next, &mut ctx);
        }
        dialog.event(&TuiEvent::Key(Key::Enter.into()), &mut ctx);
        assert!(matches!(dialog.mode, SnoozeMode::Picker));

        dialog.event(&TuiEvent::Key(Key::Esc.into()), &mut ctx);
        assert!(matches!(dialog.mode, SnoozeMode::Menu));
        assert!(dialog.dropdown.is_open());
        assert!(matches!(
            ctx.focus_request(),
            Some(FocusRequest::TargetAt { path, id })
                if path == &dialog.focus_path && id.as_str() == "input"
        ));

        for _ in 0..3 {
            dialog.event(&next, &mut ctx);
        }
        dialog.event(&TuiEvent::Key(Key::Enter.into()), &mut ctx);

        assert!(matches!(dialog.mode, SnoozeMode::Picker));
        assert!(matches!(
            ctx.focus_request(),
            Some(FocusRequest::TargetAt { id, .. }) if id.as_str() == "date-time-picker"
        ));
    }

    #[test]
    fn unsnooze_choice_exists_only_for_snoozed_tasks() {
        let quick = quick_snoozes(datetime!(2026-07-23 12:00));
        assert!(
            snooze_options(quick, None, true)
                .iter()
                .any(|option| option.choice == SnoozeChoice::Unsnooze)
        );
        assert!(
            !snooze_options(quick, None, false)
                .iter()
                .any(|option| option.choice == SnoozeChoice::Unsnooze)
        );

        let mut snoozed = SnoozeDialog::new("task".into(), datetime!(2026-07-23 12:00), None, true);
        let mut snoozed_ctx = EventCtx::default();
        for key in "unsnooze".chars() {
            snoozed.event(&TuiEvent::Key(Key::Char(key).into()), &mut snoozed_ctx);
        }
        snoozed.event(&TuiEvent::Key(Key::Enter.into()), &mut snoozed_ctx);
        assert!(matches!(
            snoozed_ctx.messages(),
            [AppMsg::UnsnoozeTask(task_id)] if task_id == "task"
        ));
    }

    #[test]
    fn stepped_picker_completion_emits_custom_snooze() {
        let now = datetime!(2026-07-23 12:00);
        let mut dialog = SnoozeDialog::new("task".into(), now, None, false);
        dialog.mode = SnoozeMode::Picker;
        let enter = TuiEvent::Key(KeyEvent::from(Key::Enter));
        let mut ctx = EventCtx::default();

        dialog.event(&enter, &mut ctx);
        dialog.event(&enter, &mut ctx);

        assert!(matches!(
            ctx.messages(),
            [AppMsg::SnoozeTask {
                task_id,
                until,
                remember_custom: Some(custom)
            }] if task_id == "task"
                && *until == datetime!(2026-07-24 8:00)
                && *custom == *until
        ));
    }

    #[test]
    fn routed_menu_focus_opens_picker_and_stepped_escape_returns_time_date_menu_then_closes() {
        let area = Rect::new(0, 0, 46, 9);
        let mut dialog = SnoozeDialog::new("task".into(), datetime!(2026-07-23 12:00), None, false);
        let mut layout = LayoutCtx::new();
        dialog.layout(area, &mut layout);
        let menu = layout
            .focus_targets()
            .iter()
            .find(|target| target.id.as_str() == "input")
            .expect("dropdown search should register routed focus")
            .clone();
        let mut focus = FocusManager::new();
        let mut dispatcher = TreeDispatcher::new();
        let transition = focus
            .apply_request(
                &FocusRequest::TargetAt {
                    path: menu.path.clone(),
                    id: menu.id.clone(),
                },
                layout.focus_targets(),
            )
            .expect("menu focus should apply");
        dispatcher.dispatch_focus(&mut dialog, transition, AnimationSettings::default());
        let menu_route = EventRoute::new(focus.current_path());

        for _ in 0..3 {
            let effects = dispatcher.dispatch_event(
                &mut dialog,
                &menu_route,
                &TuiEvent::Key(KeyEvent {
                    code: Key::Char('j'),
                    modifiers: KeyModifiers::CONTROL,
                }),
                AnimationSettings::default(),
            );
            assert!(effects.outcome.handled());
        }
        let open = dispatcher.dispatch_event(
            &mut dialog,
            &menu_route,
            &TuiEvent::Key(Key::Enter.into()),
            AnimationSettings::default(),
        );
        assert!(matches!(
            open.focus_request,
            Some(FocusRequest::TargetAt { ref id, .. }) if id.as_str() == "date-time-picker"
        ));

        let mut picker_layout = LayoutCtx::new();
        dialog.layout(area, &mut picker_layout);
        let transition = focus
            .apply_request(
                open.focus_request.as_ref().unwrap(),
                picker_layout.focus_targets(),
            )
            .expect("picker focus should apply");
        dispatcher.dispatch_focus(&mut dialog, transition, AnimationSettings::default());
        let picker_route = EventRoute::new(focus.current_path());
        let date_select = dispatcher.dispatch_event(
            &mut dialog,
            &picker_route,
            &TuiEvent::Key(Key::Enter.into()),
            AnimationSettings::default(),
        );
        assert!(date_select.outcome.handled());
        assert!(dialog.picker_on_time);

        let time_escape = dispatcher.dispatch_event(
            &mut dialog,
            &picker_route,
            &TuiEvent::Key(Key::Esc.into()),
            AnimationSettings::default(),
        );
        assert!(time_escape.outcome.handled());
        assert!(matches!(dialog.mode, SnoozeMode::Picker));
        assert!(!dialog.picker_on_time);

        let date_escape = dispatcher.dispatch_event(
            &mut dialog,
            &picker_route,
            &TuiEvent::Key(Key::Esc.into()),
            AnimationSettings::default(),
        );
        assert!(date_escape.outcome.handled());
        assert!(matches!(dialog.mode, SnoozeMode::Menu));
        assert!(date_escape.messages.is_empty());
        assert!(matches!(
            date_escape.focus_request,
            Some(FocusRequest::TargetAt { ref id, .. }) if id.as_str() == "input"
        ));

        let close = dispatcher.dispatch_event(
            &mut dialog,
            &menu_route,
            &TuiEvent::Key(Key::Esc.into()),
            AnimationSettings::default(),
        );
        assert!(matches!(close.messages.as_slice(), [AppMsg::CloseDialog]));
    }

    #[test]
    fn routed_picker_forwards_non_key_events() {
        let area = Rect::new(0, 0, 24, 10);
        let mut dialog = SnoozeDialog::new("task".into(), datetime!(2026-07-23 12:00), None, false);
        dialog.mode = SnoozeMode::Picker;
        let mut layout = LayoutCtx::new();
        dialog.layout(area, &mut layout);
        let picker = layout
            .focus_targets()
            .iter()
            .find(|target| target.id.as_str() == "date-time-picker")
            .expect("picker should register focus")
            .clone();

        let effects = TreeDispatcher::new().dispatch_event(
            &mut dialog,
            &EventRoute::new(picker.path),
            &TuiEvent::Yank,
            AnimationSettings::default(),
        );

        assert!(effects.outcome.handled());
        assert_eq!(effects.clipboard.as_deref(), Some("2026-07-24T08:00:00"));
    }

    #[test]
    fn external_editor_date_completion_tracks_time_stage_before_escape() {
        let area = Rect::new(0, 0, 24, 10);
        let mut dialog = SnoozeDialog::new("task".into(), datetime!(2026-07-23 12:00), None, false);
        dialog.mode = SnoozeMode::Picker;
        let mut layout = LayoutCtx::new();
        dialog.layout(area, &mut layout);
        let picker = layout
            .focus_targets()
            .iter()
            .find(|target| target.id.as_str() == "date-time-picker")
            .unwrap();
        let route = EventRoute::new(picker.path.clone());
        let mut dispatcher = TreeDispatcher::new();

        let edited = dispatcher.dispatch_event(
            &mut dialog,
            &route,
            &TuiEvent::ExternalEditor(tuicore::ExternalEditorResponse {
                value: "2026-07-30".into(),
                line: 1,
                col: 11,
            }),
            AnimationSettings::default(),
        );
        assert!(edited.outcome.handled());
        assert!(dialog.picker_on_time);

        let escape = dispatcher.dispatch_event(
            &mut dialog,
            &route,
            &TuiEvent::Key(Key::Esc.into()),
            AnimationSettings::default(),
        );
        assert!(escape.outcome.handled());
        assert!(matches!(dialog.mode, SnoozeMode::Picker));
        assert!(!dialog.picker_on_time);
    }
}
