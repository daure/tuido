use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
};
use tuicore::{
    ActivationMode, AnimationSettings, ChildKey, Column, DataView, DataViewTypedEvent, Dialog,
    DialogHost, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusTarget,
    LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx,
    SelectionMode, SelectionTrigger, TextInput, TextareaInput, TickResult, TuiEvent, TuiNode,
};

use super::ManagementDialogKind;
use super::common::{ManagementPane, active_choices, dropdown_single, management_empty_state};
use crate::{
    app::{AppContext, AppMsg},
    app_keymap::{self, keys},
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
        .close_on_unfocus_from_descendants(true)
        .on_close(|_| AppMsg::CloseDialog)
        .host(PeopleWorkspace::new(context))
}

pub(crate) struct PeopleWorkspace {
    context: AppContext,
    split: ManagementPane<PersonTable, PersonDetailForm>,
    observed_version: u64,
    observed_external_refresh_version: u64,
    table_focused: bool,
    detail_draft_protected: bool,
}

impl PeopleWorkspace {
    fn new(context: AppContext) -> Self {
        let split = person_split(&context);
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
        let rows = state.people.clone();
        let has_people = !rows.is_empty();
        let selected_id = state.selected_person_id.clone();
        drop(store);
        self.split.first_mut().set_rows(rows);
        self.split
            .first_mut()
            .set_empty_state(management_empty_state(
                ManagementDialogKind::People,
                has_people,
            ));
        if let Some(id) = selected_id.as_ref() {
            self.split.first_mut().highlight_id(id);
            self.split.first_mut().select_id(id.clone());
        }
        self.split.first_mut().take_events();
        let visible_id = self.split.first().highlighted_id();
        let (person, save_error) = {
            let store = self.context.store.borrow();
            let state = store.state();
            let person = visible_id
                .as_deref()
                .and_then(|id| state.people.iter().find(|person| person.id == id))
                .cloned();
            let error = visible_id
                .as_deref()
                .and_then(|id| state.person_save_error(id))
                .map(str::to_string);
            (person, error)
        };
        if self.split.second().person_id.as_deref() != visible_id.as_deref()
            || (external_refresh && !protect_detail)
        {
            self.split.second_mut().set_person(
                person.as_ref(),
                save_error.as_deref(),
                &mut EventCtx::default(),
            );
        } else {
            self.split
                .second_mut()
                .set_save_error(save_error.as_deref());
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
                    selected_changed |= self.select_person(id, ctx);
                    focus_detail |= matches!(event, DataViewTypedEvent::Activated { .. });
                }
                DataViewTypedEvent::HighlightChanged { row_id: None } => {
                    self.split.second_mut().set_person(None, None, ctx);
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

    fn select_person(&mut self, id: &str, ctx: &mut EventCtx<AppMsg>) -> bool {
        let outcome = self
            .context
            .store
            .borrow_mut()
            .dispatch(AppEvent::SelectPerson(id.to_string()));
        let store = self.context.store.borrow();
        let state = store.state();
        let person = state.people.iter().find(|person| person.id == id).cloned();
        let error = person
            .as_ref()
            .and_then(|person| state.person_save_error(&person.id))
            .map(str::to_string);
        drop(store);
        self.split
            .second_mut()
            .set_person(person.as_ref(), error.as_deref(), ctx);
        let visibility_changed = self.split.set_detail_visible(person.is_some());
        outcome.changed || visibility_changed
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
            self.detail_draft_protected = false;
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
                kind: ManagementDialogKind::People,
                entity_id,
            });
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        outcome
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

fn person_split(context: &AppContext) -> ManagementPane<PersonTable, PersonDetailForm> {
    let store = context.store.borrow();
    let state = store.state();
    let selected = state.selected_person_id.as_deref();
    let person = selected.and_then(|id| state.people.iter().find(|person| person.id == id));
    let detail = PersonDetailForm::new(
        person,
        person.and_then(|person| state.person_save_error(&person.id)),
    );
    ManagementPane::new(
        person_table(state.people.clone(), selected),
        detail,
        ManagementDialogKind::People,
    )
    .detail_visible(person.is_some())
}

fn person_table(rows: Vec<Person>, selected_id: Option<&str>) -> PersonTable {
    let has_people = !rows.is_empty();
    let mut table = DataView::new(rows, |row: &Person| row.id.clone())
        .empty_state(management_empty_state(
            ManagementDialogKind::People,
            has_people,
        ))
        .headers(true)
        .action_bar(true)
        .filter_controls(false)
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
        return Flex::column();
    };
    Flex::column()
        .gap(0)
        .child("save-status", status, FlexItem::content())
        .child(
            "name",
            TextInput::new()
                .value(person.name.clone())
                .placeholder("Person name")
                .panel("Name")
                .hotkey(keys::PERSON_NAME_FIELD.hotkey())
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
                .placeholder("Email address")
                .panel("Email")
                .hotkey(keys::PERSON_EMAIL_FIELD.hotkey())
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
            "about",
            TextareaInput::new()
                .value(person.about.clone())
                .placeholder("About this person")
                .panel("About")
                .hotkey(keys::PERSON_ABOUT_FIELD.hotkey())
                .editor_hotkey(keys::PERSON_ABOUT_EDITOR.hotkey())
                .on_edit_end({
                    let patches = Rc::clone(&patches);
                    move |value| {
                        patches.borrow_mut().push(PersonPatch::About(value));
                        AppMsg::Noop
                    }
                })
                .min_rows(2)
                .max_rows(6),
            FlexItem::content(),
        )
        .child(
            "active",
            dropdown_single(
                "Active",
                "Select active status",
                active_choices(),
                if person.active { "true" } else { "false" },
                move |id| patches.borrow_mut().push(PersonPatch::Active(id == "true")),
            )
            .hotkey(keys::PERSON_ACTIVE_FIELD.hotkey()),
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
    use tuicore::{FocusRequest, Key, KeyEvent, KeyModifiers, Tab, Tabs};

    #[test]
    fn focused_detail_input_receives_tab_navigation_characters_before_ancestor_tabs() {
        let person = Person {
            id: "person-1".into(),
            name: "Ada".into(),
            email: "ada@example.com".into(),
            about: String::new(),
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
            about: String::new(),
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

    #[test]
    fn external_refresh_repopulates_selected_person_detail_once_draft_is_safe() {
        let person = Person::new("person-1".into(), "Ada".into(), String::new());
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![person.clone()],
            projects: vec![],
            tags: vec![],
        });
        let mut workspace = PeopleWorkspace::new(context);
        workspace.detail_draft_protected = true;
        let mut refreshed = person;
        refreshed.name = "Ada Lovelace".into();
        store.borrow_mut().dispatch(AppEvent::WorkspaceRefreshed {
            snapshot: WorkspaceSnapshot {
                tasks: vec![],
                people: vec![refreshed],
                projects: vec![],
                tags: vec![],
            },
            revision: 1,
            entity_revisions: std::collections::HashMap::new(),
        });
        let area = Rect::new(0, 0, 100, 30);

        workspace.layout(area, &mut LayoutCtx::new());
        assert!(rendered_text(workspace.split.second(), area).contains("Ada"));
        assert!(!rendered_text(workspace.split.second(), area).contains("Ada Lovelace"));

        workspace.detail_draft_protected = false;
        workspace.layout(area, &mut LayoutCtx::new());
        assert!(rendered_text(workspace.split.second(), area).contains("Ada Lovelace"));
        assert_eq!(
            workspace.split.first().highlighted_id().as_deref(),
            Some("person-1")
        );
    }

    #[test]
    fn delete_hotkey_requests_confirmation_for_selected_person() {
        let person = Person::new("person-1".into(), "Ada".into(), String::new());
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![person],
            projects: vec![],
            tags: vec![],
        });
        let mut workspace = PeopleWorkspace::new(context);
        workspace.table_focused = true;
        let mut ctx = EventCtx::default();

        let outcome = workspace.handle_workspace_event(
            EventOutcome::Ignored,
            &TuiEvent::Key(tuicore::KeyEvent {
                code: Key::Char('x'),
                modifiers: tuicore::KeyModifiers::CONTROL,
            }),
            &mut ctx,
        );

        assert!(outcome.handled());
        assert!(matches!(
            ctx.messages(),
            [AppMsg::OpenDeleteManagement {
                kind: ManagementDialogKind::People,
                entity_id,
            }] if entity_id == "person-1"
        ));

        let mut plain_x_ctx = EventCtx::default();
        let plain_x = workspace.handle_workspace_event(
            EventOutcome::Ignored,
            &TuiEvent::Key(Key::Char('x').into()),
            &mut plain_x_ctx,
        );
        assert!(!plain_x.handled());
        assert!(plain_x_ctx.messages().is_empty());
    }

