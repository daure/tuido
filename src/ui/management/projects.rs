use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
};
use tuicore::{
    ActivationMode, AnimationSettings, Column, DataView, DataViewTypedEvent, Dialog, DialogHost,
    EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusTarget, LayoutCtx,
    LayoutResult, LifecycleCtx, Paragraph, RenderCtx, SelectionMode, SelectionTrigger, Separator,
    Split, TextInput, TextareaInput, TickResult, TuiEvent, TuiNode,
};

use super::common::{detail_outcome_or_escape, dropdown_single_optional, person_choices};
use crate::{
    app::{AppContext, AppMsg},
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
        .on_close(|_| AppMsg::CloseDialog)
        .host(ProjectsWorkspace::new(context))
}

pub(crate) struct ProjectsWorkspace {
    context: AppContext,
    split: Split<ProjectTable, ProjectDetailForm>,
    observed_version: u64,
}

impl ProjectsWorkspace {
    fn new(context: AppContext) -> Self {
        let split = project_split(&context);
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
        let rows = state.projects.clone();
        let error = state
            .selected_project_id
            .as_deref()
            .and_then(|id| state.project_save_error(id))
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

fn project_split(context: &AppContext) -> Split<ProjectTable, ProjectDetailForm> {
    let store = context.store.borrow();
    let state = store.state();
    let selected = state.selected_project_id.as_deref();
    let project = selected.and_then(|id| state.projects.iter().find(|project| project.id == id));
    let detail = ProjectDetailForm::new(
        project,
        &state.people,
        project.and_then(|project| state.project_save_error(&project.id)),
    );
    Split::horizontal(
        project_table(state.projects.clone(), &state.people, selected),
        detail,
    )
    .ratio(65, 35)
    .separator(Separator::new())
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
        .child("save-status", status, FlexItem::fixed(1))
        .child(
            "key",
            TextInput::new()
                .value(project.key.clone())
                .panel("Key")
                .on_edit_end({
                    let patches = Rc::clone(&patches);
                    move |value| {
                        patches.borrow_mut().push(ProjectPatch::Key(value));
                        AppMsg::Noop
                    }
                }),
            FlexItem::fixed(3),
        )
        .child(
            "name",
            TextInput::new()
                .value(project.name.clone())
                .panel("Name")
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
                .on_edit_end({
                    let patches = Rc::clone(&patches);
                    move |value| {
                        patches.borrow_mut().push(ProjectPatch::Description(value));
                        AppMsg::Noop
                    }
                })
                .min_rows(4)
                .max_rows(8),
            FlexItem::fixed(6),
        )
        .child(
            "lead",
            dropdown_single_optional(
                "Lead",
                person_choices(people),
                project.lead_person_id.as_deref(),
                move |id| patches.borrow_mut().push(ProjectPatch::LeadPerson(id)),
            ),
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
    fn management_workspace_renders_and_edits_project() {
        let person = Person {
            id: "person-1".into(),
            name: "Ada".into(),
            email: "ada@example.com".into(),
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
}
