use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
};
use tuicore::{
    ActivationMode, AnimationSettings, ChildKey, Column, DataView, DataViewTypedEvent, Dialog,
    DialogHost, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusTarget, Key,
    KeyEvent, KeyModifiers, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    RenderCtx, SelectionMode, SelectionTrigger, TextInput, TextareaInput, TickResult, TuiEvent,
    TuiNode,
};

use super::ManagementDialogKind;
use super::common::{
    ManagementPane, RequiredTextInput, dropdown_single_optional, management_empty_state,
    person_choices,
};
use crate::{
    app::{AppContext, AppMsg},
    app_keymap::{self, keys},
    domain::{AppEvent, Person, Workspace, WorkspacePatch},
    persistence_coordinator::PersistenceCommand,
    ui::save_status::SaveStatusLine,
};

type WorkspaceTable = DataView<Workspace, String>;
type WorkspacePatchSink = Rc<RefCell<Vec<WorkspacePatch>>>;
pub(crate) type WorkspacesDialog = DialogHost<WorkspaceManagement, AppMsg>;

pub(crate) fn dialog(context: AppContext) -> WorkspacesDialog {
    Dialog::new()
        .top_left("Workspaces")
        .close_on_unfocus_from_descendants(true)
        .on_close(|_| AppMsg::CloseDialog)
        .host(WorkspaceManagement::new(context))
}

pub(crate) struct WorkspaceManagement {
    context: AppContext,
    split: ManagementPane<WorkspaceTable, WorkspaceDetailForm>,
    observed_version: u64,
    observed_external_refresh_version: u64,
    table_focused: bool,
    detail_draft_protected: bool,
    table_people: Vec<(String, String)>,
    detail_people: Vec<(String, String)>,
}

