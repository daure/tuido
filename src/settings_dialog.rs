use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect};
use time::Time;
use tuicore::{
    AnimationSettings, Dropdown, DropdownCommitMode, DropdownSearchMode, EventCtx, EventOutcome,
    EventRoute, Flex, FlexItem, FocusCtx, FocusTarget, LayoutCtx, LayoutProposal, LayoutResult,
    LayoutSizeHint, LifecycleCtx, RenderCtx, TextInput, TickResult, TimePicker, Toggle, TuiEvent,
    TuiNode,
};

use crate::{
    app::AppMsg,
    domain::Workspace,
    persistence_coordinator::AppStore,
    speed_reader_settings::{SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING, SPEED_READER_WPM_SETTING},
    ui::save_status::SaveStatusLine,
};

#[derive(Clone)]
struct WorkspaceChoice {
    id: String,
    label: String,
}

pub(crate) struct SettingsDialog {
    root: Flex<AppMsg>,
    default_workspace_changes: Rc<RefCell<Vec<Option<String>>>>,
}

impl SettingsDialog {
    pub(crate) fn new(
        store: AppStore,
        show_calendar_weekends: bool,
        default_snooze_time: Time,
        workspaces: &[Workspace],
        default_workspace_id: Option<&str>,
        speed_reader_wpm: u16,
        markdown_block_pause: Duration,
    ) -> Self {
        let calendar_view = Toggle::new("Show weekends in calendar")
            .checked(show_calendar_weekends)
            .focused(true)
            .on_change(AppMsg::SetShowCalendarWeekends);
        let snooze_time = TimePicker::new()
            .panel("Default snooze time")
            .value(default_snooze_time)
            .on_select(AppMsg::SetDefaultSnoozeTime);
        let workspace_choices = workspaces
            .iter()
            .map(|workspace| WorkspaceChoice {
                id: workspace.id.clone(),
                label: workspace.name.clone(),
            })
            .collect::<Vec<_>>();
        let default_workspace_changes = Rc::new(RefCell::new(Vec::new()));
        let change_sink = Rc::clone(&default_workspace_changes);
        let default_workspace = Dropdown::single(
            workspace_choices,
            |choice| choice.id.clone(),
            |choice| choice.label.clone(),
        )
        .label("Default space")
        .placeholder("Unset")
        .no_selection_text("Unset")
        .selected(default_workspace_id.into_iter().map(str::to_string))
        .search_mode(DropdownSearchMode::Contains)
        .commit_mode(DropdownCommitMode::Explicit)
        .on_select(move |ids| {
            change_sink.borrow_mut().push(ids.into_iter().next());
        });
        let speed_reader = Flex::row()
            .gap(1)
            .child(
                "speed-reader-wpm",
                SettingTextInput::new(
                    Rc::clone(&store),
                    SPEED_READER_WPM_SETTING,
                    speed_reader_wpm.to_string(),
                    "Reader WPM (ms)",
                    AppMsg::SetSpeedReaderWpm,
                ),
                FlexItem::fill(1),
            )
            .child(
                "markdown-block-pause",
                SettingTextInput::new(
                    store,
                    SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING,
                    markdown_block_pause.as_millis().to_string(),
                    "Reader block delay (ms)",
                    AppMsg::SetMarkdownBlockPause,
                ),
                FlexItem::fill(1),
            );
        let root = Flex::column()
            .gap(0)
            .child("calendar-view", calendar_view, FlexItem::content())
            .child("snooze-time", snooze_time, FlexItem::fixed(3))
            .child("default-workspace", default_workspace, FlexItem::content())
            .child("speed-reader", speed_reader, FlexItem::content());
        Self {
            root,
            default_workspace_changes,
        }
    }

    fn emit_default_workspace_changes(&self, ctx: &mut EventCtx<AppMsg>) {
        for workspace_id in self.default_workspace_changes.borrow_mut().drain(..) {
            ctx.emit(AppMsg::SetDefaultWorkspace(workspace_id));
        }
    }
}

struct SettingTextInput {
    input: TextInput<AppMsg>,
    status: SaveStatusLine,
    store: AppStore,
    key: &'static str,
    observed_generation: Option<u64>,
    observed_error: Option<String>,
}