    #[test]
    fn new_hotkey_opens_person_creation_dialog() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![],
            projects: vec![],
            tags: vec![],
        });
        let mut dialog = dialog(context);
        let mut layout = LayoutCtx::new();
        dialog.layout(Rect::new(0, 0, 80, 24), &mut layout);
        let create = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().iter().any(|key| key.as_str() == "new"))
            .expect("top-right New button should be focusable")
            .clone();
        let mut ctx = EventCtx::default();

        let outcome = dialog.dispatch_event(
            &EventRoute::new(create.path),
            &TuiEvent::Key(Key::Char('n').into()),
            &mut ctx,
        );

        assert!(outcome.handled());
        assert!(matches!(
            ctx.messages(),
            [AppMsg::OpenCreateManagement(ManagementDialogKind::People)]
        ));
    }

    #[test]
    fn escape_from_person_detail_focuses_table_before_closing_dialog() {
        let person = Person::new("person-1".into(), "Ada".into(), "ada@example.com".into());
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![person],
            projects: vec![],
            tags: vec![],
        });
        let mut dialog = dialog(context);
        let mut layout = LayoutCtx::new();
        dialog.layout(Rect::new(0, 0, 100, 30), &mut layout);
        let name = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().iter().any(|key| key.as_str() == "name"))
            .expect("person name should be focusable")
            .clone();
        let table = layout
            .focus_targets()
            .iter()
            .find(|target| target.id.as_str() == "data-view")
            .expect("people table should be focusable")
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
                &EventRoute::new(name.path.clone()),
                &TuiEvent::Key(key),
                &mut ctx,
            );

            assert!(outcome.handled());
            assert!(ctx.messages().is_empty());
            assert!(matches!(ctx.focus_request(), Some(FocusRequest::Path(_))));

            let mut close_ctx = EventCtx::default();
            let close = dialog.dispatch_event(
                &EventRoute::new(table.path.clone()),
                &TuiEvent::Key(key),
                &mut close_ctx,
            );
            assert!(close.handled());
            assert!(matches!(close_ctx.messages(), [AppMsg::CloseDialog]));
        }
    }

    #[test]
    fn person_detail_controls_register_requested_hotkeys() {
        let person = Person::new("person-1".into(), "Ada".into(), "ada@example.com".into());
        let detail = PersonDetailForm::new(Some(&person), None);
        let mut layout = LayoutCtx::new();
        let mut detail = detail;
        detail.layout(Rect::new(0, 0, 80, 20), &mut layout);

        for hotkey in [
            keys::PERSON_NAME_FIELD.hotkey(),
            keys::PERSON_EMAIL_FIELD.hotkey(),
            keys::PERSON_ABOUT_FIELD.hotkey(),
            keys::PERSON_ABOUT_EDITOR.hotkey(),
            keys::PERSON_ACTIVE_FIELD.hotkey(),
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
    }

    #[test]
    fn people_table_disables_filter_mode() {
        let mut table = person_table(Vec::new(), None);

        let outcome = table.on_key(Key::Char('f'), Rect::new(0, 0, 80, 20));

        assert!(!outcome.handled);
        assert!(!outcome.changed);
        assert!(table.transform_state().filters.is_empty());
    }

    #[test]
    fn newly_created_person_is_selected_and_shown_in_detail() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![Person::new("person-1".into(), "Ada".into(), String::new())],
            projects: vec![],
            tags: vec![],
        });
        let mut workspace = PeopleWorkspace::new(context);
        store
            .borrow_mut()
            .dispatch(AppEvent::PersonCreated(Person::new(
                "person-2".into(),
                "Grace".into(),
                String::new(),
            )));

        workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());

        assert_eq!(
            workspace.split.first().highlighted_id().as_deref(),
            Some("person-2")
        );
        assert_eq!(
            workspace.split.first().selected_id().as_deref(),
            Some("person-2")
        );
        assert_eq!(
            workspace.split.second().person_id.as_deref(),
            Some("person-2")
        );
    }

    #[test]
    fn search_without_matches_hides_detail_and_clear_restores_it() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![Person::new("person-1".into(), "Ada".into(), String::new())],
            projects: vec![],
            tags: vec![],
        });
        let mut workspace = PeopleWorkspace::new(context);
        let mut ctx = EventCtx::default();

        workspace.split.first_mut().set_search_query("Grace");
        workspace.sync_table_events(&mut ctx);
        assert_eq!(workspace.split.first().highlighted_id(), None);
        assert_eq!(workspace.split.second().person_id, None);
        assert!(!workspace.split.is_detail_visible());
        let area = Rect::new(0, 0, 100, 30);
        workspace.layout(area, &mut LayoutCtx::new());
        assert!(rendered_text(&workspace, area).contains("No people match your search"));

        workspace.split.first_mut().clear_search();
        workspace.sync_table_events(&mut ctx);
        assert_eq!(
            workspace.split.first().highlighted_id().as_deref(),
            Some("person-1")
        );
        assert_eq!(
            workspace.split.second().person_id.as_deref(),
            Some("person-1")
        );
        assert!(workspace.split.is_detail_visible());
    }

    #[test]
    fn delete_hotkey_does_nothing_when_search_has_no_highlighted_person() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![Person::new("person-1".into(), "Ada".into(), String::new())],
            projects: vec![],
            tags: vec![],
        });
        let mut workspace = PeopleWorkspace::new(context);
        workspace.table_focused = true;
        workspace.split.first_mut().set_search_query("Grace");
        workspace.sync_table_events(&mut EventCtx::default());
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
    fn search_match_after_no_match_shows_different_person_detail() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![
                Person::new("person-1".into(), "Ada".into(), String::new()),
                Person::new("person-2".into(), "Grace".into(), String::new()),
            ],
            projects: vec![],
            tags: vec![],
        });
        let mut workspace = PeopleWorkspace::new(context);
        let mut ctx = EventCtx::default();

        workspace.split.first_mut().set_search_query("nobody");
        workspace.sync_table_events(&mut ctx);
        workspace.split.first_mut().set_search_query("Grace");
        workspace.sync_table_events(&mut ctx);

        assert_eq!(
            workspace.split.first().highlighted_id().as_deref(),
            Some("person-2")
        );
        assert_eq!(
            workspace.split.second().person_id.as_deref(),
            Some("person-2")
        );
        assert!(workspace.split.is_detail_visible());
    }

    #[test]
    fn deleting_final_person_hides_detail() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![Person::new("person-1".into(), "Ada".into(), String::new())],
            projects: vec![],
            tags: vec![],
        });
        let mut workspace = PeopleWorkspace::new(context);
        store
            .borrow_mut()
            .dispatch(AppEvent::PersonDeleted("person-1".into()));

        workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());

        assert_eq!(workspace.split.first().highlighted_id(), None);
        assert_eq!(workspace.split.second().person_id, None);
        assert!(!workspace.split.is_detail_visible());
    }
}