impl WorkspaceManagement {
    fn new(context: AppContext) -> Self {
        let split = workspace_split(&context);
        let store = context.store.borrow();
        let state = store.state();
        let observed_version = state.version;
        let observed_external_refresh_version = state.external_refresh_version;
        let people = people_signature(&state.people);
        drop(store);
        Self {
            context,
            split,
            observed_version,
            observed_external_refresh_version,
            table_focused: false,
            detail_draft_protected: false,
            table_people: people.clone(),
            detail_people: people,
        }
    }
    fn sync_store_version(&mut self) {
        let store = self.context.store.borrow();
        let state = store.state();
        let version = state.version;
        let people_signature = people_signature(&state.people);
        let table_people_changed = self.table_people != people_signature;
        let detail_people_changed = self.detail_people != people_signature;
        let external_refresh =
            self.observed_external_refresh_version != state.external_refresh_version;
        if self.observed_version == version && !external_refresh && !detail_people_changed {
            return;
        }
        let protect_detail = external_refresh
            && (self.detail_draft_protected || self.context.coordinator.borrow().has_pending());
        let external_refresh_version = state.external_refresh_version;
        let rows = state.workspaces.clone();
        let has_workspaces = !rows.is_empty();
        let people = state.people.clone();
        let selected_id = state.selected_workspace_id.clone();
        drop(store);
        if table_people_changed {
            let transform_mode = self.split.first().transform_mode();
            let transform = self.split.first().transform_state().clone();
            let highlighted_id = self.split.first().highlighted_id();
            let mut table = workspace_table(rows, &people, selected_id.as_deref());
            table.set_transform_mode(transform_mode);
            table.set_search_query(transform.search);
            for filter in transform.filters {
                table.set_filter(filter.column_id, filter.value);
            }
            if let Some(id) = highlighted_id {
                table.highlight_id(&id);
            }
            *self.split.first_mut() = table;
            self.table_people = people_signature.clone();
        } else {
            self.split.first_mut().set_rows(rows);
        }
        self.split
            .first_mut()
            .set_empty_state(management_empty_state(
                ManagementDialogKind::Workspaces,
                has_workspaces,
            ));
        if let Some(id) = selected_id.as_ref() {
            self.split.first_mut().highlight_id(id);
            self.split.first_mut().select_id(id.clone());
        }
        self.split.first_mut().take_events();
        let visible_id = self.split.first().highlighted_id();
        let (workspace, error) = {
            let store = self.context.store.borrow();
            let state = store.state();
            let workspace = visible_id
                .as_deref()
                .and_then(|id| state.workspaces.iter().find(|workspace| workspace.id == id))
                .cloned();
            let error = visible_id
                .as_deref()
                .and_then(|id| state.workspace_save_error(id))
                .map(str::to_string);
            (workspace, error)
        };
        if self.split.second().workspace_id.as_deref() != visible_id.as_deref()
            || (external_refresh && !protect_detail)
            || (detail_people_changed && !self.detail_draft_protected && !protect_detail)
        {
            self.split.second_mut().set_workspace(
                workspace.as_ref(),
                &people,
                error.as_deref(),
                &mut EventCtx::default(),
            );
            self.detail_people = people_signature;
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
                    selected_changed |= self.select_workspace(id, ctx);
                    focus_detail |= matches!(event, DataViewTypedEvent::Activated { .. });
                }
                DataViewTypedEvent::HighlightChanged { row_id: None } => {
                    let people = self.context.store.borrow().state().people.clone();
                    self.split
                        .second_mut()
                        .set_workspace(None, &people, None, ctx);
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
    fn select_workspace(&mut self, id: &str, ctx: &mut EventCtx<AppMsg>) -> bool {
        let outcome = self
            .context
            .store
            .borrow_mut()
            .dispatch(AppEvent::SelectWorkspace(id.to_string()));
        let store = self.context.store.borrow();
        let state = store.state();
        let workspace = state
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .cloned();
        let people = state.people.clone();
        let error = workspace
            .as_ref()
            .and_then(|workspace| state.workspace_save_error(&workspace.id))
            .map(str::to_string);
        drop(store);
        self.split
            .second_mut()
            .set_workspace(workspace.as_ref(), &people, error.as_deref(), ctx);
        let visibility_changed = self.split.set_detail_visible(workspace.is_some());
        outcome.changed || visibility_changed
    }
    fn sync_detail_changes(&mut self) -> bool {
        let patches = self.split.second_mut().take_patches();
        let mut changed = false;
        for (workspace_id, patch) in patches {
            let outcome = self
                .context
                .store
                .borrow_mut()
                .dispatch(AppEvent::PatchWorkspace {
                    workspace_id: workspace_id.clone(),
                    patch: patch.clone(),
                });
            if outcome.changed {
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::PatchWorkspace(workspace_id, patch));
                changed = true;
            }
        }
        if changed {
            self.detail_draft_protected = false;
            let store = self.context.store.borrow();
            let state = store.state();
            self.split.first_mut().set_rows(state.workspaces.clone());
            self.split.second_mut().set_save_error(
                state
                    .selected_workspace_id
                    .as_deref()
                    .and_then(|id| state.workspace_save_error(id)),
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
                kind: ManagementDialogKind::Workspaces,
                entity_id,
            });
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        outcome
    }
}

fn people_signature(people: &[Person]) -> Vec<(String, String)> {
    people
        .iter()
        .map(|person| (person.id.clone(), person.name.clone()))
        .collect()
}

impl TuiNode<AppMsg> for WorkspaceManagement {
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

struct WorkspaceDetailForm {
    root: Flex<AppMsg>,
    workspace_id: Option<String>,
    patches: WorkspacePatchSink,
    save_status: SaveStatusLine,
}
impl WorkspaceDetailForm {
    fn new(workspace: Option<&Workspace>, people: &[Person], error: Option<&str>) -> Self {
        let patches = Rc::new(RefCell::new(Vec::new()));
        let status = SaveStatusLine::new(error);
        Self {
            root: Flex::column().child(
                "form",
                workspace_detail_form(workspace, people, Rc::clone(&patches), status.clone()),
                FlexItem::content(),
            ),
            workspace_id: workspace.map(|workspace| workspace.id.clone()),
            patches,
            save_status: status,
        }
    }
    fn take_patches(&mut self) -> Vec<(String, WorkspacePatch)> {
        let Some(id) = self.workspace_id.clone() else {
            self.patches.borrow_mut().clear();
            return Vec::new();
        };
        self.patches
            .borrow_mut()
            .drain(..)
            .map(|patch| (id.clone(), patch))
            .collect()
    }
    fn set_workspace(
        &mut self,
        workspace: Option<&Workspace>,
        people: &[Person],
        error: Option<&str>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.patches = Rc::new(RefCell::new(Vec::new()));
        self.workspace_id = workspace.map(|workspace| workspace.id.clone());
        self.save_status = SaveStatusLine::new(error);
        self.root
            .replace(
                "form",
                workspace_detail_form(
                    workspace,
                    people,
                    Rc::clone(&self.patches),
                    self.save_status.clone(),
                ),
                FlexItem::content(),
                ctx,
            )
            .expect("workspace detail form host should contain form child");
    }
    fn set_save_error(&self, error: Option<&str>) {
        self.save_status.set_error(error);
    }
}
impl TuiNode<AppMsg> for WorkspaceDetailForm {
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

fn workspace_split(context: &AppContext) -> ManagementPane<WorkspaceTable, WorkspaceDetailForm> {
    let store = context.store.borrow();
    let state = store.state();
    let selected = state.selected_workspace_id.as_deref();
    let workspace =
        selected.and_then(|id| state.workspaces.iter().find(|workspace| workspace.id == id));
    let detail = WorkspaceDetailForm::new(
        workspace,
        &state.people,
        workspace.and_then(|workspace| state.workspace_save_error(&workspace.id)),
    );
    ManagementPane::new(
        workspace_table(state.workspaces.clone(), &state.people, selected),
        detail,
        ManagementDialogKind::Workspaces,
    )
    .detail_visible(workspace.is_some())
}

fn workspace_table(
    rows: Vec<Workspace>,
    people: &[Person],
    selected: Option<&str>,
) -> WorkspaceTable {
    let has_workspaces = !rows.is_empty();
    let names: HashMap<String, String> = people
        .iter()
        .map(|person| (person.id.clone(), person.name.clone()))
        .collect();
    let filter_names = names.clone();
    let mut table = DataView::new(rows, |row: &Workspace| row.id.clone())
        .empty_state(management_empty_state(
            ManagementDialogKind::Workspaces,
            has_workspaces,
        ))
        .headers(true)
        .action_bar(true)
        .filter_controls(false)
        .activation_mode(ActivationMode::OnActivateKey)
        .selection_mode(SelectionMode::Single)
        .selection_trigger(SelectionTrigger::OnNavigate)
        .columns(vec![
            Column::text(
                "key",
                "Key",
                Constraint::Percentage(20),
                |row: &Workspace| row.key.clone(),
            )
            .sortable(|row| row.key.clone()),
            Column::text(
                "name",
                "Workspace",
                Constraint::Percentage(45),
                |row: &Workspace| row.name.clone(),
            ),
            Column::text(
                "lead",
                "Lead",
                Constraint::Percentage(35),
                move |row: &Workspace| {
                    row.lead_person_id
                        .as_ref()
                        .and_then(|id| names.get(id))
                        .cloned()
                        .unwrap_or_else(|| "—".into())
                },
            )
            .filter_key(move |row| {
                row.lead_person_id
                    .as_ref()
                    .and_then(|id| filter_names.get(id))
                    .cloned()
                    .unwrap_or_else(|| "none".into())
            }),
        ]);
    if let Some(id) = selected {
        table = table.selected([id.to_string()]);
    }
    table
}

type WorkspaceKeyCommit = Box<dyn Fn(&str)>;

pub(crate) struct WorkspaceKeyInput {
    input: TextInput<AppMsg>,
    committed_value: String,
    on_commit: Option<WorkspaceKeyCommit>,
    on_invalid: Option<WorkspaceKeyCommit>,
}

impl WorkspaceKeyInput {
    pub(crate) fn new(input: TextInput<AppMsg>) -> Self {
        let committed_value = input.current_value().to_string();
        Self {
            input,
            committed_value,
            on_commit: None,
            on_invalid: None,
        }
    }