impl SettingTextInput {
    fn new(
        store: AppStore,
        key: &'static str,
        value: String,
        label: &'static str,
        on_edit_end: fn(String) -> AppMsg,
    ) -> Self {
        let (generation, error) = {
            let store = store.borrow();
            (
                store.state().app_setting_generations.get(key).copied(),
                store.state().app_setting_errors.get(key).cloned(),
            )
        };
        Self {
            input: TextInput::new()
                .value(value)
                .numbers_only(true)
                .panel(label)
                .on_edit_end(on_edit_end),
            status: SaveStatusLine::new(error.as_deref()),
            store,
            key,
            observed_generation: generation,
            observed_error: error,
        }
    }

    fn sync_save_failure(&mut self) {
        let (value, generation, error) = {
            let store = self.store.borrow();
            let state = store.state();
            (
                state.app_setting_values.get(self.key).cloned(),
                state.app_setting_generations.get(self.key).copied(),
                state.app_setting_errors.get(self.key).cloned(),
            )
        };
        if generation == self.observed_generation && error == self.observed_error {
            return;
        }
        if error.is_some()
            && let Some(value) = value
        {
            self.input.set_value(value);
            self.input.move_cursor_to_end();
        }
        self.status.set_error(error.as_deref());
        self.observed_generation = generation;
        self.observed_error = error;
    }
}

