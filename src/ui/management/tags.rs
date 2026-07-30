use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
};
use tuicore::{
    ActivationMode, AnimationSettings, ChildKey, Column, DataView, DataViewTypedEvent, Dialog,
    DialogHost, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusTarget,
    LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx,
    SelectionMode, SelectionTrigger, TextInput, TickResult, TuiEvent, TuiNode,
};

use super::ManagementDialogKind;
use super::common::{ManagementPane, management_empty_state};
use crate::{
    app::{AppContext, AppMsg},
    app_keymap::{self, keys},
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
        .close_on_unfocus_from_descendants(true)
        .on_close(|_| AppMsg::CloseDialog)
        .host(TagsWorkspace::new(context))
}

pub(crate) struct TagsWorkspace {
    context: AppContext,
    split: ManagementPane<TagTable, TagDetailForm>,
    observed_version: u64,
    observed_external_refresh_version: u64,
    table_focused: bool,
    detail_draft_protected: bool,
}
impl TagsWorkspace {
    fn new(context: AppContext) -> Self {
        let split = tag_split(&context);
        let observed_version = context.store.borrow().state().version;
        let observed_external_refresh_version =
            context.store.borrow().state().external_refresh_version;
        Self {
            context,
            split,
            observed_version,
            observed_external_refresh_version,
            table_focused: false,
            detail_draft_protected: false,
        }
    }
    fn sync_store_version(&mut self) {
        let store = self.context.store.borrow();
        let state = store.state();
        let version = state.version;
        let external_refresh =
            self.observed_external_refresh_version != state.external_refresh_version;
        if self.observed_version == version && !external_refresh {
            return;
        }
        let protect_detail = external_refresh
            && (self.detail_draft_protected || self.context.coordinator.borrow().has_pending());
        let external_refresh_version = state.external_refresh_version;
        let rows = state.tags.clone();
        let has_tags = !rows.is_empty();
        let selected_id = state.selected_tag_id.clone();
        drop(store);
        self.split.first_mut().set_rows(rows);
        self.split
            .first_mut()
            .set_empty_state(management_empty_state(ManagementDialogKind::Tags, has_tags));
        if let Some(id) = selected_id.as_ref() {
            self.split.first_mut().highlight_id(id);
            self.split.first_mut().select_id(id.clone());
        }
        self.split.first_mut().take_events();
        let visible_id = self.split.first().highlighted_id();
        let (tag, error) = {
            let store = self.context.store.borrow();
            let state = store.state();
            let tag = visible_id
                .as_deref()
                .and_then(|id| state.tags.iter().find(|tag| tag.id == id))
                .cloned();
            let error = visible_id
                .as_deref()
                .and_then(|id| state.tag_save_error(id))
                .map(str::to_string);
            (tag, error)
        };
        if self.split.second().tag_id.as_deref() != visible_id.as_deref()
            || (external_refresh && !protect_detail)
        {
            self.split.second_mut().set_tag(
                tag.as_ref(),
                error.as_deref(),
                &mut EventCtx::default(),
            );
        } else {
            self.split.second_mut().set_save_error(error.as_deref());
        }
        self.split.set_detail_visible(visible_id.is_some());
        self.observed_version = version;
        if !protect_detail {
            self.observed_external_refresh_version = external_refresh_version;
        }
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
                DataViewTypedEvent::HighlightChanged { row_id: None } => {
                    self.split.second_mut().set_tag(None, None, ctx);
                    self.detail_draft_protected = false;
                    selected_changed |= self.split.set_detail_visible(false);
                }
                DataViewTypedEvent::SelectionChanged { .. }
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
        let store = self.context.store.borrow();
        let state = store.state();
        let tag = state.tags.iter().find(|tag| tag.id == id).cloned();
        let error = tag
            .as_ref()
            .and_then(|tag| state.tag_save_error(&tag.id))
            .map(str::to_string);
        drop(store);
        self.split
            .second_mut()
            .set_tag(tag.as_ref(), error.as_deref(), ctx);
        let visibility_changed = self.split.set_detail_visible(tag.is_some());
        outcome.changed || visibility_changed
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
            self.detail_draft_protected = false;
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
    fn handle_workspace_event(
        &self,
        outcome: EventOutcome,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        if outcome.handled() || !self.table_focused {
            return outcome;
        }
        let selected = self.split.first().highlighted_id();
        if let Some(entity_id) = selected
            && app_keymap::matches_any(
                event,
                &[
                    keys::MANAGEMENT_DELETE_X,
                    keys::MANAGEMENT_DELETE,
                    keys::MANAGEMENT_DELETE_BACKSPACE,
                ],
            )
        {
            ctx.emit(AppMsg::OpenDeleteManagement {
                kind: ManagementDialogKind::Tags,
                entity_id,
            });
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        outcome
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
        self.handle_workspace_event(outcome, event, ctx)
    }
    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        if self.split.return_to_table_on_unfocus(route, event, ctx) {
            return EventOutcome::Handled;
        }
        let outcome = self.split.dispatch_event(route, event, ctx);
        if self.sync_detail_changes() {
            ctx.request_redraw();
        }
        self.sync_table_events(ctx);
        self.handle_workspace_event(outcome, event, ctx)
    }
    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        if target.for_child(&ChildKey::first()).is_some() {
            self.table_focused = focused;
        } else if focused {
            self.table_focused = false;
        }
        if target.for_child(&ChildKey::second()).is_some() {
            self.detail_draft_protected = focused;
        } else if focused {
            self.detail_draft_protected = false;
        }
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

fn tag_split(context: &AppContext) -> ManagementPane<TagTable, TagDetailForm> {
    let store = context.store.borrow();
    let state = store.state();
    let selected = state.selected_tag_id.as_deref();
    let tag = selected.and_then(|id| state.tags.iter().find(|tag| tag.id == id));
    let detail = TagDetailForm::new(tag, tag.and_then(|tag| state.tag_save_error(&tag.id)));
    ManagementPane::new(
        tag_table(state.tags.clone(), selected),
        detail,
        ManagementDialogKind::Tags,
    )
    .detail_visible(tag.is_some())
}
fn tag_table(rows: Vec<Tag>, selected: Option<&str>) -> TagTable {
    let has_tags = !rows.is_empty();
    let mut table = DataView::new(rows, |row: &Tag| row.id.clone())
        .empty_state(management_empty_state(ManagementDialogKind::Tags, has_tags))
        .headers(true)
        .action_bar(true)
        .filter_controls(false)
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
        return Flex::column();
    };
    Flex::column()
        .gap(0)
        .child("save-status", status, FlexItem::content())
        .child(
            "label",
            TextInput::new()
                .value(tag.label.clone())
                .panel("Label")
                .hotkey(keys::TAG_LABEL_FIELD.hotkey())
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
    use tuicore::{FocusRequest, Key, KeyEvent, KeyModifiers};

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

    #[test]
    fn delete_hotkey_requests_confirmation_for_selected_tag() {
        let tag = Tag::new("tag-1".into(), "api".into());
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![],
            projects: vec![],
            tags: vec![tag],
        });
        let mut workspace = TagsWorkspace::new(context);
        workspace.table_focused = true;
        let mut ctx = EventCtx::default();

        let outcome = workspace.handle_workspace_event(
            EventOutcome::Ignored,
            &TuiEvent::Key(tuicore::KeyEvent {
                code: tuicore::Key::Char('x'),
                modifiers: tuicore::KeyModifiers::CONTROL,
            }),
            &mut ctx,
        );

        assert!(outcome.handled());
        assert!(matches!(
            ctx.messages(),
            [AppMsg::OpenDeleteManagement {
                kind: ManagementDialogKind::Tags,
                entity_id,
            }] if entity_id == "tag-1"
        ));
    }