    pub(crate) fn on_commit(mut self, handler: impl Fn(&str) + 'static) -> Self {
        self.on_commit = Some(Box::new(handler));
        self
    }

    pub(crate) fn on_invalid(mut self, handler: impl Fn(&str) + 'static) -> Self {
        self.on_invalid = Some(Box::new(handler));
        self
    }
}

impl TuiNode<AppMsg> for WorkspaceKeyInput {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.input.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.input.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        <TextInput<AppMsg> as TuiNode<AppMsg>>::render(&self.input, frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        let commit = self.input.insert_mode()
            && matches!(
                event,
                TuiEvent::Key(KeyEvent {
                    code: Key::Enter,
                    modifiers: KeyModifiers::NONE,
                })
            );
        let outcome = self.input.event(event, ctx);
        if commit {
            let value = self.input.current_value().to_string();
            if Workspace::is_valid_key(&value) {
                self.committed_value = Workspace::normalize_key(&value);
                self.input.set_value(self.committed_value.clone());
                if let Some(on_commit) = &self.on_commit {
                    on_commit(&self.committed_value);
                }
            } else {
                self.input.set_value(self.committed_value.clone());
                if let Some(on_invalid) = &self.on_invalid {
                    on_invalid(&self.committed_value);
                }
                ctx.notify(tuicore::Notification::warning(
                    "Invalid workspace key",
                    "Use 2-5 characters without spaces.",
                ));
                ctx.request_redraw();
            }
        }
        outcome
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

fn workspace_detail_form(
    workspace: Option<&Workspace>,
    people: &[Person],
    patches: WorkspacePatchSink,
    status: SaveStatusLine,
) -> Flex<AppMsg> {
    let Some(workspace) = workspace else {
        return Flex::column();
    };
    Flex::column()
        .gap(0)
        .child("save-status", status, FlexItem::content())
        .child(
            "key",
            WorkspaceKeyInput::new(
                TextInput::new()
                    .value(workspace.key.clone())
                    .placeholder("Workspace key")
                    .panel("Key")
                    .max_len(5)
                    .hotkey(keys::WORKSPACE_KEY_FIELD.hotkey()),
            )
            .on_commit({
                let patches = Rc::clone(&patches);
                move |value| {
                    patches
                        .borrow_mut()
                        .push(WorkspacePatch::Key(value.to_string()));
                }
            }),
            FlexItem::fixed(3),
        )
        .child(
            "name",
            RequiredTextInput::new(
                TextInput::new()
                    .value(workspace.name.clone())
                    .placeholder("Workspace name")
                    .panel("Name")
                    .hotkey(keys::WORKSPACE_NAME_FIELD.hotkey()),
                "Invalid workspace name",
                {
                    let patches = Rc::clone(&patches);
                    move |value| {
                        patches
                            .borrow_mut()
                            .push(WorkspacePatch::Name(value.to_string()));
                    }
                },
            ),
            FlexItem::fixed(3),
        )
        .child(
            "description",
            TextareaInput::new()
                .value(workspace.description.clone())
                .placeholder("Workspace description")
                .panel("Description")
                .hotkey(keys::WORKSPACE_DESCRIPTION_FIELD.hotkey())
                .editor_hotkey(keys::WORKSPACE_DESCRIPTION_EDITOR.hotkey())
                .on_edit_end({
                    let patches = Rc::clone(&patches);
                    move |value| {
                        patches
                            .borrow_mut()
                            .push(WorkspacePatch::Description(value));
                        AppMsg::Noop
                    }
                })
                .min_rows(2)
                .max_rows(6),
            FlexItem::content(),
        )
        .child(
            "lead",
            dropdown_single_optional(
                "Lead",
                "Select workspace lead",
                person_choices(people),
                workspace.lead_person_id.as_deref(),
                move |id| patches.borrow_mut().push(WorkspacePatch::LeadPerson(id)),
            )
            .hotkey(keys::WORKSPACE_LEAD_FIELD.hotkey()),
            FlexItem::fixed(3),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::tests::{rendered_text, test_context},
        domain::{PersonPatch, WorkspaceSnapshot},
    };
    use tuicore::{FocusRequest, HotkeyEvent, Key, KeyEvent, KeyModifiers};

    #[test]
    fn invalid_workspace_key_commit_restores_original_and_notifies() {
        let reverted = Rc::new(RefCell::new(None));
        let mut input =
            WorkspaceKeyInput::new(TextInput::new().value("CORE").focused(true).max_len(5))
                .on_invalid({
                    let reverted = Rc::clone(&reverted);
                    move |value| *reverted.borrow_mut() = Some(value.to_string())
                });
        input.event(
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        for _ in 0..3 {
            input.event(
                &TuiEvent::Key(KeyEvent::from(Key::Backspace)),
                &mut EventCtx::default(),
            );
        }
        let mut ctx = EventCtx::default();

        let outcome = input.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
        let effects = tuicore::DispatchEffects::from_event_ctx(outcome, ctx);

        assert_eq!(input.input.current_value(), "CORE");
        assert_eq!(reverted.borrow().as_deref(), Some("CORE"));
        assert_eq!(
            effects.notifications,
            vec![tuicore::Notification::warning(
                "Invalid workspace key",
                "Use 2-5 characters without spaces.",
            )]
        );
    }

    #[test]
    fn management_workspace_renders_and_edits_workspace() {
        let person = Person {
            id: "person-1".into(),
            name: "Ada".into(),
            email: "ada@example.com".into(),
            about: "Owns architecture decisions".into(),
            active: true,
        };
        let workspace = Workspace {
            id: "workspace-1".into(),
            key: "CORE".into(),
            name: "Core".into(),
            description: "Platform".into(),
            lead_person_id: Some(person.id.clone()),
        };
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![person],
            workspaces: vec![workspace],
            tags: vec![],
        });
        let mut workspace = WorkspaceManagement::new(context);
        let area = Rect::new(0, 0, 100, 30);
        workspace.layout(area, &mut LayoutCtx::new());
        let text = rendered_text(&workspace, area);
        for expected in ["Workspace", "CORE", "Core", "Ada", "Description"] {
            assert!(text.contains(expected), "missing {expected}");
        }
        workspace
            .split
            .second_mut()
            .patches
            .borrow_mut()
            .push(WorkspacePatch::Name("Foundation".into()));
        assert!(workspace.sync_detail_changes());
        assert_eq!(store.borrow().state().workspaces[0].name, "Foundation");
    }

    #[test]
    fn local_person_rename_updates_workspace_table_and_lead_choice() {
        let person = Person::new("person-1".into(), "Ada".into(), String::new());
        let mut workspace = Workspace::new(
            "workspace-1".into(),
            "CORE".into(),
            "Core".into(),
            String::new(),
        );
        workspace.lead_person_id = Some(person.id.clone());
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![person],
            workspaces: vec![workspace],
            tags: vec![],
        });
        let mut workspace = WorkspaceManagement::new(context);
        store.borrow_mut().dispatch(AppEvent::PatchPerson {
            person_id: "person-1".into(),
            patch: PersonPatch::Name("Grace".into()),
        });
        let area = Rect::new(0, 0, 100, 30);

