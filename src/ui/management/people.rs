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

use super::common::{active_choices, detail_outcome_or_escape, dropdown_single};
use crate::{
    app::{AppContext, AppMsg},
    domain::{AppEvent, Person, PersonPatch},
    persistence_coordinator::PersistenceCommand,
    ui::save_status::SaveStatusLine,
};

type PersonTable = DataView<Person, String>;
type PersonPatchSink = Rc<RefCell<Vec<PersonPatch>>>;
pub(crate) type PeopleDialog = DialogHost<PeopleWorkspace, AppMsg>;

pub(crate) fn dialog(context: AppContext) -> PeopleDialog {
    Dialog::new()
        .top_left("People")
        .on_close(|_| AppMsg::CloseDialog)
        .host(PeopleWorkspace::new(context))
}

pub(crate) struct PeopleWorkspace {
    context: AppContext,
    split: Split<PersonTable, PersonDetailForm>,
    observed_version: u64,
}

impl PeopleWorkspace {
    fn new(context: AppContext) -> Self {
        let split = person_split(&context);
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
        let rows = state.people.clone();
        let save_error = state
            .selected_person_id
            .as_deref()
            .and_then(|id| state.person_save_error(id))
            .map(str::to_string);
        drop(store);
        self.split.first_mut().set_rows(rows);
        self.split
            .second_mut()
            .set_save_error(save_error.as_deref());
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
                    selected_changed |= self.select_person(id, ctx);
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

    fn select_person(&mut self, id: &str, ctx: &mut EventCtx<AppMsg>) -> bool {
        let outcome = self
            .context
            .store
            .borrow_mut()
            .dispatch(AppEvent::SelectPerson(id.to_string()));
        if outcome.changed {
            let store = self.context.store.borrow();
            let state = store.state();
            let person = state.people.iter().find(|person| person.id == id);
            let error = person.and_then(|person| state.person_save_error(&person.id));
            self.split.second_mut().set_person(person, error, ctx);
        }
        outcome.changed
    }

    fn sync_detail_changes(&mut self) -> bool {
        let patches = self.split.second_mut().take_patches();
        let mut changed = false;
        for (person_id, patch) in patches {
            let outcome = self
                .context
                .store
                .borrow_mut()
                .dispatch(AppEvent::PatchPerson {
                    person_id: person_id.clone(),
                    patch: patch.clone(),
                });
            if outcome.changed {
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::PatchPerson(person_id, patch));
                changed = true;
            }
        }
        if changed {
            let store = self.context.store.borrow();
            let state = store.state();
            self.split.first_mut().set_rows(state.people.clone());
            self.split.second_mut().set_save_error(
                state
                    .selected_person_id
                    .as_deref()
                    .and_then(|id| state.person_save_error(id)),
            );
            self.observed_version = state.version;
        }
        changed
    }
}