    #[test]
    fn delete_hotkey_does_nothing_when_search_has_no_highlighted_tag() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![],
            projects: vec![],
            tags: vec![Tag::new("tag-1".into(), "api".into())],
        });
        let mut workspace = TagsWorkspace::new(context);
        workspace.table_focused = true;
        workspace.split.first_mut().set_search_query("frontend");
        workspace.sync_table_events(&mut EventCtx::default());
        let area = Rect::new(0, 0, 100, 30);
        workspace.layout(area, &mut LayoutCtx::new());
        assert!(rendered_text(&workspace, area).contains("No tags match your search"));
        let mut ctx = EventCtx::default();

        let outcome = workspace.handle_workspace_event(
            EventOutcome::Ignored,
            &TuiEvent::Key(KeyEvent {
                code: Key::Char('x'),
                modifiers: KeyModifiers::CONTROL,
            }),
            &mut ctx,
        );

        assert!(!outcome.handled());
        assert!(ctx.messages().is_empty());
    }

    #[test]
    fn search_match_after_no_match_shows_different_tag_detail() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![],
            projects: vec![],
            tags: vec![
                Tag::new("tag-1".into(), "api".into()),
                Tag::new("tag-2".into(), "frontend".into()),
            ],
        });
        let mut workspace = TagsWorkspace::new(context);
        let mut ctx = EventCtx::default();

        workspace.split.first_mut().set_search_query("nobody");
        workspace.sync_table_events(&mut ctx);
        workspace.split.first_mut().set_search_query("frontend");
        workspace.sync_table_events(&mut ctx);

        assert_eq!(
            workspace.split.first().highlighted_id().as_deref(),
            Some("tag-2")
        );
        assert_eq!(workspace.split.second().tag_id.as_deref(), Some("tag-2"));
        assert!(workspace.split.is_detail_visible());
    }

    #[test]
    fn tags_table_disables_filter_mode() {
        let mut table = tag_table(Vec::new(), None);

        let outcome = table.on_key(tuicore::Key::Char('f'), Rect::new(0, 0, 80, 20));

        assert!(!outcome.handled);
        assert!(!outcome.changed);
        assert!(table.transform_state().filters.is_empty());
    }

    #[test]
    fn newly_created_tag_is_selected_and_shown_in_detail() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![],
            projects: vec![],
            tags: vec![Tag::new("tag-1".into(), "api".into())],
        });
        let mut workspace = TagsWorkspace::new(context);
        store.borrow_mut().dispatch(AppEvent::TagCreated(Tag::new(
            "tag-2".into(),
            "frontend".into(),
        )));

        workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());

        assert_eq!(
            workspace.split.first().highlighted_id().as_deref(),
            Some("tag-2")
        );
        assert_eq!(
            workspace.split.first().selected_id().as_deref(),
            Some("tag-2")
        );
        assert_eq!(workspace.split.second().tag_id.as_deref(), Some("tag-2"));
    }

    #[test]
    fn escape_from_tag_detail_focuses_table_before_closing_dialog() {
        let tag = Tag::new("tag-1".into(), "api".into());
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![],
            projects: vec![],
            tags: vec![tag],
        });
        let mut dialog = dialog(context);
        let mut layout = LayoutCtx::new();
        dialog.layout(Rect::new(0, 0, 100, 30), &mut layout);
        let label = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().iter().any(|key| key.as_str() == "label"))
            .expect("tag label should be focusable")
            .clone();

        for key in [
            KeyEvent::from(Key::Esc),
            KeyEvent {
                code: Key::Char('['),
                modifiers: KeyModifiers::CONTROL,
            },
        ] {
            let mut ctx = EventCtx::default();
            let outcome = dialog.dispatch_event(
                &EventRoute::new(label.path.clone()),
                &TuiEvent::Key(key),
                &mut ctx,
            );

            assert!(outcome.handled());
            assert!(ctx.messages().is_empty());
            assert!(matches!(ctx.focus_request(), Some(FocusRequest::Path(_))));
        }
    }

    #[test]
    fn tag_label_registers_requested_hotkey() {
        let tag = Tag::new("tag-1".into(), "api".into());
        let mut detail = TagDetailForm::new(Some(&tag), None);
        let mut layout = LayoutCtx::new();
        detail.layout(Rect::new(0, 0, 80, 12), &mut layout);

        let hotkey = keys::TAG_LABEL_FIELD.hotkey();
        assert_eq!(
            layout
                .focus_targets()
                .iter()
                .filter(|target| target.hotkey_sequences.contains(&hotkey))
                .count(),
            1
        );
    }
}