impl TuiNode<AppMsg> for SettingTextInput {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let mut hint = self.input.measure(proposal);
        if self
            .store
            .borrow()
            .state()
            .app_setting_errors
            .contains_key(self.key)
        {
            hint.min.height = hint.min.height.saturating_add(1);
            hint.preferred.height = hint.preferred.height.saturating_add(1);
        }
        hint
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync_save_failure();
        let input_height = area.height.min(3);
        self.input
            .layout(Rect::new(area.x, area.y, area.width, input_height), ctx);
        if area.height > input_height {
            self.status.layout(
                Rect::new(
                    area.x,
                    area.y.saturating_add(input_height),
                    area.width,
                    area.height - input_height,
                ),
                ctx,
            );
        }
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        let input_height = area.height.min(3);
        <TextInput<AppMsg> as TuiNode<AppMsg>>::render(
            &self.input,
            frame,
            Rect::new(area.x, area.y, area.width, input_height),
            ctx,
        );
        if area.height > input_height {
            self.status.render(
                frame,
                Rect::new(
                    area.x,
                    area.y.saturating_add(input_height),
                    area.width,
                    area.height - input_height,
                ),
                ctx,
            );
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        self.input.event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        self.input.dispatch_event(route, event, ctx)
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
        let outcome = self.root.event(event, ctx);
        self.emit_default_workspace_changes(ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let outcome = self.root.dispatch_event(route, event, ctx);
        self.emit_default_workspace_changes(ctx);
        outcome
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
    use crate::domain::{AppEvent, AppState, WorkspaceSnapshot, reduce_app_state};
    use time::macros::time;
    use tuicore::{Key, KeyEvent, Store};

    fn store() -> AppStore {
        Rc::new(RefCell::new(Store::new(
            AppState::from_snapshot(WorkspaceSnapshot {
                tasks: Vec::new(),
                people: Vec::new(),
                workspaces: Vec::new(),
                tags: Vec::new(),
            }),
            reduce_app_state as fn(&mut AppState, AppEvent) -> tuicore::DispatchOutcome,
        )))
    }

    fn workspace() -> Workspace {
        Workspace::new(
            "workspace-1".into(),
            "ONE".into(),
            "Workspace one".into(),
            String::new(),
        )
    }

    #[test]
    fn settings_emit_changes_for_immediate_persistence() {
        let mut dialog = SettingsDialog::new(
            store(),
            false,
            time!(8:15),
            &[],
            None,
            300,
            Duration::from_millis(250),
        );
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
        let mut dialog = SettingsDialog::new(
            store(),
            true,
            time!(8:15),
            &[],
            None,
            300,
            Duration::from_millis(250),
        );
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

    #[test]
    fn settings_include_default_workspace_dropdown() {
        let workspace = workspace();
        let mut dialog = SettingsDialog::new(
            store(),
            true,
            time!(8:15),
            std::slice::from_ref(&workspace),
            Some(&workspace.id),
            300,
            Duration::from_millis(250),
        );
        let mut layout = LayoutCtx::new();
        dialog.layout(Rect::new(0, 0, 40, 8), &mut layout);
        let dropdown = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target
                    .path
                    .keys()
                    .iter()
                    .any(|key| key.as_str() == "default-workspace")
            })
            .expect("default workspace dropdown should be focusable");

        assert!(dropdown.area.width > 0);
    }

    #[test]
    fn speed_reader_inputs_follow_default_workspace_side_by_side() {
        let workspace = workspace();
        let mut dialog = SettingsDialog::new(
            store(),
            true,
            time!(8:15),
            std::slice::from_ref(&workspace),
            Some(&workspace.id),
            425,
            Duration::from_millis(1_250),
        );
        let mut layout = LayoutCtx::new();
        dialog.layout(Rect::new(0, 0, 60, 11), &mut layout);
        let targets = layout.focus_targets();
        let workspace = target_with_key(targets, "default-workspace");
        let wpm = target_with_key(targets, "speed-reader-wpm");
        let delay = target_with_key(targets, "markdown-block-pause");

        assert!(wpm.area.y > workspace.area.y);
        assert_eq!(wpm.area.y, delay.area.y);
        assert!(wpm.area.right() < delay.area.x);
    }

    #[test]
    fn speed_reader_inputs_emit_numeric_strings_on_edit_end() {
        let mut dialog = SettingsDialog::new(
            store(),
            true,
            time!(8:15),
            &[],
            None,
            300,
            Duration::from_millis(250),
        );
        let mut layout = LayoutCtx::new();
        dialog.layout(Rect::new(0, 0, 60, 11), &mut layout);

        let wpm = target_with_key(layout.focus_targets(), "speed-reader-wpm").clone();
        let mut wpm_ctx = EventCtx::default();
        edit_value(&mut dialog, &wpm, 3, "425", &mut wpm_ctx);
        assert!(matches!(
            wpm_ctx.messages(),
            [AppMsg::SetSpeedReaderWpm(value)] if value == "425"
        ));

        let delay = target_with_key(layout.focus_targets(), "markdown-block-pause").clone();
        let mut delay_ctx = EventCtx::default();
        edit_value(&mut dialog, &delay, 3, "1250", &mut delay_ctx);
        assert!(matches!(
            delay_ctx.messages(),
            [AppMsg::SetMarkdownBlockPause(value)] if value == "1250"
        ));
    }

    #[test]
    fn speed_reader_setting_inputs_reject_non_digits() {
        let mut input = SettingTextInput::new(
            store(),
            SPEED_READER_WPM_SETTING,
            "300".into(),
            "Reader WPM (ms)",
            AppMsg::SetSpeedReaderWpm,
        );
        input.input.set_focused(true);
        let mut ctx = EventCtx::default();

        input.event(&TuiEvent::Key(Key::Enter.into()), &mut ctx);
        input.event(&TuiEvent::Key(Key::Char('x').into()), &mut ctx);
        input.event(&TuiEvent::Key(Key::Char(' ').into()), &mut ctx);
        input.event(&TuiEvent::Key(Key::Char('4').into()), &mut ctx);

        assert_eq!(input.input.current_value(), "3004");
    }

    fn target_with_key<'a>(targets: &'a [FocusTarget], key: &str) -> &'a FocusTarget {
        targets
            .iter()
            .find(|target| target.path.keys().iter().any(|part| part.as_str() == key))
            .unwrap_or_else(|| panic!("{key} should be focusable"))
    }

    fn edit_value(
        dialog: &mut SettingsDialog,
        target: &FocusTarget,
        current_len: usize,
        value: &str,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        dialog.dispatch_focus(target, true, &mut FocusCtx::default());
        let route = EventRoute::new(target.path.clone());
        dialog.dispatch_event(&route, &TuiEvent::Key(Key::Enter.into()), ctx);
        for _ in 0..current_len {
            dialog.dispatch_event(&route, &TuiEvent::Key(Key::Backspace.into()), ctx);
        }
        for character in value.chars() {
            dialog.dispatch_event(&route, &TuiEvent::Key(Key::Char(character).into()), ctx);
        }
        dialog.dispatch_event(&route, &TuiEvent::Key(Key::Enter.into()), ctx);
    }
}
