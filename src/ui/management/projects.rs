use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
};
use tuicore::{
    ActivationMode, AnimationSettings, ChildKey, Column, DataView, DataViewTypedEvent, Dialog,
    DialogHost, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusTarget, Key,
    KeyEvent, KeyModifiers, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    Paragraph, RenderCtx, SelectionMode, SelectionTrigger, TextInput, TextareaInput, TickResult,
    TuiEvent, TuiNode,
};

use super::ManagementDialogKind;
use super::common::{ManagementPane, dropdown_single_optional, person_choices};
use crate::{
    app::{AppContext, AppMsg},
    app_keymap::{self, keys},
    domain::{AppEvent, Person, Project, ProjectPatch},
    persistence_coordinator::PersistenceCommand,
    ui::save_status::SaveStatusLine,
};

type ProjectTable = DataView<Project, String>;
type ProjectPatchSink = Rc<RefCell<Vec<ProjectPatch>>>;
pub(crate) type ProjectsDialog = DialogHost<ProjectsWorkspace, AppMsg>;

pub(crate) fn dialog(context: AppContext) -> ProjectsDialog {
    Dialog::new()
        .top_left("Projects")
        .close_on_unfocus_from_descendants(true)
        .on_close(|_| AppMsg::CloseDialog)
        .host(ProjectsWorkspace::new(context))
}

pub(crate) struct ProjectsWorkspace {
    context: AppContext,
    split: ManagementPane<ProjectTable, ProjectDetailForm>,
    observed_version: u64,
    observed_external_refresh_version: u64,
    table_focused: bool,
    detail_draft_protected: bool,
    table_people: Vec<(String, String)>,
    detail_people: Vec<(String, String)>,
}

