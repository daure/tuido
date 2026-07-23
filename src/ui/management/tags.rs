use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
};
use tuicore::{
    ActivationMode, AnimationSettings, Column, DataView, DataViewTypedEvent, Dialog, DialogHost,
    EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusTarget, LayoutCtx,
    LayoutResult, LifecycleCtx, Paragraph, RenderCtx, SelectionMode, SelectionTrigger, Separator,
    Split, TextInput, TickResult, TuiEvent, TuiNode,
};

use super::common::detail_outcome_or_escape;
use crate::{
    app::{AppContext, AppMsg},
    domain::{AppEvent, Tag, TagPatch},
    persistence_coordinator::PersistenceCommand,
    ui::save_status::SaveStatusLine,
};

type TagTable = DataView<Tag, String>;
type TagPatchSink = Rc<RefCell<Vec<TagPatch>>>;
pub(crate) type TagsDialog = DialogHost<TagsWorkspace, AppMsg>;

pub(crate) fn dialog(context: AppContext) -> TagsDialog {
    Dialog::new()
        .top_left("Tags")
        .on_close(|_| AppMsg::CloseDialog)
        .host(TagsWorkspace::new(context))
}

pub(crate) struct TagsWorkspace {
    context: AppContext,
    split: Split<TagTable, TagDetailForm>,
    observed_version: u64,
}
impl TagsWorkspace {
    fn new(context: AppContext) -> Self {
        let split = tag_split(&context);
        let observed_version = context.store.borrow().state().version;
        Self {
            context,
            split,
            observed_version,
        }
    }
    fn sync_store_version(&mut self) {
        let store = self.context.store.borrow();
        let state = store.state();
        let version = state.version;
        if self.observed_version == version {
            return;
        }
        let rows = state.tags.clone();
        let error = state
            .selected_tag_id
            .as_deref()
            .and_then(|id| state.tag_save_error(id))
            .map(str::to_string);
        drop(store);
        self.split.first_mut().set_rows(rows);
        self.split.second_mut().set_save_error(error.as_deref());
        self.observed_version = version;
    }
    fn sync_table_events(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let events = self.split.first_mut().take_events();
        let mut focus_detail = false;
        let mut selected_changed = false;
        for event in events {
            match &event {
                DataViewTypedEvent::HighlightChanged { row_id: Some(id) }
                | DataViewTypedEvent::Activated { row_id: id } => {
                    selected_changed |= self.select_tag(id, ctx);
                    focus_detail |= matches!(event, DataViewTypedEvent::Activated { .. });
                }
                DataViewTypedEvent::HighlightChanged { row_id: None }
                | DataViewTypedEvent::SelectionChanged { .. }
                | DataViewTypedEvent::TransformChanged { .. } => {}
            }
        }
        if selected_changed {
            ctx.request_layout();
            ctx.request_redraw();
        }
        if focus_detail {
            ctx.focus_next();
            ctx.request_redraw();
        }
    }
    fn select_tag(&mut self, id: &str, ctx: &mut EventCtx<AppMsg>) -> bool {
        let outcome = self
            .context
            .store
            .borrow_mut()
            .dispatch(AppEvent::SelectTag(id.to_string()));
        if outcome.changed {
            let store = self.context.store.borrow();
            let state = store.state();
            let tag = state.tags.iter().find(|tag| tag.id == id);
            let error = tag.and_then(|tag| state.tag_save_error(&tag.id));
            self.split.second_mut().set_tag(tag, error, ctx);
        }
        outcome.changed
    }
    fn sync_detail_changes(&mut self) -> bool {
        let patches = self.split.second_mut().take_patches();
        let mut changed = false;
        for (tag_id, patch) in patches {
            let outcome = self
                .context
                .store
                .borrow_mut()
                .dispatch(AppEvent::PatchTag {
                    tag_id: tag_id.clone(),
                    patch: patch.clone(),
                });
            if outcome.changed {
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::PatchTag(tag_id, patch));
                changed = true;
            }
        }
        if changed {
            let store = self.context.store.borrow();
            let state = store.state();
            self.split.first_mut().set_rows(state.tags.clone());
            self.split.second_mut().set_save_error(
                state
                    .selected_tag_id
                    .as_deref()
                    .and_then(|id| state.tag_save_error(id)),
            );
            self.observed_version = state.version;
        }
        changed
    }
}
impl TuiNode<AppMsg> for TagsWorkspace {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync_store_version();
        self.split.layout(area, ctx)
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.split.render(frame, area, ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        let outcome = self.split.event(event, ctx);
        if self.sync_detail_changes() {
            ctx.request_redraw();
        }
        self.sync_table_events(ctx);
        outcome
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let outcome = self.split.dispatch_event(route, event, ctx);
        if self.sync_detail_changes() {
            ctx.request_redraw();
        }
        self.sync_table_events(ctx);
        outcome
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        self.split.dispatch_focus(target, focused, ctx);
        if self.sync_detail_changes() {
            ctx.request_redraw();
        }
    }
    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.split.tick(dt, settings)
    }
    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.split.init(ctx);
    }
    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.split.mount(ctx);
    }
    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.split.unmount(ctx);
    }
    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.split.destroy(ctx);
    }
}

