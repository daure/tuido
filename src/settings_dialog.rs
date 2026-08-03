use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect};
use time::Time;
use tuicore::{
    AnimationSettings, Dropdown, DropdownCommitMode, DropdownSearchMode, EventCtx, EventOutcome,
    EventRoute, Flex, FlexItem, FocusCtx, FocusTarget, LayoutCtx, LayoutProposal, LayoutResult,
    LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult, TimePicker, Toggle, TuiEvent, TuiNode,
};

use crate::{app::AppMsg, domain::Workspace};

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
        show_calendar_weekends: bool,
        default_snooze_time: Time,
        workspaces: &[Workspace],
        default_workspace_id: Option<&str>,
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
        .label("Default workspace")
        .placeholder("Unset")
        .no_selection_text("Unset")
        .selected(default_workspace_id.into_iter().map(str::to_string))
        .search_mode(DropdownSearchMode::Contains)
        .commit_mode(DropdownCommitMode::Explicit)
        .on_select(move |ids| {
            change_sink.borrow_mut().push(ids.into_iter().next());
        });
        let root = Flex::column()
            .gap(0)
            .child("calendar-view", calendar_view, FlexItem::content())
            .child("snooze-time", snooze_time, FlexItem::fixed(3))
            .child("default-workspace", default_workspace, FlexItem::content());
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
    use time::macros::time;
    use tuicore::{Key, KeyEvent};

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
        let mut dialog = SettingsDialog::new(false, time!(8:15), &[], None);
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
        let mut dialog = SettingsDialog::new(true, time!(8:15), &[], None);
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
            true,
            time!(8:15),
            std::slice::from_ref(&workspace),
            Some(&workspace.id),
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
}