impl ProjectsWorkspace {
    fn new(context: AppContext) -> Self {
        let split = project_split(&context);
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
        let rows = state.projects.clone();
        let people = state.people.clone();
        let selected_id = state.selected_project_id.clone();
        let project = selected_id
            .as_deref()
            .and_then(|id| state.projects.iter().find(|project| project.id == id))
            .cloned();
        let error = selected_id
            .as_deref()
            .and_then(|id| state.project_save_error(id))
            .map(str::to_string);
        drop(store);
        if table_people_changed {
            *self.split.first_mut() = project_table(rows, &people, selected_id.as_deref());
            self.table_people = people_signature.clone();
        } else {
            self.split.first_mut().set_rows(rows);
        }
        if let Some(id) = selected_id.as_ref() {
            self.split.first_mut().highlight_id(id);
            self.split.first_mut().select_id(id.clone());
        }
        self.split.first_mut().take_events();
        if self.split.second().project_id.as_deref() != selected_id.as_deref()
            || (external_refresh && !protect_detail)
            || (detail_people_changed && !self.detail_draft_protected && !protect_detail)
        {
            self.split.second_mut().set_project(
                project.as_ref(),
                &people,
                error.as_deref(),
                &mut EventCtx::default(),
            );
            self.detail_people = people_signature;
        } else {
            self.split.second_mut().set_save_error(error.as_deref());
        }
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
                    selected_changed |= self.select_project(id, ctx);
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
    fn select_project(&mut self, id: &str, ctx: &mut EventCtx<AppMsg>) -> bool {
        let outcome = self
            .context
            .store
            .borrow_mut()
            .dispatch(AppEvent::SelectProject(id.to_string()));
        if outcome.changed {
            let store = self.context.store.borrow();
            let state = store.state();
            let project = state.projects.iter().find(|project| project.id == id);
            let error = project.and_then(|project| state.project_save_error(&project.id));
            self.split
                .second_mut()
                .set_project(project, &state.people, error, ctx);
        }
        outcome.changed
    }
    fn sync_detail_changes(&mut self) -> bool {
        let patches = self.split.second_mut().take_patches();
        let mut changed = false;
        for (project_id, patch) in patches {
            let outcome = self
                .context
                .store
                .borrow_mut()
                .dispatch(AppEvent::PatchProject {
                    project_id: project_id.clone(),
                    patch: patch.clone(),
                });
            if outcome.changed {
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::PatchProject(project_id, patch));
                changed = true;
            }
        }
        if changed {
            self.detail_draft_protected = false;
            let store = self.context.store.borrow();
            let state = store.state();
            self.split.first_mut().set_rows(state.projects.clone());
            self.split.second_mut().set_save_error(
                state
                    .selected_project_id
                    .as_deref()
                    .and_then(|id| state.project_save_error(id)),
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
        let selected = self
            .context
            .store
            .borrow()
            .state()
            .selected_project_id
            .clone();
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
                kind: ManagementDialogKind::Projects,
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

impl TuiNode<AppMsg> for ProjectsWorkspace {
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

struct ProjectDetailForm {
    root: Flex<AppMsg>,
    project_id: Option<String>,
    patches: ProjectPatchSink,
    save_status: SaveStatusLine,
}
impl ProjectDetailForm {
    fn new(project: Option<&Project>, people: &[Person], error: Option<&str>) -> Self {
        let patches = Rc::new(RefCell::new(Vec::new()));
        let status = SaveStatusLine::new(error);
        Self {
            root: Flex::column().child(
                "form",
                project_detail_form(project, people, Rc::clone(&patches), status.clone()),
                FlexItem::content(),
            ),
            project_id: project.map(|project| project.id.clone()),
            patches,
            save_status: status,
        }
    }
    fn take_patches(&mut self) -> Vec<(String, ProjectPatch)> {
        let Some(id) = self.project_id.clone() else {
            self.patches.borrow_mut().clear();
            return Vec::new();
        };
        self.patches
            .borrow_mut()
            .drain(..)
            .map(|patch| (id.clone(), patch))
            .collect()
    }
    fn set_project(
        &mut self,
        project: Option<&Project>,
        people: &[Person],
        error: Option<&str>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.patches = Rc::new(RefCell::new(Vec::new()));
        self.project_id = project.map(|project| project.id.clone());
        self.save_status = SaveStatusLine::new(error);
        self.root
            .replace(
                "form",
                project_detail_form(
                    project,
                    people,
                    Rc::clone(&self.patches),
                    self.save_status.clone(),
                ),
                FlexItem::content(),
                ctx,
            )
            .expect("project detail form host should contain form child");
    }
    fn set_save_error(&self, error: Option<&str>) {
        self.save_status.set_error(error);
    }
}
impl TuiNode<AppMsg> for ProjectDetailForm {
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

fn project_split(context: &AppContext) -> ManagementPane<ProjectTable, ProjectDetailForm> {
    let store = context.store.borrow();
    let state = store.state();
    let selected = state.selected_project_id.as_deref();
    let project = selected.and_then(|id| state.projects.iter().find(|project| project.id == id));
    let detail = ProjectDetailForm::new(
        project,
        &state.people,
        project.and_then(|project| state.project_save_error(&project.id)),
    );
    ManagementPane::new(
        project_table(state.projects.clone(), &state.people, selected),
        detail,
        ManagementDialogKind::Projects,
    )
}

fn project_table(rows: Vec<Project>, people: &[Person], selected: Option<&str>) -> ProjectTable {
    let names: HashMap<String, String> = people
        .iter()
        .map(|person| (person.id.clone(), person.name.clone()))
        .collect();
    let filter_names = names.clone();
    let mut table = DataView::new(rows, |row: &Project| row.id.clone())
        .headers(true)
        .action_bar(true)
        .filter_controls(false)
        .activation_mode(ActivationMode::OnActivateKey)
        .selection_mode(SelectionMode::Single)
        .selection_trigger(SelectionTrigger::OnNavigate)
        .columns(vec![
            Column::text("key", "Key", Constraint::Percentage(20), |row: &Project| {
                row.key.clone()
            })
            .sortable(|row| row.key.clone()),
            Column::text(
                "name",
                "Project",
                Constraint::Percentage(45),
                |row: &Project| row.name.clone(),
            ),
            Column::text(
                "lead",
                "Lead",
                Constraint::Percentage(35),
                move |row: &Project| {
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

pub(crate) struct ProjectKeyInput {
    input: TextInput<AppMsg>,
    on_commit: Option<Box<dyn Fn(&str)>>,
}

impl ProjectKeyInput {
    pub(crate) fn new(input: TextInput<AppMsg>) -> Self {
        Self {
            input,
            on_commit: None,
        }
    }

    pub(crate) fn on_commit(mut self, handler: impl Fn(&str) + 'static) -> Self {
        self.on_commit = Some(Box::new(handler));
        self
    }

    fn normalize(&mut self) {
        self.input
            .set_value(self.input.current_value().to_uppercase());
    }
}

impl TuiNode<AppMsg> for ProjectKeyInput {
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
            self.normalize();
            if let Some(on_commit) = &self.on_commit {
                on_commit(self.input.current_value());
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

fn project_detail_form(
    project: Option<&Project>,
    people: &[Person],
    patches: ProjectPatchSink,
    status: SaveStatusLine,
) -> Flex<AppMsg> {
    let Some(project) = project else {
        return Flex::column().child(
            "empty",
            Paragraph::new("No project selected."),
            FlexItem::fixed(1),
        );
    };
    Flex::column()
        .gap(0)
        .child("save-status", status, FlexItem::content())
        .child(
            "key",
            ProjectKeyInput::new(
                TextInput::new()
                    .value(project.key.clone())
                    .panel("Key")
                    .hotkey(keys::PROJECT_KEY_FIELD.hotkey()),
            )
            .on_commit({
                let patches = Rc::clone(&patches);
                move |value| {
                    patches
                        .borrow_mut()
                        .push(ProjectPatch::Key(value.to_string()));
                }
            }),
            FlexItem::fixed(3),
        )
        .child(
            "name",
            TextInput::new()
                .value(project.name.clone())
                .panel("Name")
                .hotkey(keys::PROJECT_NAME_FIELD.hotkey())
                .on_edit_end({
                    let patches = Rc::clone(&patches);
                    move |value| {
                        patches.borrow_mut().push(ProjectPatch::Name(value));
                        AppMsg::Noop
                    }
                }),
            FlexItem::fixed(3),
        )
        .child(
            "description",
            TextareaInput::new()
                .value(project.description.clone())
                .panel("Description")
                .hotkey(keys::PROJECT_DESCRIPTION_FIELD.hotkey())
                .editor_hotkey(keys::PROJECT_DESCRIPTION_EDITOR.hotkey())
                .on_edit_end({
                    let patches = Rc::clone(&patches);
                    move |value| {
                        patches.borrow_mut().push(ProjectPatch::Description(value));
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
                person_choices(people),
                project.lead_person_id.as_deref(),
                move |id| patches.borrow_mut().push(ProjectPatch::LeadPerson(id)),
            )
            .hotkey(keys::PROJECT_LEAD_FIELD.hotkey()),
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
    fn project_key_input_becomes_uppercase_after_pressing_enter() {
        let commits = Rc::new(RefCell::new(Vec::new()));
        let mut input = ProjectKeyInput::new(TextInput::new().focused(true)).on_commit({
            let commits = Rc::clone(&commits);
            move |value| commits.borrow_mut().push(value.to_string())
        });

        input.event(
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        assert!(input.input.insert_mode());
        input.event(
            &TuiEvent::Key(KeyEvent::from(Key::Char('c'))),
            &mut EventCtx::default(),
        );
        assert_eq!(input.input.current_value(), "c");

        input.event(
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );

        assert_eq!(input.input.current_value(), "C");
        assert_eq!(*commits.borrow(), vec!["C"]);
    }

    #[test]
    fn project_key_input_does_not_commit_on_escape() {
        let commits = Rc::new(RefCell::new(Vec::new()));
        let mut input = ProjectKeyInput::new(TextInput::new().focused(true)).on_commit({
            let commits = Rc::clone(&commits);
            move |value| commits.borrow_mut().push(value.to_string())
        });
        input.event(
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        input.event(
            &TuiEvent::Key(KeyEvent::from(Key::Char('c'))),
            &mut EventCtx::default(),
        );

        input.event(
            &TuiEvent::Key(KeyEvent::from(Key::Esc)),
            &mut EventCtx::default(),
        );

        assert_eq!(input.input.current_value(), "c");
        assert!(commits.borrow().is_empty());
    }

    #[test]
    fn management_workspace_renders_and_edits_project() {
        let person = Person {
            id: "person-1".into(),
            name: "Ada".into(),
            email: "ada@example.com".into(),
            about: "Owns architecture decisions".into(),
            active: true,
        };
        let project = Project {
            id: "project-1".into(),
            key: "CORE".into(),
            name: "Core".into(),
            description: "Platform".into(),
            lead_person_id: Some(person.id.clone()),
        };
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![person],
            projects: vec![project],
            tags: vec![],
        });
        let mut workspace = ProjectsWorkspace::new(context);
        let area = Rect::new(0, 0, 100, 30);
        workspace.layout(area, &mut LayoutCtx::new());
        let text = rendered_text(&workspace, area);
        for expected in ["Project", "CORE", "Core", "Ada", "Description"] {
            assert!(text.contains(expected), "missing {expected}");
        }
        workspace
            .split
            .second_mut()
            .patches
            .borrow_mut()
            .push(ProjectPatch::Name("Foundation".into()));
        assert!(workspace.sync_detail_changes());
        assert_eq!(store.borrow().state().projects[0].name, "Foundation");
    }

    #[test]
    fn local_person_rename_updates_project_table_and_lead_choice() {
        let person = Person::new("person-1".into(), "Ada".into(), String::new());
        let mut project = Project::new(
            "project-1".into(),
            "CORE".into(),
            "Core".into(),
            String::new(),
        );
        project.lead_person_id = Some(person.id.clone());
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![person],
            projects: vec![project],
            tags: vec![],
        });
        let mut workspace = ProjectsWorkspace::new(context);
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
    fn local_person_deletion_removes_stale_project_lead_name() {
        let person = Person::new("person-1".into(), "Ada".into(), String::new());
        let mut project = Project::new(
            "project-1".into(),
            "CORE".into(),
            "Core".into(),
            String::new(),
        );
        project.lead_person_id = Some(person.id.clone());
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![person],
            projects: vec![project],
            tags: vec![],
        });
        let mut workspace = ProjectsWorkspace::new(context);
        store
            .borrow_mut()
            .dispatch(AppEvent::PersonDeleted("person-1".into()));
        let area = Rect::new(0, 0, 100, 30);

        workspace.layout(area, &mut LayoutCtx::new());
        let text = rendered_text(&workspace, area);

        assert!(!text.contains("Ada"));
        assert_eq!(store.borrow().state().projects[0].lead_person_id, None);
    }

    #[test]
    fn delete_hotkey_requests_confirmation_for_selected_project() {
        let project = Project::new(
            "project-1".into(),
            "CORE".into(),
            "Core".into(),
            String::new(),
        );
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![],
            projects: vec![project],
            tags: vec![],
        });
        let mut workspace = ProjectsWorkspace::new(context);
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
                kind: ManagementDialogKind::Projects,
                entity_id,
            }] if entity_id == "project-1"
        ));
    }

    #[test]
    fn projects_table_disables_filter_mode() {
        let mut table = project_table(Vec::new(), &[], None);

        let outcome = table.on_key(tuicore::Key::Char('f'), Rect::new(0, 0, 80, 20));

        assert!(!outcome.handled);
        assert!(!outcome.changed);
        assert!(table.transform_state().filters.is_empty());
    }

    #[test]
    fn newly_created_project_is_selected_and_shown_in_detail() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![],
            projects: vec![Project::new(
                "project-1".into(),
                "CORE".into(),
                "Core".into(),
                String::new(),
            )],
            tags: vec![],
        });
        let mut workspace = ProjectsWorkspace::new(context);
        store
            .borrow_mut()
            .dispatch(AppEvent::ProjectCreated(Project::new(
                "project-2".into(),
                "APP".into(),
                "App".into(),
                String::new(),
            )));

        workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());

        assert_eq!(
            workspace.split.first().highlighted_id().as_deref(),
            Some("project-2")
        );
        assert_eq!(
            workspace.split.first().selected_id().as_deref(),
            Some("project-2")
        );
        assert_eq!(
            workspace.split.second().project_id.as_deref(),
            Some("project-2")
        );
    }

    #[test]
    fn escape_from_project_detail_focuses_table_before_closing_dialog() {
        let project = Project::new(
            "project-1".into(),
            "CORE".into(),
            "Core".into(),
            "Platform".into(),
        );
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![],
            projects: vec![project],
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
            .expect("project description should be focusable")
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
    fn project_detail_controls_register_requested_hotkeys() {
        let project = Project::new(
            "project-1".into(),
            "CORE".into(),
            "Core".into(),
            "Platform".into(),
        );
        let mut detail = ProjectDetailForm::new(Some(&project), &[], None);
        let mut layout = LayoutCtx::new();
        detail.layout(Rect::new(0, 0, 80, 24), &mut layout);

        for hotkey in [
            keys::PROJECT_KEY_FIELD.hotkey(),
            keys::PROJECT_NAME_FIELD.hotkey(),
            keys::PROJECT_DESCRIPTION_FIELD.hotkey(),
            keys::PROJECT_DESCRIPTION_EDITOR.hotkey(),
            keys::PROJECT_LEAD_FIELD.hotkey(),
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
                    .contains(&keys::PROJECT_DESCRIPTION_EDITOR.hotkey())
            })
            .expect("description editor hotkey should have a target");
        let mut ctx = EventCtx::default();
        let outcome = detail.dispatch_event(
            &EventRoute::new(description.path.clone()),
            &TuiEvent::Hotkey(HotkeyEvent::Commit(
                keys::PROJECT_DESCRIPTION_EDITOR.hotkey(),
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