impl TuiNode<AppMsg> for PeopleWorkspace {
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

struct PersonDetailForm {
    root: Flex<AppMsg>,
    person_id: Option<String>,
    patches: PersonPatchSink,
    save_status: SaveStatusLine,
}

impl PersonDetailForm {
    fn new(person: Option<&Person>, save_error: Option<&str>) -> Self {
        let patches = Rc::new(RefCell::new(Vec::new()));
        let save_status = SaveStatusLine::new(save_error);
        Self {
            root: Flex::column().child(
                "form",
                person_detail_form(person, Rc::clone(&patches), save_status.clone()),
                FlexItem::content(),
            ),
            person_id: person.map(|person| person.id.clone()),
            patches,
            save_status,
        }
    }
    fn take_patches(&mut self) -> Vec<(String, PersonPatch)> {
        let Some(id) = self.person_id.clone() else {
            self.patches.borrow_mut().clear();
            return Vec::new();
        };
        self.patches
            .borrow_mut()
            .drain(..)
            .map(|patch| (id.clone(), patch))
            .collect()
    }
    fn set_person(
        &mut self,
        person: Option<&Person>,
        error: Option<&str>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.patches = Rc::new(RefCell::new(Vec::new()));
        self.person_id = person.map(|person| person.id.clone());
        self.save_status = SaveStatusLine::new(error);
        self.root
            .replace(
                "form",
                person_detail_form(person, Rc::clone(&self.patches), self.save_status.clone()),
                FlexItem::content(),
                ctx,
            )
            .expect("person detail form host should contain form child");
    }
    fn set_save_error(&self, error: Option<&str>) {
        self.save_status.set_error(error);
    }
}

impl TuiNode<AppMsg> for PersonDetailForm {
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

fn person_split(context: &AppContext) -> Split<PersonTable, PersonDetailForm> {
    let store = context.store.borrow();
    let state = store.state();
    let selected = state.selected_person_id.as_deref();
    let person = selected.and_then(|id| state.people.iter().find(|person| person.id == id));
    let detail = PersonDetailForm::new(
        person,
        person.and_then(|person| state.person_save_error(&person.id)),
    );
    Split::horizontal(person_table(state.people.clone(), selected), detail)
        .ratio(65, 35)
        .separator(Separator::new())
}

fn person_table(rows: Vec<Person>, selected_id: Option<&str>) -> PersonTable {
    let mut table = DataView::new(rows, |row: &Person| row.id.clone())
        .headers(true)
        .action_bar(true)
        .activation_mode(ActivationMode::OnActivateKey)
        .selection_mode(SelectionMode::Single)
        .selection_trigger(SelectionTrigger::OnNavigate)
        .columns(vec![
            Column::text(
                "name",
                "Person",
                Constraint::Percentage(45),
                |row: &Person| row.name.clone(),
            )
            .sortable(|row| row.name.clone()),
            Column::text(
                "email",
                "Email",
                Constraint::Percentage(40),
                |row: &Person| row.email.clone(),
            ),
            Column::text(
                "active",
                "Active",
                Constraint::Percentage(15),
                |row: &Person| if row.active { "yes" } else { "no" }.to_string(),
            )
            .filter_key(|row| if row.active { "active" } else { "inactive" }.to_string()),
        ]);
    if let Some(id) = selected_id {
        table = table.selected([id.to_string()]);
    }
    table
}

fn person_detail_form(
    person: Option<&Person>,
    patches: PersonPatchSink,
    status: SaveStatusLine,
) -> Flex<AppMsg> {
    let Some(person) = person else {
        return Flex::column().child(
            "empty",
            Paragraph::new("No person selected."),
            FlexItem::fixed(1),
        );
    };
    Flex::column()
        .gap(0)
        .child("save-status", status, FlexItem::fixed(1))
        .child(
            "name",
            TextInput::new()
                .value(person.name.clone())
                .panel("Name")
                .on_edit_end({
                    let patches = Rc::clone(&patches);
                    move |value| {
                        patches.borrow_mut().push(PersonPatch::Name(value));
                        AppMsg::Noop
                    }
                }),
            FlexItem::fixed(3),
        )
        .child(
            "email",
            TextInput::new()
                .value(person.email.clone())
                .panel("Email")
                .on_edit_end({
                    let patches = Rc::clone(&patches);
                    move |value| {
                        patches.borrow_mut().push(PersonPatch::Email(value));
                        AppMsg::Noop
                    }
                }),
            FlexItem::fixed(3),
        )
        .child(
            "active",
            dropdown_single(
                "Active",
                active_choices(),
                if person.active { "true" } else { "false" },
                move |id| patches.borrow_mut().push(PersonPatch::Active(id == "true")),
            ),
            FlexItem::fixed(3),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::tests::{rendered_text, test_context},
        domain::{PersonField, SaveTarget, WorkspaceSnapshot},
    };
    use tuicore::{Key, Tab, Tabs};

    #[test]
    fn focused_detail_input_receives_tab_navigation_characters_before_ancestor_tabs() {
        let person = Person {
            id: "person-1".into(),
            name: "Ada".into(),
            email: "ada@example.com".into(),
            active: true,
        };
        let detail = PersonDetailForm::new(Some(&person), None);
        let patches = Rc::clone(&detail.patches);
        let mut tabs = Tabs::new(vec![
            Tab::new("Details", detail),
            Tab::text("Other", "Other tab"),
        ]);
        let mut layout = LayoutCtx::new();
        tabs.layout(Rect::new(0, 0, 80, 24), &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.id.as_str() == "input")
            .expect("detail name input should be focusable")
            .clone();
        tabs.dispatch_focus(&target, true, &mut FocusCtx::default());
        let route = EventRoute::new(target.path);

        for key in [Key::Enter, Key::Char('['), Key::Char(']'), Key::Enter] {
            let outcome =
                tabs.dispatch_event(&route, &TuiEvent::Key(key.into()), &mut EventCtx::default());
            assert_eq!(outcome, EventOutcome::Handled);
            assert_eq!(tabs.selected_index(), 0);
        }

        assert!(matches!(
            patches.borrow().as_slice(),
            [PersonPatch::Name(value)] if value == "Ada[]"
        ));
    }

    #[test]
    fn save_status_reconciliation_keeps_pending_detail_changes() {
        let person = Person {
            id: "person-1".into(),
            name: "Ada".into(),
            email: "ada@example.com".into(),
            active: true,
        };
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![person],
            projects: vec![],
            tags: vec![],
        });
        let mut workspace = PeopleWorkspace::new(context);
        workspace
            .split
            .second_mut()
            .patches
            .borrow_mut()
            .push(PersonPatch::Name("Ada Lovelace".into()));
        store.borrow_mut().dispatch(AppEvent::SaveCompleted {
            target: SaveTarget::person("person-1".into(), PersonField::Email),
            error: Some("offline".into()),
        });
        let area = Rect::new(0, 0, 100, 30);
        workspace.layout(area, &mut LayoutCtx::new());
        let patches = workspace.split.second_mut().take_patches();
        assert!(
            matches!(patches.as_slice(), [(id, PersonPatch::Name(name))] if id == "person-1" && name == "Ada Lovelace")
        );
        assert!(rendered_text(&workspace, area).contains("Save failed"));
    }
}