struct TagDetailForm {
    root: Flex<AppMsg>,
    tag_id: Option<String>,
    patches: TagPatchSink,
    save_status: SaveStatusLine,
}
impl TagDetailForm {
    fn new(tag: Option<&Tag>, error: Option<&str>) -> Self {
        let patches = Rc::new(RefCell::new(Vec::new()));
        let status = SaveStatusLine::new(error);
        Self {
            root: Flex::column().child(
                "form",
                tag_detail_form(tag, Rc::clone(&patches), status.clone()),
                FlexItem::content(),
            ),
            tag_id: tag.map(|tag| tag.id.clone()),
            patches,
            save_status: status,
        }
    }
    fn take_patches(&mut self) -> Vec<(String, TagPatch)> {
        let Some(id) = self.tag_id.clone() else {
            self.patches.borrow_mut().clear();
            return Vec::new();
        };
        self.patches
            .borrow_mut()
            .drain(..)
            .map(|patch| (id.clone(), patch))
            .collect()
    }
    fn set_tag(&mut self, tag: Option<&Tag>, error: Option<&str>, ctx: &mut EventCtx<AppMsg>) {
        self.patches = Rc::new(RefCell::new(Vec::new()));
        self.tag_id = tag.map(|tag| tag.id.clone());
        self.save_status = SaveStatusLine::new(error);
        self.root
            .replace(
                "form",
                tag_detail_form(tag, Rc::clone(&self.patches), self.save_status.clone()),
                FlexItem::content(),
                ctx,
            )
            .expect("tag detail form host should contain form child");
    }
    fn set_save_error(&self, error: Option<&str>) {
        self.save_status.set_error(error);
    }
}
impl TuiNode<AppMsg> for TagDetailForm {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.root.layout(area, ctx)
    }
    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.root.render(frame, area, ctx);
    }
    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        let outcome = self.root.event(event, ctx);
        detail_outcome_or_escape(outcome, event, ctx)
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let outcome = self.root.dispatch_event(route, event, ctx);
        detail_outcome_or_escape(outcome, event, ctx)
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

fn tag_split(context: &AppContext) -> Split<TagTable, TagDetailForm> {
    let store = context.store.borrow();
    let state = store.state();
    let selected = state.selected_tag_id.as_deref();
    let tag = selected.and_then(|id| state.tags.iter().find(|tag| tag.id == id));
    let detail = TagDetailForm::new(tag, tag.and_then(|tag| state.tag_save_error(&tag.id)));
    Split::horizontal(tag_table(state.tags.clone(), selected), detail)
        .ratio(65, 35)
        .separator(Separator::new())
}
fn tag_table(rows: Vec<Tag>, selected: Option<&str>) -> TagTable {
    let mut table = DataView::new(rows, |row: &Tag| row.id.clone())
        .headers(true)
        .action_bar(true)
        .activation_mode(ActivationMode::OnActivateKey)
        .selection_mode(SelectionMode::Single)
        .selection_trigger(SelectionTrigger::OnNavigate)
        .columns(vec![
            Column::text("label", "Tag", Constraint::Fill(1), |row: &Tag| {
                row.label.clone()
            })
            .sortable(|row| row.label.clone())
            .filter_key(|row| row.label.clone()),
        ]);
    if let Some(id) = selected {
        table = table.selected([id.to_string()]);
    }
    table
}
fn tag_detail_form(
    tag: Option<&Tag>,
    patches: TagPatchSink,
    status: SaveStatusLine,
) -> Flex<AppMsg> {
    let Some(tag) = tag else {
        return Flex::column().child(
            "empty",
            Paragraph::new("No tag selected."),
            FlexItem::fixed(1),
        );
    };
    Flex::column()
        .gap(0)
        .child("save-status", status, FlexItem::fixed(1))
        .child(
            "label",
            TextInput::new()
                .value(tag.label.clone())
                .panel("Label")
                .on_edit_end(move |value| {
                    patches.borrow_mut().push(TagPatch::Label(value));
                    AppMsg::Noop
                }),
            FlexItem::fixed(3),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::tests::{rendered_text, test_context},
        domain::WorkspaceSnapshot,
    };
    #[test]
    fn management_workspace_has_table_and_editable_detail() {
        let tags = vec![
            Tag {
                id: "tag-api".into(),
                label: "api".into(),
            },
            Tag {
                id: "tag-backend".into(),
                label: "backend".into(),
            },
        ];
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![],
            projects: vec![],
            tags,
        });
        let mut workspace = TagsWorkspace::new(context);
        let area = Rect::new(0, 0, 100, 30);
        workspace.layout(area, &mut LayoutCtx::new());
        let text = rendered_text(&workspace, area);
        for expected in ["Tag", "api", "backend", "Label"] {
            assert!(text.contains(expected));
        }
        workspace.select_tag("tag-backend", &mut EventCtx::default());
        workspace
            .split
            .second_mut()
            .patches
            .borrow_mut()
            .push(TagPatch::Label("platform".into()));
        assert!(workspace.sync_detail_changes());
        assert_eq!(store.borrow().state().tags[1].label, "platform");
    }
}