        workspace.layout(area, &mut LayoutCtx::new());
        let text = rendered_text(&workspace, area);

        assert!(
            text.matches("Grace").count() >= 2,
            "table and detail should update: {text}"
        );
        assert!(!text.contains("Ada"));
    }

    #[test]
    fn local_person_deletion_removes_stale_workspace_lead_name() {
        let person = Person::new("person-1".into(), "Ada".into(), String::new());
        let mut workspace = Workspace::new(
            "workspace-1".into(),
            "CORE".into(),
            "Core".into(),
            String::new(),
        );
        workspace.lead_person_id = Some(person.id.clone());
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![person],
            workspaces: vec![workspace],
            tags: vec![],
        });
        let mut workspace = WorkspaceManagement::new(context);
        store
            .borrow_mut()
            .dispatch(AppEvent::PersonDeleted("person-1".into()));
        let area = Rect::new(0, 0, 100, 30);

        workspace.layout(area, &mut LayoutCtx::new());
        let text = rendered_text(&workspace, area);

        assert!(!text.contains("Ada"));
        assert_eq!(store.borrow().state().workspaces[0].lead_person_id, None);
    }

    #[test]
    fn delete_hotkey_requests_confirmation_for_selected_workspace() {
        let workspace = Workspace::new(
            "workspace-1".into(),
            "CORE".into(),
            "Core".into(),
            String::new(),
        );
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![],
            workspaces: vec![workspace],
            tags: vec![],
        });
        let mut workspace = WorkspaceManagement::new(context);
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
                kind: ManagementDialogKind::Workspaces,
                entity_id,
            }] if entity_id == "workspace-1"
        ));
    }

    #[test]
    fn delete_hotkey_does_nothing_when_search_has_no_highlighted_workspace() {
        let workspace = Workspace::new(
            "workspace-1".into(),
            "CORE".into(),
            "Core".into(),
            String::new(),
        );
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![],
            workspaces: vec![workspace],
            tags: vec![],
        });
        let mut workspace = WorkspaceManagement::new(context);
        workspace.table_focused = true;
        workspace.split.first_mut().set_search_query("App");
        workspace.sync_table_events(&mut EventCtx::default());
        let area = Rect::new(0, 0, 100, 30);
        workspace.layout(area, &mut LayoutCtx::new());
        assert!(rendered_text(&workspace, area).contains("No workspaces match your search"));
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
    fn search_match_after_no_match_shows_different_workspace_detail() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![],
            workspaces: vec![
                Workspace::new(
                    "workspace-1".into(),
                    "CORE".into(),
                    "Core".into(),
                    String::new(),
                ),
                Workspace::new(
                    "workspace-2".into(),
                    "APP".into(),
                    "App".into(),
                    String::new(),
                ),
            ],
            tags: vec![],
        });
        let mut workspace = WorkspaceManagement::new(context);
        let mut ctx = EventCtx::default();

        workspace.split.first_mut().set_search_query("nobody");
        workspace.sync_table_events(&mut ctx);
        workspace.split.first_mut().set_search_query("App");
        workspace.sync_table_events(&mut ctx);

        assert_eq!(
            workspace.split.first().highlighted_id().as_deref(),
            Some("workspace-2")
        );
        assert_eq!(
            workspace.split.second().workspace_id.as_deref(),
            Some("workspace-2")
        );
        assert!(workspace.split.is_detail_visible());
    }

    #[test]
    fn person_refresh_preserves_workspace_search_and_visible_detail() {
        let person = Person::new("person-1".into(), "Ada".into(), String::new());
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![person],
            workspaces: vec![
                Workspace::new(
                    "workspace-1".into(),
                    "CORE".into(),
                    "Core".into(),
                    String::new(),
                ),
                Workspace::new(
                    "workspace-2".into(),
                    "APP".into(),
                    "App".into(),
                    String::new(),
                ),
            ],
            tags: vec![],
        });
        let mut workspace = WorkspaceManagement::new(context);
        workspace.split.first_mut().set_search_query("App");
        workspace.sync_table_events(&mut EventCtx::default());
        store.borrow_mut().dispatch(AppEvent::PatchPerson {
            person_id: "person-1".into(),
            patch: PersonPatch::Name("Grace".into()),
        });

        workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());

        assert_eq!(workspace.split.first().transform_state().search, "App");
        assert_eq!(
            workspace.split.first().highlighted_id().as_deref(),
            Some("workspace-2")
        );
        assert_eq!(
            workspace.split.second().workspace_id.as_deref(),
            Some("workspace-2")
        );
        assert!(workspace.split.is_detail_visible());
    }

    #[test]
    fn workspaces_table_disables_filter_mode() {
        let mut table = workspace_table(Vec::new(), &[], None);

        let outcome = table.on_key(tuicore::Key::Char('f'), Rect::new(0, 0, 80, 20));

        assert!(!outcome.handled);
        assert!(!outcome.changed);
        assert!(table.transform_state().filters.is_empty());
    }

    #[test]
    fn newly_created_workspace_is_selected_and_shown_in_detail() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![],
            workspaces: vec![Workspace::new(
                "workspace-1".into(),
                "CORE".into(),
                "Core".into(),
                String::new(),
            )],
            tags: vec![],
        });
        let mut workspace = WorkspaceManagement::new(context);
        store
            .borrow_mut()
            .dispatch(AppEvent::WorkspaceCreated(Workspace::new(
                "workspace-2".into(),
                "APP".into(),
                "App".into(),
                String::new(),
            )));

        workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());

        assert_eq!(
            workspace.split.first().highlighted_id().as_deref(),
            Some("workspace-2")
        );
        assert_eq!(
            workspace.split.first().selected_id().as_deref(),
            Some("workspace-2")
        );
        assert_eq!(
            workspace.split.second().workspace_id.as_deref(),
            Some("workspace-2")
        );
    }

    #[test]
    fn escape_from_workspace_detail_focuses_table_before_closing_dialog() {
        let workspace = Workspace::new(
            "workspace-1".into(),
            "CORE".into(),
            "Core".into(),
            "Platform".into(),
        );
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![],
            workspaces: vec![workspace],
            tags: vec![],
        });
        let mut dialog = dialog(context);
        let mut layout = LayoutCtx::new();
        dialog.layout(Rect::new(0, 0, 100, 30), &mut layout);
        let description = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target
                    .path
                    .keys()
                    .iter()
                    .any(|key| key.as_str() == "description")
            })
            .expect("workspace description should be focusable")
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
                &EventRoute::new(description.path.clone()),
                &TuiEvent::Key(key),
                &mut ctx,
            );

            assert!(outcome.handled());
            assert!(ctx.messages().is_empty());
            assert!(matches!(ctx.focus_request(), Some(FocusRequest::Path(_))));
        }
    }

    #[test]
    fn workspace_detail_controls_register_requested_hotkeys() {
        let workspace = Workspace::new(
            "workspace-1".into(),
            "CORE".into(),
            "Core".into(),
            "Platform".into(),
        );
        let mut detail = WorkspaceDetailForm::new(Some(&workspace), &[], None);
        let mut layout = LayoutCtx::new();
        detail.layout(Rect::new(0, 0, 80, 24), &mut layout);

        for hotkey in [
            keys::WORKSPACE_KEY_FIELD.hotkey(),
            keys::WORKSPACE_NAME_FIELD.hotkey(),
            keys::WORKSPACE_DESCRIPTION_FIELD.hotkey(),
            keys::WORKSPACE_DESCRIPTION_EDITOR.hotkey(),
            keys::WORKSPACE_LEAD_FIELD.hotkey(),
        ] {
            assert_eq!(
                layout
                    .focus_targets()
                    .iter()
                    .filter(|target| target.hotkey_sequences.contains(&hotkey))
                    .count(),
                1,
                "{hotkey} should be registered once"
            );
        }

        let description = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target
                    .hotkey_sequences
                    .contains(&keys::WORKSPACE_DESCRIPTION_EDITOR.hotkey())
            })
            .expect("description editor hotkey should have a target");
        let mut ctx = EventCtx::default();
        let outcome = detail.dispatch_event(
            &EventRoute::new(description.path.clone()),
            &TuiEvent::Hotkey(HotkeyEvent::Commit(
                keys::WORKSPACE_DESCRIPTION_EDITOR.hotkey(),
            )),
            &mut ctx,
        );
        assert!(outcome.handled());
        assert_eq!(
            ctx.external_editor_request()
                .map(|request| request.value.as_str()),
            Some("Platform")
        );
    }
}
