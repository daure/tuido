use std::{cell::RefCell, error::Error, rc::Rc, time::Duration};

use crate::app_keymap::{self, keys};
use crate::calendar::CalendarWorkspace;
use crate::create_management_dialog::{CreateManagementDialog, ManagementEntityDraft};
use crate::create_task_dialog::{CreateTaskDialog, CreateTaskDraft};
use crate::domain::{
    AppEvent, AppState, Person, Project, Tag, Task, TaskPatch, TaskPriority, TaskSize, TaskState,
    reduce_app_state,
};
use crate::persistence_coordinator::{AppStore, PersistenceCommand, PersistenceCoordinator};
use crate::service::TuidoService;
use crate::snooze::{SnoozeDialog, local_now};
use crate::storage::Storage;
use crate::ui::management::{ManagementDialogKind, people, projects, tags};
use crate::ui::save_status::SaveStatusLine;
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
};
use time::{Date, PrimitiveDateTime};
use tuicore::{
    ActivationMode, AnimationSettings, AxisProposal, Button, CellContext, ChildKey, ChipColorRole,
    Column, ConfirmationDialog, ConfirmationDialogOutcome, CrossAlign, DataView,
    DataViewTypedEvent, DatePickerDropdown, DateTimePickerDropdown, Dialog, DialogBackdrop,
    DialogHost, DialogLayer, Dropdown, DropdownCommitMode, DropdownSearchMode, EventCtx,
    EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusId, FocusRequest, FocusTarget,
    HotkeyLabelMode, KeySpec, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint,
    LifecycleCtx, Menu, MenuItem, MenuSearchMode, Paragraph, RenderCtx, SelectedTag, SelectionMode,
    SelectionTrigger, Split, StatusBar, StatusBarMenuItem, Store, Tab, Tabs, TabsVariant, TagInput,
    TagInputEvent, TextInput, TextareaInput, TickResult, TreeApp, TuiEvent, TuiNode,
    WeatherProviderConfig,
};
use uuid::Uuid;

const PEOPLE_MENU_ID: &str = "people";
const PROJECTS_MENU_ID: &str = "projects";
const TAGS_MENU_ID: &str = "tags";

#[derive(Debug)]
pub(crate) enum AppMsg {
    Noop,
    OpenManagementDialog(ManagementDialogKind),
    OpenCreateManagement(ManagementDialogKind),
    CreateManagementSubmitted(ManagementEntityDraft),
    OpenDeleteManagement {
        kind: ManagementDialogKind,
        entity_id: String,
    },
    DeleteManagementConfirmed {
        kind: ManagementDialogKind,
        entity_id: String,
    },
    OpenCreateTask,
    CreateTaskSubmitted(CreateTaskDraft),
    OpenDeleteTask(String),
    DeleteTaskConfirmed(String),
    OpenTaskSnooze(String),
    SnoozeTask {
        task_id: String,
        until: PrimitiveDateTime,
        remember_custom: Option<PrimitiveDateTime>,
    },
    UnsnoozeTask(String),
    CloseManagementOverlay,
    CloseDialog,
}

pub fn run() -> Result<(), Box<dyn Error>> {
    match crate::paths::ui_config_source()? {
        crate::paths::UiConfigSource::Legacy => tuicore::try_init()?,
        crate::paths::UiConfigSource::Directory(config_dir) => {
            tuicore::try_init_from_dir(config_dir)?
        }
        crate::paths::UiConfigSource::Defaults => {
            tuicore::set_theme(tuicore::Theme::default());
            tuicore::set_keybindings(tuicore::KeyBindings::default());
            tuicore::set_preset(tuicore::Preset::default());
        }
    }
    app_keymap::try_init()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let storage = runtime.block_on(Storage::connect_from_env())?;
    runtime.block_on(storage.migrate())?;
    let last_custom_snooze = runtime.block_on(storage.load_last_custom_snooze())?;
    let service = TuidoService::from_storage(&storage);
    let workspace = runtime.block_on(service.consistent_workspace())?;
    let mut app_state = AppState::from_snapshot(workspace.snapshot);
    app_state.workspace_revision = workspace.revision;
    app_state.entity_revisions = workspace.entity_revisions;
    app_state.last_custom_snooze = last_custom_snooze;
    let store = Rc::new(RefCell::new(Store::new(
        app_state,
        reduce_app_state as fn(&mut AppState, AppEvent) -> tuicore::DispatchOutcome,
    )));
    let coordinator = Rc::new(RefCell::new(PersistenceCoordinator::new(
        Rc::clone(&store),
        storage.pool(),
        storage.dialect(),
        runtime.handle().clone(),
        storage.notification_url(),
    )));
    let run_result = TreeApp::new(App::new(store, Rc::clone(&coordinator)))
        .initial_focus(FocusRequest::Target(FocusId::new("data-view")))
        .on_message(|app, message, ctx| match message {
            AppMsg::Noop => {}
            AppMsg::OpenManagementDialog(kind) => app.open_management_dialog(kind, ctx),
            AppMsg::OpenCreateManagement(kind) => app.open_create_management_dialog(kind, ctx),
            AppMsg::CreateManagementSubmitted(draft) => app.submit_create_management(draft, ctx),
            AppMsg::OpenDeleteManagement { kind, entity_id } => {
                app.open_delete_management_dialog(kind, &entity_id, ctx)
            }
            AppMsg::DeleteManagementConfirmed { kind, entity_id } => {
                app.delete_management(kind, &entity_id, ctx)
            }
            AppMsg::OpenCreateTask => app.open_create_task_dialog(ctx),
            AppMsg::CreateTaskSubmitted(draft) => app.submit_create_task(draft, ctx),
            AppMsg::OpenDeleteTask(task_id) => app.open_delete_task_dialog(&task_id, ctx),
            AppMsg::DeleteTaskConfirmed(task_id) => app.delete_task(task_id, ctx),
            AppMsg::OpenTaskSnooze(task_id) => app.open_task_snooze_dialog(&task_id, ctx),
            AppMsg::SnoozeTask {
                task_id,
                until,
                remember_custom,
            } => app.snooze_task(task_id, until, remember_custom, ctx),
            AppMsg::UnsnoozeTask(task_id) => app.unsnooze_task(task_id, ctx),
            AppMsg::CloseManagementOverlay => app.close_management_overlay(ctx),
            AppMsg::CloseDialog => app.close_dialog(ctx),
        })
        .run();
    let drained = coordinator.borrow_mut().drain(Duration::from_secs(2));
    run_result?;
    if !drained {
        return Err("timed out draining pending persistence commands".into());
    }
    Ok(())
}

type PrimaryDialogLayer = DialogLayer<Flex<AppMsg>, AppDialog>;
type AppDialogLayers = DialogLayer<PrimaryDialogLayer, AppDialog>;

struct App {
    root: AppDialogLayers,
    context: AppContext,
}

impl App {
    fn new(store: AppStore, coordinator: Rc<RefCell<PersistenceCoordinator>>) -> Self {
        let context = AppContext { store, coordinator };
        let tabs = Tabs::new(vec![
            Tab::new("Tasks", TaskWorkspace::new(context.clone()))
                .hotkey(keys::APP_TASKS_TAB.hotkey()),
            Tab::new("Calendar", CalendarWorkspace::new(context.clone()))
                .hotkey(keys::APP_CALENDAR_TAB.hotkey()),
        ])
        .selected(0)
        .variant(TabsVariant::Underline)
        .bordered(true);

        let root = Flex::column().child("tabs", tabs, FlexItem::fill(1)).child(
            "footer",
            StatusBar::new()
                .menu_items([
                    StatusBarMenuItem::Custom {
                        id: PEOPLE_MENU_ID,
                        label: "People",
                    },
                    StatusBarMenuItem::Custom {
                        id: PROJECTS_MENU_ID,
                        label: "Projects",
                    },
                    StatusBarMenuItem::Custom {
                        id: TAGS_MENU_ID,
                        label: "Tags",
                    },
                    StatusBarMenuItem::Theme,
                ])
                .weather_provider(WeatherProviderConfig::new().enabled(false))
                .on_custom_menu_item(|id| match id {
                    PEOPLE_MENU_ID => AppMsg::OpenManagementDialog(ManagementDialogKind::People),
                    PROJECTS_MENU_ID => {
                        AppMsg::OpenManagementDialog(ManagementDialogKind::Projects)
                    }
                    TAGS_MENU_ID => AppMsg::OpenManagementDialog(ManagementDialogKind::Tags),
                    _ => AppMsg::OpenManagementDialog(ManagementDialogKind::People),
                }),
            FlexItem::fixed(1),
        );

        let primary = DialogLayer::new(root, empty_app_dialog())
            .active(false)
            .layer_percent(80)
            .layer_cross_percent(80)
            .backdrop(DialogBackdrop::dim().amount(0.5));
        Self {
            root: DialogLayer::new(primary, empty_app_dialog())
                .active(false)
                .layer_percent(60)
                .layer_cross_percent(50)
                .base_overlays_visible(true)
                .backdrop(DialogBackdrop::dim().amount(0.5)),
            context,
        }
    }

    fn primary_dialog(&mut self) -> &mut PrimaryDialogLayer {
        self.root.base_mut()
    }

    fn open_management_dialog(&mut self, kind: ManagementDialogKind, ctx: &mut EventCtx<AppMsg>) {
        self.root.set_active_with_context(false, ctx);
        let dialog = management_dialog(self.context.clone(), kind);
        let primary = self.primary_dialog();
        primary.replace_layer(dialog, ctx);
        primary.set_layer_percent(80);
        primary.set_layer_cross_percent(80);
        primary.set_fit_content(false);
        primary.set_active_immediate_with_context(true, ctx);
    }

    fn open_create_management_dialog(
        &mut self,
        kind: ManagementDialogKind,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.root
            .replace_layer(create_management_dialog_host(kind), ctx);
        self.root.set_layer_percent(60);
        self.root.set_layer_cross_percent(50);
        self.root.set_fit_content(true);
        self.root.set_active_with_context(true, ctx);
    }

    fn submit_create_management(
        &mut self,
        draft: ManagementEntityDraft,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        match draft {
            ManagementEntityDraft::Person { name, email } => {
                if name.trim().is_empty() {
                    notify_required(
                        ctx,
                        "Person name required",
                        "Enter a name before creating the person.",
                    );
                    return;
                }
                let person = Person::new(Uuid::new_v4().to_string(), name, email);
                self.context
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::PersonCreated(person.clone()));
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::CreatePerson(person));
            }
            ManagementEntityDraft::Project {
                key,
                name,
                description,
            } => {
                if key.trim().is_empty() || name.trim().is_empty() {
                    notify_required(
                        ctx,
                        "Project key and name required",
                        "Enter both a key and name before creating the project.",
                    );
                    return;
                }
                let project = Project::new(Uuid::new_v4().to_string(), key, name, description);
                self.context
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::ProjectCreated(project.clone()));
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::CreateProject(project));
            }
            ManagementEntityDraft::Tag { label } => {
                if label.trim().is_empty() {
                    notify_required(
                        ctx,
                        "Tag label required",
                        "Enter a label before creating the tag.",
                    );
                    return;
                }
                let tag = Tag::new(Uuid::new_v4().to_string(), label);
                self.context
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::TagCreated(tag.clone()));
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::CreateTag(tag));
            }
        }
        self.close_management_overlay(ctx);
    }

    fn open_delete_management_dialog(
        &mut self,
        kind: ManagementDialogKind,
        entity_id: &str,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let label = {
            let store = self.context.store.borrow();
            let state = store.state();
            match kind {
                ManagementDialogKind::People => state
                    .people
                    .iter()
                    .find(|person| person.id == entity_id)
                    .map(|person| person.name.clone()),
                ManagementDialogKind::Projects => state
                    .projects
                    .iter()
                    .find(|project| project.id == entity_id)
                    .map(|project| project.name.clone()),
                ManagementDialogKind::Tags => state
                    .tags
                    .iter()
                    .find(|tag| tag.id == entity_id)
                    .map(|tag| tag.label.clone()),
            }
        };
        let Some(label) = label else {
            return;
        };
        self.root.replace_layer(
            delete_management_dialog(kind, entity_id.to_string(), &label),
            ctx,
        );
        self.root.set_fit_content(true);
        self.root.set_active_with_context(true, ctx);
    }

    fn delete_management(
        &mut self,
        kind: ManagementDialogKind,
        entity_id: &str,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        match kind {
            ManagementDialogKind::People => {
                let deletion = self
                    .context
                    .store
                    .borrow()
                    .state()
                    .person_deletion(entity_id);
                let Some(deletion) = deletion else { return };
                self.context
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::PersonDeleted(deletion.person.id.clone()));
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::DeletePerson(deletion));
            }
            ManagementDialogKind::Projects => {
                let deletion = self
                    .context
                    .store
                    .borrow()
                    .state()
                    .project_deletion(entity_id);
                let Some(deletion) = deletion else { return };
                self.context
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::ProjectDeleted(deletion.project.id.clone()));
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::DeleteProject(deletion));
            }
            ManagementDialogKind::Tags => {
                let deletion = self.context.store.borrow().state().tag_deletion(entity_id);
                let Some(deletion) = deletion else { return };
                self.context
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::TagDeleted(deletion.tag.id.clone()));
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::DeleteTag(deletion));
            }
        }
        self.close_management_overlay(ctx);
    }

    fn open_create_task_dialog(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let primary = self.primary_dialog();
        primary.replace_layer(create_task_dialog_host(), ctx);
        primary.set_layer_percent(60);
        primary.set_layer_cross_percent(50);
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
    }

    fn submit_create_task(&mut self, draft: CreateTaskDraft, ctx: &mut EventCtx<AppMsg>) {
        let title = draft.title.trim();
        if title.is_empty() {
            ctx.notify(tuicore::Notification::warning(
                "Task title required",
                "Enter a title before creating the task.",
            ));
            return;
        }

        let task = Task::quick_capture(
            Uuid::new_v4().to_string(),
            title.to_string(),
            draft.description,
            draft.size,
        );
        self.context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(task.clone()));
        self.context
            .coordinator
            .borrow_mut()
            .submit(PersistenceCommand::CreateTask(task));
        self.close_dialog(ctx);
    }

    fn open_delete_task_dialog(&mut self, task_id: &str, ctx: &mut EventCtx<AppMsg>) {
        let Some(task) = self.task(task_id) else {
            return;
        };
        let primary = self.primary_dialog();
        primary.replace_layer(delete_task_dialog(&task), ctx);
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
    }

    fn delete_task(&mut self, task_id: String, ctx: &mut EventCtx<AppMsg>) {
        let task = {
            let store = self.context.store.borrow();
            let state = store.state();
            state.tasks.iter().find(|task| task.id == task_id).cloned()
        };
        let Some(task) = task else {
            self.close_dialog(ctx);
            return;
        };
        self.context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskDeleted(task_id.clone()));
        self.context
            .coordinator
            .borrow_mut()
            .submit(PersistenceCommand::DeleteTask(task));
        self.close_dialog(ctx);
    }

    fn open_task_snooze_dialog(&mut self, task_id: &str, ctx: &mut EventCtx<AppMsg>) {
        let Some(task) = self.task(task_id) else {
            return;
        };
        let now = match local_now() {
            Ok(now) => now,
            Err(error) => {
                ctx.notify(tuicore::Notification::error(
                    "Local time unavailable",
                    format!("Cannot open snooze options: {error}"),
                ));
                return;
            }
        };
        let last_custom = self.context.store.borrow().state().last_custom_snooze;
        let primary = self.primary_dialog();
        primary.replace_layer(
            AppDialog::Snooze(Box::new(SnoozeDialog::new(
                task.id,
                now,
                last_custom,
                task.state == TaskState::Snoozed,
            ))),
            ctx,
        );
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
    }

    fn snooze_task(
        &mut self,
        task_id: String,
        until: PrimitiveDateTime,
        remember_custom: Option<PrimitiveDateTime>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let patch = TaskPatch::Snooze {
            until,
            remember_custom,
        };
        let outcome = self
            .context
            .store
            .borrow_mut()
            .dispatch(AppEvent::PatchTask {
                task_id: task_id.clone(),
                patch: patch.clone(),
            });
        if outcome.changed {
            self.context
                .coordinator
                .borrow_mut()
                .submit(PersistenceCommand::PatchTask(task_id, patch));
        }
        self.close_dialog(ctx);
    }

    fn unsnooze_task(&mut self, task_id: String, ctx: &mut EventCtx<AppMsg>) {
        let patch = TaskPatch::Unsnooze;
        let outcome = self
            .context
            .store
            .borrow_mut()
            .dispatch(AppEvent::PatchTask {
                task_id: task_id.clone(),
                patch: patch.clone(),
            });
        if outcome.changed {
            self.context
                .coordinator
                .borrow_mut()
                .submit(PersistenceCommand::PatchTask(task_id, patch));
        }
        self.close_dialog(ctx);
    }

    fn task(&self, task_id: &str) -> Option<Task> {
        let store = self.context.store.borrow();
        store
            .state()
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .cloned()
    }

    fn close_dialog(&mut self, ctx: &mut EventCtx<AppMsg>) {
        self.root.set_active_with_context(false, ctx);
        self.primary_dialog().set_active_with_context(false, ctx);
    }

    fn close_management_overlay(&mut self, ctx: &mut EventCtx<AppMsg>) {
        self.root.set_active_with_context(false, ctx);
    }
}

impl TuiNode<AppMsg> for App {
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
        let mut result = self.root.tick(dt, settings);
        if self.context.coordinator.borrow_mut().poll() {
            result = result.merge(TickResult {
                changed: true,
                layout: true,
                active: false,
                next_tick: None,
            });
        }
        let delay = if self.context.coordinator.borrow().has_pending() {
            50
        } else {
            500
        };
        result = result.merge(TickResult::scheduled_after(Duration::from_millis(delay)));
        result
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

type TaskRow = Task;
type TaskTable = DataView<TaskRow, String>;
type TaskDetail = TaskDetailForm;
type TaskPane = Split<TaskTable, TaskDetail>;
type TaskWorkspaceLayout = Split<Flex<AppMsg>, TaskPane>;
type TaskViewChange = Rc<RefCell<Option<TaskView>>>;
type ActiveTaskView = Rc<RefCell<TaskView>>;
type VisibleTaskSelection = Rc<RefCell<Option<String>>>;
type PatchSink = Rc<RefCell<Vec<TaskPatch>>>;
#[derive(Clone)]
pub(crate) struct AppContext {
    pub(crate) store: AppStore,
    pub(crate) coordinator: Rc<RefCell<PersistenceCoordinator>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TaskView {
    Todo,
    Snoozed,
    InProgress,
    Archived,
    All,
}

impl TaskView {
    const OPTIONS: [Self; 5] = [
        Self::All,
        Self::Todo,
        Self::Snoozed,
        Self::InProgress,
        Self::Archived,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Todo => "Todo",
            Self::Snoozed => "Snoozed",
            Self::InProgress => "In progress",
            Self::Archived => "Archived",
            Self::All => "All",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Todo => "",
            Self::Snoozed => "󰒲",
            Self::InProgress => "",
            Self::Archived => "",
            Self::All => "",
        }
    }

    fn menu_label(self) -> String {
        format!("{} {}", self.icon(), self.label())
    }

    fn contains(self, task: &Task) -> bool {
        match self {
            Self::Todo => task.state == TaskState::Todo,
            Self::Snoozed => task.state == TaskState::Snoozed,
            Self::InProgress => task.state == TaskState::InProgress,
            Self::Archived => matches!(task.state, TaskState::Done | TaskState::Rejected),
            Self::All => !matches!(task.state, TaskState::Done | TaskState::Rejected),
        }
    }
}

const TASK_VIEW_MENU_TRIGGER: &str = "trigger";
const TASK_VIEW_MENU_PANEL: &str = "menu";

struct TaskViewMenu {
    trigger: Button<AppMsg>,
    menu: Menu<TaskView>,
    pending_view: TaskViewChange,
    active_view: ActiveTaskView,
}

impl TaskViewMenu {
    fn new(pending_view: TaskViewChange, active_view: ActiveTaskView) -> Self {
        let selected = *active_view.borrow();
        let hotkey = keys::TASK_VIEW_MENU.hotkey();
        let trigger = Button::new(selected.menu_label())
            .hotkey(hotkey.clone())
            .hotkey_label_mode(HotkeyLabelMode::Inline);
        let menu = Menu::new(TaskView::OPTIONS.map(|view| MenuItem::new(view, view.menu_label())))
            .search_mode(MenuSearchMode::Fuzzy)
            .visible_items(TaskView::OPTIONS.len() as u16)
            .min_popup_width(20)
            .trigger_hotkey(hotkey);
        Self {
            trigger,
            menu,
            pending_view,
            active_view,
        }
    }

    fn sync_activated(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let Some(view) = self.menu.take_activated().into_iter().last() else {
            return;
        };
        self.trigger.set_label(view.menu_label());
        *self.active_view.borrow_mut() = view;
        *self.pending_view.borrow_mut() = Some(view);
        ctx.request_layout();
        ctx.request_redraw();
    }
}

impl TuiNode<AppMsg> for TaskViewMenu {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.trigger.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.trigger
            .set_label(self.active_view.borrow().menu_label());
        ctx.push_slot(ChildKey::new(TASK_VIEW_MENU_TRIGGER), area, |ctx| {
            self.trigger.layout(area, ctx);
        });
        ctx.push_slot(ChildKey::new(TASK_VIEW_MENU_PANEL), area, |ctx| {
            <Menu<TaskView> as TuiNode<AppMsg>>::layout(&mut self.menu, area, ctx);
        });
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.trigger.render(frame, area);
        self.menu.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        if !self.menu.is_open() && keys::TASK_VIEW_MENU.matches(event) {
            self.menu.open_with_context(ctx);
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        let outcome = self.menu.event(event, ctx);
        self.sync_activated(ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        if route.path.is_empty() {
            return self.event(event, ctx);
        }
        let trigger_key = ChildKey::new(TASK_VIEW_MENU_TRIGGER);
        if let Some(route) = route
            .path
            .without_first_if(&trigger_key)
            .map(EventRoute::new)
        {
            let outcome = self.trigger.dispatch_event(&route, event, ctx);
            if outcome.handled() {
                self.menu.toggle_with_context(ctx);
            }
            return outcome;
        }
        let panel_key = ChildKey::new(TASK_VIEW_MENU_PANEL);
        let Some(route) = route.path.without_first_if(&panel_key).map(EventRoute::new) else {
            return EventOutcome::Ignored;
        };
        let outcome = self.menu.dispatch_event(&route, event, ctx);
        self.sync_activated(ctx);
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        let trigger_key = ChildKey::new(TASK_VIEW_MENU_TRIGGER);
        if let Some(target) = target.for_child(&trigger_key) {
            self.trigger.dispatch_focus(&target, focused, ctx);
            return;
        }
        let panel_key = ChildKey::new(TASK_VIEW_MENU_PANEL);
        if let Some(target) = target.for_child(&panel_key) {
            self.menu.dispatch_focus(&target, focused, ctx);
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.trigger
            .tick(dt, settings)
            .merge(<Menu<TaskView> as TuiNode<AppMsg>>::tick(
                &mut self.menu,
                dt,
                settings,
            ))
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.trigger.init(ctx);
        self.menu.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.trigger.mount(ctx);
        self.menu.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.menu.unmount(ctx);
        self.trigger.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.menu.destroy(ctx);
        self.trigger.destroy(ctx);
    }
}

struct TaskWorkspace {
    context: AppContext,
    layout: TaskWorkspaceLayout,
    task_view: TaskView,
    pending_task_view: TaskViewChange,
    active_task_view: ActiveTaskView,
    known_task_ids: Vec<String>,
    visible_task_ids: Vec<String>,
    visible_selection: VisibleTaskSelection,
    table_focused: bool,
    detail_draft_protected: bool,
    observed_version: u64,
    observed_external_refresh_version: u64,
}

#[derive(Debug, Default)]
struct TaskDetailSync {
    changed: bool,
    selected_task_changed: bool,
}

impl TaskWorkspace {
    fn new(context: AppContext) -> Self {
        let task_view = TaskView::InProgress;
        let state = context.store.borrow().state().clone();
        let selected_task_id = state
            .selected_task_id
            .clone()
            .filter(|id| {
                state
                    .tasks
                    .iter()
                    .any(|task| task.id == *id && task_view.contains(task))
            })
            .or_else(|| {
                state
                    .tasks
                    .iter()
                    .find(|task| task_view.contains(task))
                    .map(|task| task.id.clone())
            });
        let visible_task_ids = state
            .tasks
            .iter()
            .filter(|task| task_view.contains(task))
            .map(|task| task.id.clone())
            .collect();
        if let Some(task_id) = selected_task_id.as_ref()
            && state.selected_task_id.as_ref() != Some(task_id)
        {
            context
                .store
                .borrow_mut()
                .dispatch(AppEvent::SelectTask(task_id.clone()));
        }

        let pending_task_view = Rc::new(RefCell::new(None));
        let active_task_view = Rc::new(RefCell::new(task_view));
        let visible_selection = Rc::new(RefCell::new(selected_task_id.clone()));
        let toolbar = task_toolbar(Rc::clone(&pending_task_view), Rc::clone(&active_task_view));
        let pane = task_split(&context.store, task_view);
        let layout =
            Split::vertical(toolbar, pane).constraints(Constraint::Length(1), Constraint::Min(1));
        let observed_version = context.store.borrow().state().version;
        let observed_external_refresh_version = state.external_refresh_version;
        Self {
            context,
            layout,
            task_view,
            pending_task_view,
            active_task_view,
            known_task_ids: state.tasks.iter().map(|task| task.id.clone()).collect(),
            visible_task_ids,
            visible_selection,
            table_focused: false,
            detail_draft_protected: false,
            observed_version,
            observed_external_refresh_version,
        }
    }

    fn table(&self) -> &TaskTable {
        self.layout.second().first()
    }

    fn table_mut(&mut self) -> &mut TaskTable {
        self.layout.second_mut().first_mut()
    }

    fn detail(&self) -> &TaskDetail {
        self.layout.second().second()
    }

    fn detail_mut(&mut self) -> &mut TaskDetail {
        self.layout.second_mut().second_mut()
    }

    fn sync_store_version(&mut self) {
        let state = self.context.store.borrow().state().clone();
        let external_refresh =
            self.observed_external_refresh_version != state.external_refresh_version;
        if self.observed_version != state.version || external_refresh {
            let selected_new_todo = state
                .selected_task_id
                .as_deref()
                .filter(|id| !self.known_task_ids.iter().any(|known| known == *id))
                .and_then(|id| state.tasks.iter().find(|task| task.id == id))
                .is_some_and(|task| task.state == TaskState::Todo);
            if selected_new_todo && !matches!(self.task_view, TaskView::All | TaskView::Todo) {
                self.table_mut().clear_search();
                self.task_view = TaskView::All;
                *self.active_task_view.borrow_mut() = TaskView::All;
            }
            let protect_detail = external_refresh
                && (self.detail_draft_protected || self.context.coordinator.borrow().has_pending());
            self.refresh_from_state(
                &state,
                false,
                !external_refresh,
                external_refresh && !protect_detail,
            );
            if selected_new_todo {
                self.table_mut().reveal_highlighted();
            }
        }
    }

    fn refresh_from_state(
        &mut self,
        state: &AppState,
        select_first: bool,
        preserve_position: bool,
        refresh_detail: bool,
    ) {
        let external_refresh =
            self.observed_external_refresh_version != state.external_refresh_version;
        let previous_task_id = self.table().highlighted_id();
        let previous_index = preserve_position.then(|| {
            previous_task_id.as_ref().and_then(|id| {
                self.visible_task_ids
                    .iter()
                    .position(|visible_id| visible_id == id)
            })
        });
        let rows = state
            .tasks
            .iter()
            .filter(|task| self.task_view.contains(task))
            .cloned()
            .collect::<Vec<_>>();
        let contains_id = |id: &str| rows.iter().any(|task| task.id == id);
        let selected_task_id = if select_first {
            rows.first().map(|task| task.id.clone())
        } else {
            state
                .selected_task_id
                .as_deref()
                .filter(|id| contains_id(id))
                .map(str::to_string)
                .or_else(|| previous_task_id.filter(|id| contains_id(id)))
                .or_else(|| {
                    previous_index.flatten().and_then(|index| {
                        rows.get(index.min(rows.len().saturating_sub(1)))
                            .map(|task| task.id.clone())
                    })
                })
                .or_else(|| rows.first().map(|task| task.id.clone()))
        };
        let selected_task = selected_task_id
            .as_deref()
            .and_then(|id| state.tasks.iter().find(|task| task.id == id));
        let save_error = selected_task
            .and_then(|task| state.task_status_error(&task.id))
            .map(str::to_string);

        self.visible_task_ids = rows.iter().map(|task| task.id.clone()).collect();
        self.table_mut().set_rows(rows);
        if let Some(task_id) = selected_task_id.as_ref() {
            self.table_mut().highlight_id(task_id);
            self.table_mut().select_id(task_id.clone());
        }
        self.table_mut().take_events();
        *self.visible_selection.borrow_mut() = selected_task_id.clone();

        if let Some(task_id) = selected_task_id.as_ref()
            && state.selected_task_id.as_ref() != Some(task_id)
        {
            self.context
                .store
                .borrow_mut()
                .dispatch(AppEvent::SelectTask(task_id.clone()));
        }

        let detail_needs_refresh = self.detail().task_id.as_deref() != selected_task_id.as_deref()
            || self.detail().task_state != selected_task.map(|task| task.state);
        if detail_needs_refresh || refresh_detail {
            self.detail_mut().set_task(
                selected_task,
                &state.people,
                &state.projects,
                &state.tags,
                save_error.as_deref(),
                &mut EventCtx::default(),
            );
        } else {
            self.detail_mut().task_state = selected_task.map(|task| task.state);
        }
        self.detail_mut().set_save_error(save_error.as_deref());
        self.known_task_ids = state.tasks.iter().map(|task| task.id.clone()).collect();
        self.observed_version = state.version;
        if !external_refresh || refresh_detail {
            self.observed_external_refresh_version = state.external_refresh_version;
        }
    }

    fn sync_task_view_change(&mut self) -> bool {
        let Some(next_view) = self.pending_task_view.borrow_mut().take() else {
            return false;
        };
        if next_view == self.task_view {
            return false;
        }
        self.table_mut().clear_search();
        self.task_view = next_view;
        *self.active_task_view.borrow_mut() = next_view;
        let state = self.context.store.borrow().state().clone();
        self.refresh_from_state(&state, true, false, false);
        true
    }

    fn sync_table_events(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let events = self.table_mut().take_events();
        let mut focus_detail = false;
        let mut selected_changed = false;

        for event in events {
            match &event {
                DataViewTypedEvent::HighlightChanged { row_id: Some(id) }
                | DataViewTypedEvent::Activated { row_id: id } => {
                    selected_changed |= self.select_task(id, ctx);
                    if matches!(event, DataViewTypedEvent::Activated { .. }) {
                        focus_detail = true;
                    }
                }
                DataViewTypedEvent::HighlightChanged { row_id: None } => {
                    *self.visible_selection.borrow_mut() = None;
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

    fn select_task(&mut self, id: &str, ctx: &mut EventCtx<AppMsg>) -> bool {
        *self.visible_selection.borrow_mut() = Some(id.to_string());
        let outcome = self
            .context
            .store
            .borrow_mut()
            .dispatch(AppEvent::SelectTask(id.to_string()));
        let state = self.context.store.borrow().state().clone();
        let selected_task = state.tasks.iter().find(|task| task.id == id);
        let save_error = selected_task.and_then(|task| state.task_status_error(&task.id));
        self.detail_mut().set_task(
            selected_task,
            &state.people,
            &state.projects,
            &state.tags,
            save_error,
            ctx,
        );
        outcome.changed
    }

    fn drain_detail_patches(&mut self) -> bool {
        let patches = self.detail_mut().take_patches();
        let mut changed = false;
        for (task_id, patch) in patches {
            changed |= self.apply_patch(task_id, patch);
        }
        changed
    }

    fn sync_detail_changes(&mut self) -> TaskDetailSync {
        if !self.drain_detail_patches() {
            return TaskDetailSync::default();
        }
        self.detail_draft_protected = false;
        let previous_task_id = self.table().highlighted_id();
        let state = self.context.store.borrow().state().clone();
        self.refresh_from_state(&state, false, true, false);
        let selected_task_id = self.table().highlighted_id();
        TaskDetailSync {
            changed: true,
            selected_task_changed: selected_task_id.is_some()
                && selected_task_id != previous_task_id,
        }
    }

    fn apply_patch(&mut self, task_id: String, patch: TaskPatch) -> bool {
        let outcome = self
            .context
            .store
            .borrow_mut()
            .dispatch(AppEvent::PatchTask {
                task_id: task_id.clone(),
                patch: patch.clone(),
            });
        if !outcome.changed {
            return false;
        }
        self.context
            .coordinator
            .borrow_mut()
            .submit(PersistenceCommand::PatchTask(task_id, patch));
        true
    }

    fn handle_workspace_event(
        &self,
        outcome: EventOutcome,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        if outcome.handled() {
            return outcome;
        }
        let visible_task_id = self.visible_selection.borrow().clone();
        let message = if self.table_focused
            && visible_task_id.is_some()
            && app_keymap::matches_any(
                event,
                &[
                    keys::TASK_DELETE,
                    keys::TASK_DELETE_X,
                    keys::TASK_DELETE_CTRL_X,
                ],
            ) {
            visible_task_id.map(AppMsg::OpenDeleteTask)
        } else if self.table_focused
            && visible_task_id.is_some()
            && keys::TASK_SNOOZE.matches(event)
        {
            visible_task_id.map(AppMsg::OpenTaskSnooze)
        } else {
            None
        };
        if let Some(message) = message {
            ctx.emit(message);
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        if detail_escape(event) {
            focus_task_table(ctx);
            return EventOutcome::Handled;
        }
        outcome
    }
}

impl TuiNode<AppMsg> for TaskWorkspace {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync_store_version();
        self.layout.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.layout.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        self.sync_store_version();
        let outcome = self.layout.event(event, ctx);
        let view_changed = self.sync_task_view_change();
        let detail_sync = self.sync_detail_changes();
        if view_changed || detail_sync.changed {
            ctx.request_layout();
            ctx.request_redraw();
        }
        if view_changed || detail_sync.selected_task_changed {
            ctx.focus(FocusRequest::Target(FocusId::new("data-view")));
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
        self.sync_store_version();
        let outcome = self.layout.dispatch_event(route, event, ctx);
        let view_changed = self.sync_task_view_change();
        let detail_sync = self.sync_detail_changes();
        if view_changed || detail_sync.changed {
            ctx.request_layout();
            ctx.request_redraw();
        }
        if view_changed || detail_sync.selected_task_changed {
            ctx.focus(FocusRequest::Target(FocusId::new("data-view")));
        }
        self.sync_table_events(ctx);
        self.handle_workspace_event(outcome, event, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        let table_targeted = target
            .for_child(&ChildKey::second())
            .and_then(|target| target.for_child(&ChildKey::first()))
            .is_some();
        if table_targeted {
            self.table_focused = focused;
        } else if focused {
            self.table_focused = false;
        }
        let detail_targeted = target
            .for_child(&ChildKey::second())
            .and_then(|target| target.for_child(&ChildKey::second()))
            .is_some();
        if detail_targeted {
            self.detail_draft_protected = focused;
        } else if focused {
            self.detail_draft_protected = false;
        }
        self.layout.dispatch_focus(target, focused, ctx);
        let detail_sync = self.sync_detail_changes();
        if detail_sync.changed {
            ctx.request_redraw();
        }
        if detail_sync.selected_task_changed {
            ctx.focus(FocusRequest::Target(FocusId::new("data-view")));
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.layout.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.layout.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.layout.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.layout.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.layout.destroy(ctx);
    }
}

enum AppDialog {
    People(Box<people::PeopleDialog>),
    Projects(Box<projects::ProjectsDialog>),
    Tags(Box<tags::TagsDialog>),
    CreateManagement(DialogHost<CreateManagementDialog, AppMsg>),
    DeleteManagement(ConfirmationDialog<AppMsg>),
    CreateTask(DialogHost<CreateTaskDialog, AppMsg>),
    DeleteTask(ConfirmationDialog<AppMsg>),
    Empty(Dialog<AppMsg>),
    Snooze(Box<SnoozeDialog>),
}

fn empty_app_dialog() -> AppDialog {
    AppDialog::Empty(Dialog::new())
}

fn management_dialog(context: AppContext, kind: ManagementDialogKind) -> AppDialog {
    match kind {
        ManagementDialogKind::People => AppDialog::People(Box::new(people::dialog(context))),
        ManagementDialogKind::Projects => AppDialog::Projects(Box::new(projects::dialog(context))),
        ManagementDialogKind::Tags => AppDialog::Tags(Box::new(tags::dialog(context))),
    }
}

impl TuiNode<AppMsg> for AppDialog {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        match self {
            Self::People(dialog) => dialog.measure(proposal),
            Self::Projects(dialog) => dialog.measure(proposal),
            Self::Tags(dialog) => dialog.measure(proposal),
            Self::CreateManagement(dialog) => measure_dialog_host(dialog, proposal),
            Self::DeleteManagement(dialog) => dialog.measure(proposal),
            Self::CreateTask(dialog) => {
                let body = dialog.child().measure(proposal);
                let chrome = dialog.dialog().measure(proposal);
                let width = match proposal.width {
                    AxisProposal::AtMost(width) | AxisProposal::Exact(width) => width,
                    AxisProposal::Unbounded => body
                        .preferred
                        .width
                        .saturating_add(2)
                        .max(chrome.preferred.width),
                };
                LayoutSizeHint::content(
                    width,
                    body.preferred
                        .height
                        .saturating_add(chrome.preferred.height),
                )
                .normalized(proposal)
            }
            Self::DeleteTask(dialog) => dialog.measure(proposal),
            Self::Empty(dialog) => dialog.measure(proposal),
            Self::Snooze(dialog) => dialog.measure(proposal),
        }
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        match self {
            Self::People(dialog) => dialog.layout(area, ctx),
            Self::Projects(dialog) => dialog.layout(area, ctx),
            Self::Tags(dialog) => dialog.layout(area, ctx),
            Self::CreateManagement(dialog) => dialog.layout(area, ctx),
            Self::DeleteManagement(dialog) => dialog.layout(area, ctx),
            Self::CreateTask(dialog) => dialog.layout(area, ctx),
            Self::DeleteTask(dialog) => dialog.layout(area, ctx),
            Self::Empty(dialog) => dialog.layout(area, ctx),
            Self::Snooze(dialog) => dialog.layout(area, ctx),
        }
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        match self {
            Self::People(dialog) => dialog.render(frame, area, ctx),
            Self::Projects(dialog) => dialog.render(frame, area, ctx),
            Self::Tags(dialog) => dialog.render(frame, area, ctx),
            Self::CreateManagement(dialog) => dialog.render(frame, area, ctx),
            Self::DeleteManagement(dialog) => dialog.render(frame, area),
            Self::CreateTask(dialog) => dialog.render(frame, area, ctx),
            Self::DeleteTask(dialog) => dialog.render(frame, area),
            Self::Empty(dialog) => dialog.render(frame, area),
            Self::Snooze(dialog) => dialog.render(frame, area, ctx),
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        match self {
            Self::People(dialog) => dialog.event(event, ctx),
            Self::Projects(dialog) => dialog.event(event, ctx),
            Self::Tags(dialog) => dialog.event(event, ctx),
            Self::CreateManagement(dialog) => dialog.event(event, ctx),
            Self::DeleteManagement(dialog) => dialog.event(event, ctx),
            Self::CreateTask(dialog) => dialog.event(event, ctx),
            Self::DeleteTask(dialog) => dialog.event(event, ctx),
            Self::Empty(dialog) => dialog.event(event, ctx),
            Self::Snooze(dialog) => dialog.event(event, ctx),
        }
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        match self {
            Self::People(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::Projects(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::Tags(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::CreateManagement(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::DeleteManagement(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::CreateTask(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::DeleteTask(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::Empty(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::Snooze(dialog) => dialog.dispatch_event(route, event, ctx),
        }
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        match self {
            Self::People(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::Projects(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::Tags(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::CreateManagement(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::DeleteManagement(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::CreateTask(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::DeleteTask(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::Empty(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::Snooze(dialog) => dialog.dispatch_focus(target, focused, ctx),
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        match self {
            Self::People(dialog) => dialog.tick(dt, settings),
            Self::Projects(dialog) => dialog.tick(dt, settings),
            Self::Tags(dialog) => dialog.tick(dt, settings),
            Self::CreateManagement(dialog) => dialog.tick(dt, settings),
            Self::DeleteManagement(dialog) => dialog.tick(dt, settings),
            Self::CreateTask(dialog) => dialog.tick(dt, settings),
            Self::DeleteTask(dialog) => dialog.tick(dt, settings),
            Self::Empty(dialog) => dialog.tick(dt, settings),
            Self::Snooze(dialog) => dialog.tick(dt, settings),
        }
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::People(dialog) => dialog.init(ctx),
            Self::Projects(dialog) => dialog.init(ctx),
            Self::Tags(dialog) => dialog.init(ctx),
            Self::CreateManagement(dialog) => dialog.init(ctx),
            Self::DeleteManagement(dialog) => dialog.init(ctx),
            Self::CreateTask(dialog) => dialog.init(ctx),
            Self::DeleteTask(dialog) => dialog.init(ctx),
            Self::Empty(dialog) => dialog.init(ctx),
            Self::Snooze(dialog) => dialog.init(ctx),
        }
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::People(dialog) => dialog.mount(ctx),
            Self::Projects(dialog) => dialog.mount(ctx),
            Self::Tags(dialog) => dialog.mount(ctx),
            Self::CreateManagement(dialog) => dialog.mount(ctx),
            Self::DeleteManagement(dialog) => dialog.mount(ctx),
            Self::CreateTask(dialog) => dialog.mount(ctx),
            Self::DeleteTask(dialog) => dialog.mount(ctx),
            Self::Empty(dialog) => dialog.mount(ctx),
            Self::Snooze(dialog) => dialog.mount(ctx),
        }
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::People(dialog) => dialog.unmount(ctx),
            Self::Projects(dialog) => dialog.unmount(ctx),
            Self::Tags(dialog) => dialog.unmount(ctx),
            Self::CreateManagement(dialog) => dialog.unmount(ctx),
            Self::DeleteManagement(dialog) => dialog.unmount(ctx),
            Self::CreateTask(dialog) => dialog.unmount(ctx),
            Self::DeleteTask(dialog) => dialog.unmount(ctx),
            Self::Empty(dialog) => dialog.unmount(ctx),
            Self::Snooze(dialog) => dialog.unmount(ctx),
        }
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::People(dialog) => dialog.destroy(ctx),
            Self::Projects(dialog) => dialog.destroy(ctx),
            Self::Tags(dialog) => dialog.destroy(ctx),
            Self::CreateManagement(dialog) => dialog.destroy(ctx),
            Self::DeleteManagement(dialog) => dialog.destroy(ctx),
            Self::CreateTask(dialog) => dialog.destroy(ctx),
            Self::DeleteTask(dialog) => dialog.destroy(ctx),
            Self::Empty(dialog) => dialog.destroy(ctx),
            Self::Snooze(dialog) => dialog.destroy(ctx),
        }
    }
}

fn measure_dialog_host<C: TuiNode<AppMsg>>(
    dialog: &DialogHost<C, AppMsg>,
    proposal: LayoutProposal,
) -> LayoutSizeHint {
    let body = dialog.child().measure(proposal);
    let chrome = dialog.dialog().measure(proposal);
    let width = match proposal.width {
        AxisProposal::AtMost(width) | AxisProposal::Exact(width) => width,
        AxisProposal::Unbounded => body
            .preferred
            .width
            .saturating_add(2)
            .max(chrome.preferred.width),
    };
    LayoutSizeHint::content(
        width,
        body.preferred
            .height
            .saturating_add(chrome.preferred.height),
    )
    .normalized(proposal)
}

fn create_management_dialog_host(kind: ManagementDialogKind) -> AppDialog {
    let create = CreateManagementDialog::new(kind);
    let actions = create.actions();
    AppDialog::CreateManagement(
        Dialog::new()
            .top_left(format!("Create {}", kind.singular()))
            .actions(actions)
            .close_on_unfocus_from_descendants(true)
            .on_close(|_| AppMsg::CloseManagementOverlay)
            .host(create),
    )
}

fn delete_management_dialog(
    kind: ManagementDialogKind,
    entity_id: String,
    label: &str,
) -> AppDialog {
    let description = format!("Delete “{label}”? This cannot be undone.");
    let dialog = ConfirmationDialog::new(format!("Delete {}?", kind.singular()), &description)
        .yes_text("Delete")
        .yes_hotkey(KeySpec::plain('d'))
        .on_outcome(move |outcome| match outcome {
            ConfirmationDialogOutcome::Confirmed => AppMsg::DeleteManagementConfirmed {
                kind,
                entity_id: entity_id.clone(),
            },
            ConfirmationDialogOutcome::Cancelled | ConfirmationDialogOutcome::Closed(_) => {
                AppMsg::CloseManagementOverlay
            }
        });
    AppDialog::DeleteManagement(dialog)
}

fn notify_required(ctx: &mut EventCtx<AppMsg>, title: &str, body: &str) {
    ctx.notify(tuicore::Notification::warning(title, body));
}

fn create_task_dialog_host() -> AppDialog {
    let create_task = CreateTaskDialog::new();
    let actions = create_task.actions();
    AppDialog::CreateTask(
        Dialog::new()
            .top_left("Create task")
            .actions(actions)
            .close_on_unfocus_from_descendants(true)
            .on_close(|_| AppMsg::CloseDialog)
            .host(create_task),
    )
}

fn delete_task_dialog(task: &Task) -> AppDialog {
    let task_id = task.id.clone();
    let description = format!("Delete “{}”? This cannot be undone.", task.title);
    let dialog = ConfirmationDialog::new("Delete task?", &description)
        .yes_text("Delete")
        .yes_hotkey(KeySpec::plain('d'))
        .on_outcome(move |outcome| match outcome {
            ConfirmationDialogOutcome::Confirmed => AppMsg::DeleteTaskConfirmed(task_id.clone()),
            ConfirmationDialogOutcome::Cancelled | ConfirmationDialogOutcome::Closed(_) => {
                AppMsg::CloseDialog
            }
        });
    AppDialog::DeleteTask(dialog)
}

struct TaskDetailForm {
    root: Flex<AppMsg>,
    task_id: Option<String>,
    task_state: Option<TaskState>,
    patches: PatchSink,
    save_status: SaveStatusLine,
}

impl TaskDetailForm {
    fn new(
        task: Option<&TaskRow>,
        people: &[Person],
        projects: &[Project],
        tags: &[Tag],
        save_error: Option<&str>,
    ) -> Self {
        let patches = Rc::new(RefCell::new(Vec::new()));
        let save_status = SaveStatusLine::new(save_error);
        Self {
            root: Flex::column().child(
                "form",
                detail_form(
                    task,
                    people,
                    projects,
                    tags,
                    Rc::clone(&patches),
                    save_status.clone(),
                ),
                FlexItem::content(),
            ),
            task_id: task.map(|task| task.id.clone()),
            task_state: task.map(|task| task.state),
            patches,
            save_status,
        }
    }

    fn take_patches(&mut self) -> Vec<(String, TaskPatch)> {
        let Some(task_id) = self.task_id.clone() else {
            self.patches.borrow_mut().clear();
            return Vec::new();
        };
        self.patches
            .borrow_mut()
            .drain(..)
            .map(|patch| (task_id.clone(), patch))
            .collect()
    }

    fn set_task(
        &mut self,
        task: Option<&TaskRow>,
        people: &[Person],
        projects: &[Project],
        tags: &[Tag],
        save_error: Option<&str>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.patches = Rc::new(RefCell::new(Vec::new()));
        self.task_id = task.map(|task| task.id.clone());
        self.task_state = task.map(|task| task.state);
        self.save_status = SaveStatusLine::new(save_error);
        self.root
            .replace(
                "form",
                detail_form(
                    task,
                    people,
                    projects,
                    tags,
                    Rc::clone(&self.patches),
                    self.save_status.clone(),
                ),
                FlexItem::content(),
                ctx,
            )
            .expect("detail form host should contain form child");
    }

    fn set_save_error(&self, save_error: Option<&str>) {
        self.save_status.set_error(save_error);
    }
}

impl TuiNode<AppMsg> for TaskDetailForm {
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

fn task_toolbar(pending_view: TaskViewChange, active_view: ActiveTaskView) -> Flex<AppMsg> {
    let view = TaskViewMenu::new(pending_view, active_view);
    let new_task = Button::new("New")
        .hotkey(keys::TASK_QUICK_CREATE.hotkey())
        .on_press(|| AppMsg::OpenCreateTask);

    Flex::row()
        .align(CrossAlign::Center)
        .gap(1)
        .child("view", view, FlexItem::content())
        .child("space", Paragraph::new(""), FlexItem::fill(1))
        .child("new", new_task, FlexItem::content())
}

fn task_split(store: &AppStore, task_view: TaskView) -> TaskPane {
    let store_ref = store.borrow();
    let state = store_ref.state();
    let rows = state
        .tasks
        .iter()
        .filter(|task| task_view.contains(task))
        .cloned()
        .collect::<Vec<_>>();
    let selected = state.selected_task_id.as_deref().filter(|id| {
        state
            .tasks
            .iter()
            .any(|task| task.id == **id && task_view.contains(task))
    });
    let table = task_table(rows, selected);
    let selected_task = selected.and_then(|id| state.tasks.iter().find(|task| task.id == id));
    let save_error = selected_task.and_then(|task| state.task_status_error(&task.id));
    let detail = TaskDetailForm::new(
        selected_task,
        &state.people,
        &state.projects,
        &state.tags,
        save_error,
    );
    Split::horizontal(table, detail).ratio(65, 35)
}

fn task_table(rows: Vec<TaskRow>, selected_id: Option<&str>) -> DataView<TaskRow, String> {
    let mut table = DataView::new(rows, |row: &TaskRow| row.id.clone())
        .action_bar(true)
        .filter_controls(false)
        .activation_mode(ActivationMode::OnActivateKey)
        .selection_mode(SelectionMode::Single)
        .selection_trigger(SelectionTrigger::OnNavigate)
        .columns(vec![
            Column::rich(
                "state",
                "",
                Constraint::Length(1),
                |row: &TaskRow, _: &CellContext<String>| Line::from(task_state_icon(row.state)),
            )
            .constrained()
            .filter_key(|row| row.state.label().to_string()),
            Column::rich(
                "priority",
                "",
                Constraint::Length(1),
                |row: &TaskRow, _: &CellContext<String>| priority_icon_line(row.priority),
            )
            .constrained()
            .filter_key(|row| row.priority.label().to_string()),
            Column::rich(
                "size",
                "Size",
                Constraint::Length(5),
                |row: &TaskRow, _: &CellContext<String>| {
                    chip_line(row.size.label(), row.size.role())
                },
            )
            .constrained()
            .filter_key(|row| row.size.label().to_string()),
            Column::text("title", "Task", Constraint::Fill(1), |row: &TaskRow| {
                row.title.clone()
            })
            .constrained()
            .sortable(|row| row.title.clone())
            .filter_key(|row| row.title.clone()),
        ]);
    if let Some(id) = selected_id {
        table = table.selected([id.to_string()]);
    }
    table
}

struct TaskTagsInput {
    input: TagInput<String>,
    available_tags: Vec<Tag>,
    patch_sink: PatchSink,
}

impl TaskTagsInput {
    fn new(task: &Task, tags: &[Tag], patch_sink: PatchSink) -> Self {
        let input = TagInput::with_options(
            tags.iter().cloned(),
            |tag| tag.id.clone(),
            |tag| tag.label.clone(),
        )
        .selected_existing(task.tag_ids.iter().cloned())
        .panel("Tags")
        .hotkey(keys::TASK_TAGS_FIELD.hotkey());
        Self {
            input,
            available_tags: tags.to_vec(),
            patch_sink,
        }
    }

    fn sync_events(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let events = self.input.take_events();
        let value_changed = events.iter().any(|event| {
            !matches!(
                event,
                TagInputEvent::QueryChanged { .. } | TagInputEvent::SubmitRequested
            )
        });
        if !value_changed {
            return;
        }

        let mut selected = Vec::new();
        for tag in self.input.selected_tags() {
            let tag = match tag {
                SelectedTag::Existing { id, label } => Tag {
                    id: id.clone(),
                    label: label.clone(),
                },
                SelectedTag::Custom { label } => {
                    if let Some(existing) = self
                        .available_tags
                        .iter()
                        .find(|existing| existing.label == *label)
                    {
                        existing.clone()
                    } else {
                        let tag = Tag {
                            id: Uuid::new_v4().to_string(),
                            label: label.trim().to_string(),
                        };
                        self.available_tags.push(tag.clone());
                        tag
                    }
                }
            };
            if !tag.label.is_empty() && !selected.iter().any(|item: &Tag| item.id == tag.id) {
                selected.push(tag);
            }
        }
        self.patch_sink.borrow_mut().push(TaskPatch::Tags(selected));
        ctx.request_layout();
        ctx.request_redraw();
    }
}

impl TuiNode<AppMsg> for TaskTagsInput {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        <TagInput<String> as TuiNode<AppMsg>>::measure(&self.input, proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        <TagInput<String> as TuiNode<AppMsg>>::layout(&mut self.input, area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        <TagInput<String> as TuiNode<AppMsg>>::render(&self.input, frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        let outcome = <TagInput<String> as TuiNode<AppMsg>>::event(&mut self.input, event, ctx);
        self.sync_events(ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let outcome = <TagInput<String> as TuiNode<AppMsg>>::dispatch_event(
            &mut self.input,
            route,
            event,
            ctx,
        );
        self.sync_events(ctx);
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        <TagInput<String> as TuiNode<AppMsg>>::dispatch_focus(
            &mut self.input,
            target,
            focused,
            ctx,
        );
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        <TagInput<String> as TuiNode<AppMsg>>::tick(&mut self.input, dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        <TagInput<String> as TuiNode<AppMsg>>::init(&mut self.input, ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        <TagInput<String> as TuiNode<AppMsg>>::mount(&mut self.input, ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        <TagInput<String> as TuiNode<AppMsg>>::unmount(&mut self.input, ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        <TagInput<String> as TuiNode<AppMsg>>::destroy(&mut self.input, ctx);
    }
}

fn detail_form(
    task: Option<&TaskRow>,
    people: &[Person],
    projects: &[Project],
    tags: &[Tag],
    patch_sink: PatchSink,
    save_status: SaveStatusLine,
) -> Flex<AppMsg> {
    let Some(task) = task else {
        return Flex::<AppMsg>::column().child(
            "empty",
            Paragraph::new("No task selected."),
            FlexItem::fixed(1),
        );
    };

    let mut form = Flex::<AppMsg>::column()
        .gap(0)
        .child("save-status", save_status, FlexItem::content())
        .child(
            "title",
            TextInput::<AppMsg>::new()
                .value(task.title.clone())
                .panel("Title")
                .hotkey(keys::TASK_TITLE_FIELD.hotkey())
                .on_edit_end({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| {
                        patch_sink.borrow_mut().push(TaskPatch::Title(value));
                        AppMsg::Noop
                    }
                }),
            FlexItem::fixed(3),
        )
        .child(
            "description",
            TextareaInput::<AppMsg>::new()
                .value(task.detail.clone())
                .panel("Description")
                .hotkey(keys::TASK_DESCRIPTION_FIELD.hotkey())
                .editor_hotkey(keys::TASK_DESCRIPTION_EDITOR.hotkey())
                .on_edit_end({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| {
                        patch_sink.borrow_mut().push(TaskPatch::Detail(value));
                        AppMsg::Noop
                    }
                })
                .min_rows(4)
                .max_rows(10),
            FlexItem::content(),
        )
        .child(
            "state",
            dropdown_single("State", state_choices(), task.state.id(), {
                let patch_sink = Rc::clone(&patch_sink);
                move |id| {
                    if let Some(value) = TaskState::parse(&id) {
                        patch_sink.borrow_mut().push(TaskPatch::State(value));
                    }
                }
            })
            .hotkey(keys::TASK_STATE_FIELD.hotkey()),
            FlexItem::fixed(3),
        )
        .child(
            "size",
            dropdown_single("Size", size_choices(), task.size.id(), {
                let patch_sink = Rc::clone(&patch_sink);
                move |id| {
                    if let Some(value) = TaskSize::parse(&id) {
                        patch_sink.borrow_mut().push(TaskPatch::Size(value));
                    }
                }
            })
            .hotkey(keys::TASK_SIZE_FIELD.hotkey()),
            FlexItem::fixed(3),
        )
        .child(
            "priority",
            dropdown_single("Priority", priority_choices(), task.priority.id(), {
                let patch_sink = Rc::clone(&patch_sink);
                move |id| {
                    if let Some(value) = TaskPriority::parse(&id) {
                        patch_sink.borrow_mut().push(TaskPatch::Priority(value));
                    }
                }
            })
            .hotkey(keys::TASK_PRIORITY_FIELD.hotkey()),
            FlexItem::fixed(3),
        )
        .child(
            "people",
            task_people_dropdown(task, people, Rc::clone(&patch_sink)),
            FlexItem::fixed(3),
        )
        .child(
            "projects",
            task_projects_dropdown(task, projects, Rc::clone(&patch_sink)),
            FlexItem::fixed(3),
        )
        .child(
            "tags",
            TaskTagsInput::new(task, tags, Rc::clone(&patch_sink)),
            FlexItem::content(),
        )
        .child(
            "start-date",
            DatePickerDropdown::<AppMsg>::new()
                .value(parse_date(task.start_date.as_deref()))
                .panel("Start date")
                .hotkey(keys::TASK_START_DATE_FIELD.hotkey())
                .on_select({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |date| {
                        patch_sink
                            .borrow_mut()
                            .push(TaskPatch::StartDate(Some(date.to_string())));
                        AppMsg::Noop
                    }
                }),
            FlexItem::fixed(3),
        )
        .child(
            "end-date",
            DatePickerDropdown::<AppMsg>::new()
                .value(parse_date(task.due_date.as_deref()))
                .panel("End date")
                .hotkey(keys::TASK_END_DATE_FIELD.hotkey())
                .on_select({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |date| {
                        patch_sink
                            .borrow_mut()
                            .push(TaskPatch::EndDate(Some(date.to_string())));
                        AppMsg::Noop
                    }
                }),
            FlexItem::fixed(3),
        );
    if task.state == TaskState::Snoozed {
        form = form.child(
            "snoozed-until",
            DateTimePickerDropdown::<AppMsg>::new()
                .value(task.snoozed_until)
                .panel("Snoozed until")
                .hotkey(keys::TASK_SNOOZED_UNTIL_FIELD.hotkey())
                .on_select({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |until| {
                        patch_sink.borrow_mut().push(TaskPatch::Snooze {
                            until,
                            remember_custom: None,
                        });
                        AppMsg::Noop
                    }
                }),
            FlexItem::fixed(3),
        );
    }
    form
}

fn parse_date(value: Option<&str>) -> Option<Date> {
    value.and_then(|value| {
        Date::parse(value, &time::format_description::well_known::Iso8601::DATE).ok()
    })
}

fn chip_line(label: &'static str, role: ChipColorRole) -> Line<'static> {
    let theme = tuicore::theme();
    let color = match role {
        ChipColorRole::Accent => theme.accent_fg(),
        ChipColorRole::Success => theme.success_fg(),
        ChipColorRole::Warning => theme.warning_fg(),
        ChipColorRole::Error => theme.error_fg(),
        ChipColorRole::Selected => theme.selected_bg(),
        ChipColorRole::Highlight => theme.highlight_bg(),
        ChipColorRole::Muted => theme.border_fg(),
    };
    Line::from(Span::styled(
        label,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
}

fn task_state_icon(state: TaskState) -> &'static str {
    match state {
        TaskState::Todo => "",
        TaskState::InProgress => "",
        TaskState::Done => "",
        TaskState::Snoozed => "󰒲",
        TaskState::Rejected => "",
    }
}

fn priority_icon_line(priority: TaskPriority) -> Line<'static> {
    let theme = tuicore::theme();
    let color = match priority {
        TaskPriority::Low => theme.accent_fg(),
        TaskPriority::Medium => theme.warning_fg(),
        TaskPriority::High => theme.error_fg(),
    };
    Line::from(Span::styled(
        task_priority_icon(priority),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
}

fn task_priority_icon(priority: TaskPriority) -> &'static str {
    match priority {
        TaskPriority::Low => "󰅀",
        TaskPriority::Medium => "󰇼",
        TaskPriority::High => "󰅃",
    }
}

pub(crate) fn detail_escape(event: &TuiEvent) -> bool {
    app_keymap::matches_any(event, &[keys::DETAIL_CLOSE, keys::DETAIL_CLOSE_ALT])
}

fn detail_outcome_or_escape(
    outcome: EventOutcome,
    event: &TuiEvent,
    ctx: &mut EventCtx<AppMsg>,
) -> EventOutcome {
    if outcome.handled() {
        return outcome;
    }
    if detail_escape(event) {
        focus_task_table(ctx);
        return EventOutcome::Handled;
    }
    outcome
}

fn focus_task_table(ctx: &mut EventCtx<AppMsg>) {
    ctx.focus(FocusRequest::Target(FocusId::new("data-view")));
    ctx.stop_propagation();
    ctx.request_redraw();
}

fn dropdown_single(
    label: &'static str,
    rows: Vec<Choice>,
    selected: &str,
    on_select: impl Fn(String) + 'static,
) -> Dropdown<Choice, String> {
    Dropdown::single(rows, |row| row.id.clone(), |row| row.label.clone())
        .label(label)
        .selected_one(selected.to_string())
        .search_mode(DropdownSearchMode::Contains)
        .commit_mode(DropdownCommitMode::Explicit)
        .on_select(move |ids| {
            if let Some(id) = ids.into_iter().next() {
                on_select(id);
            }
        })
}

fn dropdown_multi(
    label: &'static str,
    rows: Vec<Choice>,
    selected: &[String],
    on_select: impl Fn(Vec<String>) + 'static,
) -> Dropdown<Choice, String> {
    Dropdown::multi(rows, |row| row.id.clone(), |row| row.label.clone())
        .label(label)
        .placeholder("Select")
        .selected(selected.iter().cloned())
        .search_mode(DropdownSearchMode::Contains)
        .on_select(on_select)
}

fn task_people_dropdown(
    task: &TaskRow,
    people: &[Person],
    patch_sink: PatchSink,
) -> Dropdown<Choice, String> {
    dropdown_multi(
        "People",
        person_choices(people),
        &task.people_ids,
        move |ids| patch_sink.borrow_mut().push(TaskPatch::People(ids)),
    )
    .hotkey(keys::TASK_PEOPLE_FIELD.hotkey())
}

fn task_projects_dropdown(
    task: &TaskRow,
    projects: &[Project],
    patch_sink: PatchSink,
) -> Dropdown<Choice, String> {
    dropdown_multi(
        "Projects",
        project_choices(projects),
        &task.project_ids,
        move |ids| patch_sink.borrow_mut().push(TaskPatch::Projects(ids)),
    )
    .hotkey(keys::TASK_PROJECTS_FIELD.hotkey())
}

#[derive(Debug, Clone)]
struct Choice {
    id: String,
    label: String,
}

fn state_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "todo".to_string(),
            label: "Todo".to_string(),
        },
        Choice {
            id: "in_progress".to_string(),
            label: "In-progress".to_string(),
        },
        Choice {
            id: "done".to_string(),
            label: "Done".to_string(),
        },
        Choice {
            id: "rejected".to_string(),
            label: "Rejected".to_string(),
        },
    ]
}

fn size_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "small".to_string(),
            label: "Small".to_string(),
        },
        Choice {
            id: "medium".to_string(),
            label: "Medium".to_string(),
        },
        Choice {
            id: "big".to_string(),
            label: "Big".to_string(),
        },
    ]
}

fn priority_choices() -> Vec<Choice> {
    [TaskPriority::Low, TaskPriority::Medium, TaskPriority::High]
        .into_iter()
        .map(|priority| Choice {
            id: priority.id().to_string(),
            label: priority.label().to_string(),
        })
        .collect()
}

fn person_choices(people: &[Person]) -> Vec<Choice> {
    people
        .iter()
        .map(|person| Choice {
            id: person.id.clone(),
            label: person.name.clone(),
        })
        .collect()
}

fn project_choices(projects: &[Project]) -> Vec<Choice> {
    projects
        .iter()
        .map(|project| Choice {
            id: project.id.clone(),
            label: project.name.clone(),
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::domain::{SaveTarget, TaskField, WorkspaceSnapshot};
    use ratatui::{Terminal, backend::TestBackend};
    use sqlx::any::AnyPoolOptions;
    use tuicore::{
        FocusManager, HotkeyEvent, Key, KeyEvent, KeyModifiers, Propagation, TreeDispatcher,
    };

    fn test_task() -> Task {
        Task {
            id: "task-1".to_string(),
            title: "Original".to_string(),
            state: TaskState::InProgress,
            size: TaskSize::Small,
            priority: TaskPriority::Medium,
            start_date: None,
            due_date: None,
            snoozed_until: None,
            people_ids: Vec::new(),
            project_ids: Vec::new(),
            tag_ids: Vec::new(),
            detail: "Existing detail".to_string(),
        }
    }

    #[test]
    fn task_tags_input_selects_existing_tags_and_creates_shared_candidates() {
        let api = Tag {
            id: "tag-api".to_string(),
            label: "api".to_string(),
        };
        let patches = Rc::new(RefCell::new(Vec::new()));
        let mut input = TaskTagsInput::new(
            &test_task(),
            std::slice::from_ref(&api),
            Rc::clone(&patches),
        );
        input.input.set_focused(true);
        let mut ctx = EventCtx::default();

        input.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
        for character in "api".chars() {
            input.event(
                &TuiEvent::Key(KeyEvent::from(Key::Char(character))),
                &mut ctx,
            );
        }
        input.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
        for character in "backend".chars() {
            input.event(
                &TuiEvent::Key(KeyEvent::from(Key::Char(character))),
                &mut ctx,
            );
        }
        input.event(
            &TuiEvent::Key(KeyEvent {
                code: Key::Enter,
                modifiers: KeyModifiers::CONTROL,
            }),
            &mut ctx,
        );

        let patches = patches.borrow();
        let TaskPatch::Tags(tags) = patches.last().expect("tag changes should emit a patch") else {
            panic!("expected tags patch");
        };
        assert_eq!(tags.first(), Some(&api));
        assert_eq!(tags.get(1).map(|tag| tag.label.as_str()), Some("backend"));
        assert_ne!(tags[1].id, api.id);
    }

    #[test]
    fn task_tags_input_participates_in_control_focus_navigation() {
        let mut input = TaskTagsInput::new(&test_task(), &[], Rc::new(RefCell::new(Vec::new())));
        let mut layout = LayoutCtx::new();

        input.layout(Rect::new(0, 0, 40, 3), &mut layout);

        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.id.as_str() == "tag-input")
            .expect("tags input should register a focus target");
        assert!(target.enabled);
        assert!(target.control);
    }

    fn task_with(id: &str, title: &str, state: TaskState) -> Task {
        let mut task = test_task();
        task.id = id.to_string();
        task.title = title.to_string();
        task.state = state;
        task
    }

    fn select_workspace_task(workspace: &mut TaskWorkspace, task_id: &str) {
        let task_id = task_id.to_string();
        workspace.table_mut().highlight_id(&task_id);
        workspace.table_mut().select_id(task_id.clone());
        workspace.table_mut().take_events();
        workspace.select_task(&task_id, &mut EventCtx::default());
    }

    pub(crate) fn test_context(
        snapshot: WorkspaceSnapshot,
    ) -> (tokio::runtime::Runtime, AppContext, AppStore) {
        sqlx::any::install_default_drivers();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime should build");
        let pool = {
            let _runtime_guard = runtime.enter();
            AnyPoolOptions::new()
                .connect_lazy("sqlite::memory:")
                .expect("lazy pool should build")
        };
        let store = Rc::new(RefCell::new(Store::new(
            AppState::from_snapshot(snapshot),
            reduce_app_state as fn(&mut AppState, AppEvent) -> tuicore::DispatchOutcome,
        )));
        let coordinator = Rc::new(RefCell::new(PersistenceCoordinator::new(
            Rc::clone(&store),
            pool,
            crate::storage::SqlDialect::Sqlite,
            runtime.handle().clone(),
            None,
        )));
        let context = AppContext {
            store: Rc::clone(&store),
            coordinator,
        };
        (runtime, context, store)
    }

    pub(crate) fn rendered_text(node: &impl TuiNode<AppMsg>, area: Rect) -> String {
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))
            .expect("terminal should build");
        terminal
            .draw(|frame| node.render(frame, area, &mut RenderCtx::new()))
            .expect("node should render");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn external_refresh_repopulates_selected_task_detail_once_draft_is_safe() {
        let task = test_task();
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![task.clone()],
            people: vec![],
            projects: vec![],
            tags: vec![],
        });
        let mut workspace = TaskWorkspace::new(context);
        workspace.detail_draft_protected = true;
        let mut refreshed = task;
        refreshed.title = "Externally changed".into();
        store.borrow_mut().dispatch(AppEvent::WorkspaceRefreshed {
            snapshot: WorkspaceSnapshot {
                tasks: vec![refreshed],
                people: vec![],
                projects: vec![],
                tags: vec![],
            },
            revision: 1,
            entity_revisions: std::collections::HashMap::new(),
        });
        let area = Rect::new(0, 0, 100, 30);

        workspace.layout(area, &mut LayoutCtx::new());
        assert!(rendered_text(workspace.detail(), area).contains("Original"));

        workspace.detail_draft_protected = false;
        workspace.layout(area, &mut LayoutCtx::new());
        let detail = rendered_text(workspace.detail(), area);
        assert!(detail.contains("Externally changed"));
        assert_eq!(
            workspace.table().highlighted_id().as_deref(),
            Some("task-1")
        );
    }

    fn rendered_area_has_focus_style(
        node: &impl TuiNode<AppMsg>,
        canvas: Rect,
        area: Rect,
    ) -> bool {
        let mut terminal = Terminal::new(TestBackend::new(canvas.width, canvas.height))
            .expect("terminal should build");
        terminal
            .draw(|frame| node.render(frame, canvas, &mut RenderCtx::new()))
            .expect("node should render");
        let buffer = terminal.backend().buffer();
        let theme = tuicore::theme();
        (area.y..area.bottom()).any(|y| {
            (area.x..area.right()).any(|x| {
                let cell = buffer.cell((x, y)).expect("focused area cell should exist");
                cell.fg == theme.highlight_fg() && cell.bg == theme.highlight_bg()
            })
        })
    }

    #[test]
    fn task_toolbar_shows_icon_menu_and_new_binding() {
        assert_eq!(
            TaskView::OPTIONS,
            [
                TaskView::All,
                TaskView::Todo,
                TaskView::Snoozed,
                TaskView::InProgress,
                TaskView::Archived,
            ]
        );
        assert_eq!(
            TaskView::OPTIONS.map(TaskView::label),
            ["All", "Todo", "Snoozed", "In progress", "Archived"]
        );
        assert_eq!(
            TaskView::OPTIONS.map(TaskView::icon),
            ["", "", "󰒲", "", ""]
        );
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let area = Rect::new(0, 0, 80, 40);

        workspace.layout(area, &mut LayoutCtx::new());
        let text = rendered_text(&workspace, area);

        for expected in [
            "",
            "In progress",
            &keys::TASK_VIEW_MENU.label(),
            &keys::TASK_QUICK_CREATE.label(),
            "New",
        ] {
            assert!(text.contains(expected), "missing toolbar text: {expected}");
        }
        assert!(!text.contains("View:"));
        assert!(!text.contains("Resolve"));
        assert!(!text.contains("Permanently"));
    }

    #[test]
    fn task_table_state_column_is_icon_only() {
        let mut table = task_table(
            vec![
                task_with("todo", "Todo work", TaskState::Todo),
                task_with("active", "Active work", TaskState::InProgress),
                task_with("done", "Done work", TaskState::Done),
                task_with("snoozed", "Snoozed work", TaskState::Snoozed),
                task_with("rejected", "Rejected work", TaskState::Rejected),
            ],
            None,
        );
        let area = Rect::new(0, 0, 100, 10);
        <TaskTable as TuiNode<AppMsg>>::layout(&mut table, area, &mut LayoutCtx::new());

        let text = rendered_text(&table, area);

        assert!(!text.contains("State"));
        for label in ["TODO", "IN-PROGRESS", "DONE", "SNOOZED", "REJECTED"] {
            assert!(
                !text.contains(label),
                "state label leaked into table: {label}"
            );
        }
        for icon in ["", "", "", "󰒲", ""] {
            assert!(text.contains(icon), "missing state icon: {icon}");
        }
    }

    #[test]
    fn task_table_fixed_columns_keep_padding_before_flexible_title() {
        for width in [40, 100, 200] {
            let mut table = task_table(
                vec![task_with("active", "Zebra work", TaskState::InProgress)],
                None,
            );
            let area = Rect::new(0, 0, width, 5);
            <TaskTable as TuiNode<AppMsg>>::layout(&mut table, area, &mut LayoutCtx::new());
            let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))
                .expect("terminal should build");

            terminal
                .draw(|frame| {
                    <TaskTable as TuiNode<AppMsg>>::render(
                        &table,
                        frame,
                        area,
                        &mut RenderCtx::new(),
                    )
                })
                .expect("table should render");

            let buffer = terminal.backend().buffer();
            assert_eq!(buffer.cell((0, 1)).unwrap().symbol(), "");
            assert_eq!(buffer.cell((1, 1)).unwrap().symbol(), " ");
            assert_eq!(buffer.cell((2, 1)).unwrap().symbol(), "󰇼");
            assert_eq!(buffer.cell((3, 1)).unwrap().symbol(), " ");
            assert_eq!(buffer.cell((4, 1)).unwrap().symbol(), "S");
            assert_eq!(buffer.cell((8, 1)).unwrap().symbol(), "L");
            assert_eq!(buffer.cell((9, 1)).unwrap().symbol(), " ");
            assert_eq!(buffer.cell((10, 1)).unwrap().symbol(), "Z");
        }
    }

    #[test]
    fn task_table_priority_is_icon_only_in_second_column() {
        let mut low = task_with("low", "Low work", TaskState::Todo);
        low.priority = TaskPriority::Low;
        let mut medium = task_with("medium", "Medium work", TaskState::Todo);
        medium.priority = TaskPriority::Medium;
        let mut high = task_with("high", "High work", TaskState::Todo);
        high.priority = TaskPriority::High;
        let mut table = task_table(vec![low, medium, high], None);
        let area = Rect::new(0, 0, 100, 8);
        <TaskTable as TuiNode<AppMsg>>::layout(&mut table, area, &mut LayoutCtx::new());

        let text = rendered_text(&table, area);

        assert!(!text.contains("Priority"));
        for icon in ["󰅀", "󰇼", "󰅃"] {
            assert!(text.contains(icon), "missing priority icon: {icon}");
        }
    }

    #[test]
    fn task_table_state_icon_uses_row_text_color() {
        let mut table = task_table(
            vec![task_with("active", "Zebra work", TaskState::InProgress)],
            None,
        );
        let area = Rect::new(0, 0, 100, 5);
        <TaskTable as TuiNode<AppMsg>>::layout(&mut table, area, &mut LayoutCtx::new());
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))
            .expect("terminal should build");
        terminal
            .draw(|frame| {
                <TaskTable as TuiNode<AppMsg>>::render(&table, frame, area, &mut RenderCtx::new())
            })
            .expect("table should render");
        let cells = terminal.backend().buffer().content();
        let icon = cells
            .iter()
            .find(|cell| cell.symbol() == "")
            .expect("state icon should render");
        let title = cells
            .iter()
            .find(|cell| cell.symbol() == "Z")
            .expect("task title should render");

        assert_eq!(icon.fg, title.fg);
    }

    #[test]
    fn task_toolbar_new_button_emits_create_action() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let mut layout = LayoutCtx::new();
        workspace.layout(Rect::new(0, 0, 80, 40), &mut layout);
        let button_path = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().iter().any(|part| part.as_str() == "new"))
            .expect("missing new task toolbar button")
            .path
            .clone();

        let mut create_ctx = EventCtx::default();
        let create = workspace.dispatch_event(
            &EventRoute::new(button_path.clone()),
            &TuiEvent::Key(Key::Enter.into()),
            &mut create_ctx,
        );
        assert!(create.handled());
        assert!(matches!(create_ctx.messages(), [AppMsg::OpenCreateTask]));

        let mut hotkey_ctx = EventCtx::default();
        let hotkey = workspace.dispatch_event(
            &EventRoute::new(button_path),
            &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_QUICK_CREATE.hotkey())),
            &mut hotkey_ctx,
        );
        assert!(hotkey.handled());
        assert!(matches!(hotkey_ctx.messages(), [AppMsg::OpenCreateTask]));
    }

    #[test]
    fn escape_from_task_toolbar_controls_focuses_data_view() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let mut layout = LayoutCtx::new();
        workspace.layout(Rect::new(0, 0, 80, 40), &mut layout);
        let toolbar_paths = [
            layout
                .focus_targets()
                .iter()
                .find(|target| target.path.keys().iter().any(|part| part.as_str() == "new"))
                .expect("new task button should be focusable")
                .path
                .clone(),
            layout
                .focus_targets()
                .iter()
                .find(|target| {
                    let path = target.path.keys();
                    path.iter().any(|part| part.as_str() == "view")
                        && path
                            .iter()
                            .any(|part| part.as_str() == TASK_VIEW_MENU_TRIGGER)
                })
                .expect("task filter button should be focusable")
                .path
                .clone(),
        ];
        let close_keys = [
            KeyEvent::from(Key::Esc),
            KeyEvent {
                code: Key::Char('['),
                modifiers: KeyModifiers::CONTROL,
            },
        ];

        for path in toolbar_paths {
            for key in close_keys {
                let mut ctx = EventCtx::default();
                let outcome = workspace.dispatch_event(
                    &EventRoute::new(path.clone()),
                    &TuiEvent::Key(key),
                    &mut ctx,
                );

                assert!(outcome.handled());
                assert_eq!(
                    ctx.focus_request(),
                    Some(&FocusRequest::Target(FocusId::new("data-view")))
                );
            }
        }
    }

    #[test]
    fn task_view_menu_shortcut_opens_and_switches_to_snoozed() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![
                task_with("active", "Active work", TaskState::InProgress),
                task_with("snoozed", "Snoozed work", TaskState::Snoozed),
            ],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let area = Rect::new(0, 0, 80, 40);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let trigger = layout
            .focus_targets()
            .iter()
            .find(|target| {
                let path = target.path.keys();
                path.iter().any(|part| part.as_str() == "view")
                    && path
                        .iter()
                        .any(|part| part.as_str() == TASK_VIEW_MENU_TRIGGER)
            })
            .expect("view menu trigger should be focusable")
            .clone();
        let trigger_route = EventRoute::new(trigger.path);
        let mut open_ctx = EventCtx::default();
        let open = workspace.dispatch_event(
            &trigger_route,
            &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_VIEW_MENU.hotkey())),
            &mut open_ctx,
        );
        assert!(open.handled());
        assert!(matches!(
            open_ctx.focus_request(),
            Some(FocusRequest::TargetAt { id, .. }) if id.as_str() == "search"
        ));

        let mut open_layout = LayoutCtx::new();
        workspace.layout(area, &mut open_layout);
        let panel = open_layout
            .focus_targets()
            .iter()
            .find(|target| {
                let path = target.path.keys();
                path.iter().any(|part| part.as_str() == "view")
                    && path
                        .iter()
                        .any(|part| part.as_str() == TASK_VIEW_MENU_PANEL)
            })
            .expect("open view menu search should be focusable")
            .clone();
        let panel_route = EventRoute::new(panel.path);
        let next = KeyEvent {
            code: Key::Char('j'),
            modifiers: KeyModifiers::CONTROL,
        };
        for key in [next, next, KeyEvent::from(Key::Enter)] {
            let outcome = workspace.dispatch_event(
                &panel_route,
                &TuiEvent::Key(key),
                &mut EventCtx::default(),
            );
            assert!(outcome.handled(), "menu ignored {key:?}");
        }

        assert_eq!(workspace.task_view, TaskView::Snoozed);
        workspace.layout(area, &mut LayoutCtx::new());
        let text = rendered_text(&workspace, area);
        assert!(text.contains("Snoozed work"));
        assert!(!text.contains("Active work"));
    }

    #[test]
    fn task_views_group_tasks_by_workflow_state() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![
                task_with("active", "Active work", TaskState::InProgress),
                task_with("todo", "Todo work", TaskState::Todo),
                task_with("done", "Completed work", TaskState::Done),
                task_with("rejected", "Rejected work", TaskState::Rejected),
                task_with("snoozed", "Snoozed work", TaskState::Snoozed),
            ],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let area = Rect::new(0, 0, 120, 40);

        workspace.layout(area, &mut LayoutCtx::new());
        let active = rendered_text(&workspace, area);
        assert!(active.contains("Active work"));
        assert!(!active.contains("Todo work"));
        assert!(!active.contains("Completed work"));

        *workspace.pending_task_view.borrow_mut() = Some(TaskView::Todo);
        assert!(workspace.sync_task_view_change());
        workspace.layout(area, &mut LayoutCtx::new());
        let todo = rendered_text(&workspace, area);
        assert!(todo.contains("Todo work"));
        assert!(!todo.contains("Active work"));

        *workspace.pending_task_view.borrow_mut() = Some(TaskView::Snoozed);
        assert!(workspace.sync_task_view_change());
        workspace.layout(area, &mut LayoutCtx::new());
        let snoozed = rendered_text(&workspace, area);
        assert!(snoozed.contains("Snoozed work"));
        assert!(!snoozed.contains("Todo work"));

        *workspace.pending_task_view.borrow_mut() = Some(TaskView::Archived);
        assert!(workspace.sync_task_view_change());
        workspace.layout(area, &mut LayoutCtx::new());
        let archived = rendered_text(&workspace, area);
        assert!(archived.contains("Completed work"));
        assert!(archived.contains("Rejected work"));
        assert!(!archived.contains("Snoozed work"));

        *workspace.pending_task_view.borrow_mut() = Some(TaskView::All);
        assert!(workspace.sync_task_view_change());
        workspace.layout(area, &mut LayoutCtx::new());
        let all = rendered_text(&workspace, area);
        for title in ["Active work", "Todo work", "Snoozed work"] {
            assert!(all.contains(title), "missing task in All view: {title}");
        }
        assert!(!all.contains("Completed work"));
        assert!(!all.contains("Rejected work"));
    }

    #[test]
    fn switching_views_selects_first_visible_task() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![
                task_with("active-1", "Active one", TaskState::InProgress),
                task_with("todo-1", "Todo one", TaskState::Todo),
                task_with("todo-2", "Todo two", TaskState::Todo),
            ],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);

        *workspace.pending_task_view.borrow_mut() = Some(TaskView::Todo);
        assert!(workspace.sync_task_view_change());
        select_workspace_task(&mut workspace, "todo-2");
        *workspace.pending_task_view.borrow_mut() = Some(TaskView::InProgress);
        assert!(workspace.sync_task_view_change());
        *workspace.pending_task_view.borrow_mut() = Some(TaskView::Todo);
        assert!(workspace.sync_task_view_change());

        assert_eq!(
            workspace.table().highlighted_id().as_deref(),
            Some("todo-1")
        );
        assert_eq!(workspace.detail().task_id.as_deref(), Some("todo-1"));
    }

    #[test]
    fn switching_task_view_clears_table_search() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![
                task_with("active-1", "Active one", TaskState::InProgress),
                task_with("todo-1", "Todo one", TaskState::Todo),
            ],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        workspace.table_mut().set_search_query("Active");
        *workspace.pending_task_view.borrow_mut() = Some(TaskView::Todo);

        assert!(workspace.sync_task_view_change());

        assert!(workspace.table().transform_state().search.is_empty());
    }

    #[test]
    fn switching_views_focuses_first_visible_table_row() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![
                task_with("active-1", "Active one", TaskState::InProgress),
                task_with("todo-1", "Todo one", TaskState::Todo),
                task_with("todo-2", "Todo two", TaskState::Todo),
            ],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        *workspace.pending_task_view.borrow_mut() = Some(TaskView::Todo);
        let mut ctx = EventCtx::default();

        workspace.event(&TuiEvent::Key(Key::Char('~').into()), &mut ctx);

        assert_eq!(
            workspace.table().highlighted_id().as_deref(),
            Some("todo-1")
        );
        assert_eq!(
            ctx.focus_request(),
            Some(&FocusRequest::Target(FocusId::new("data-view")))
        );
    }

    #[test]
    fn state_change_selects_next_visible_task() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![
                task_with("active-1", "Active one", TaskState::InProgress),
                task_with("active-2", "Active two", TaskState::InProgress),
                task_with("active-3", "Active three", TaskState::InProgress),
            ],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        select_workspace_task(&mut workspace, "active-2");

        store.borrow_mut().dispatch(AppEvent::PatchTask {
            task_id: "active-2".to_string(),
            patch: TaskPatch::State(TaskState::Done),
        });
        workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());

        assert_eq!(
            workspace.table().highlighted_id().as_deref(),
            Some("active-3")
        );
        assert_eq!(workspace.detail().task_id.as_deref(), Some("active-3"));
    }

    #[test]
    fn detail_state_change_focuses_newly_selected_table_row() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![
                task_with("active-1", "Active one", TaskState::InProgress),
                task_with("active-2", "Active two", TaskState::InProgress),
                task_with("active-3", "Active three", TaskState::InProgress),
            ],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        select_workspace_task(&mut workspace, "active-2");
        workspace
            .detail_mut()
            .patches
            .borrow_mut()
            .push(TaskPatch::State(TaskState::Done));
        let mut ctx = EventCtx::default();

        workspace.event(&TuiEvent::Key(Key::Char('~').into()), &mut ctx);

        assert_eq!(
            workspace.table().highlighted_id().as_deref(),
            Some("active-3")
        );
        assert_eq!(
            ctx.focus_request(),
            Some(&FocusRequest::Target(FocusId::new("data-view")))
        );
    }

    #[test]
    fn state_change_for_last_task_selects_previous_visible_task() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![
                task_with("active-1", "Active one", TaskState::InProgress),
                task_with("active-2", "Active two", TaskState::InProgress),
                task_with("active-3", "Active three", TaskState::InProgress),
            ],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        select_workspace_task(&mut workspace, "active-3");

        store.borrow_mut().dispatch(AppEvent::PatchTask {
            task_id: "active-3".to_string(),
            patch: TaskPatch::State(TaskState::Done),
        });
        workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());

        assert_eq!(
            workspace.table().highlighted_id().as_deref(),
            Some("active-2")
        );
        assert_eq!(workspace.detail().task_id.as_deref(), Some("active-2"));
    }

    #[test]
    fn detail_state_change_with_no_remaining_tasks_clears_detail() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![task_with("active-1", "Active one", TaskState::InProgress)],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let area = Rect::new(0, 0, 100, 30);
        workspace.layout(area, &mut LayoutCtx::new());

        workspace
            .detail_mut()
            .patches
            .borrow_mut()
            .push(TaskPatch::State(TaskState::Done));
        assert!(workspace.sync_detail_changes().changed);
        workspace.layout(area, &mut LayoutCtx::new());

        let text = rendered_text(&workspace, area);
        assert!(text.contains("No results found."));
        assert!(text.contains("No task selected."));
        assert_eq!(workspace.table().highlighted_id(), None);
        assert_eq!(workspace.detail().task_id, None);
    }

    #[test]
    fn task_table_ignores_data_view_filter_mode_hotkey() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);

        let outcome = workspace
            .table_mut()
            .on_key(KeyEvent::from(Key::Char('f')), Rect::new(0, 0, 80, 20));

        assert!(!outcome.handled);
        assert!(!outcome.changed);
        assert!(workspace.table().transform_state().filters.is_empty());
    }

    #[test]
    fn task_table_hides_column_headers() {
        let table = task_table(
            vec![task_with("work", "Write report", TaskState::Todo)],
            Some("work"),
        );

        let text = rendered_text(&table, Rect::new(0, 0, 80, 10));

        assert!(!text.contains("Size"));
        assert!(!text.contains("Task"));
        assert!(text.contains("Write report"));
    }

    #[test]
    fn hidden_task_cannot_be_deleted_from_empty_in_progress_view() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![task_with("todo", "Todo work", TaskState::Todo)],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        workspace.table_focused = true;
        let mut ctx = EventCtx::default();

        let outcome = workspace.event(&TuiEvent::Key(KeyEvent::from(Key::Delete)), &mut ctx);

        assert_eq!(outcome, EventOutcome::Ignored);
        assert!(ctx.messages().is_empty());
    }

    #[test]
    fn created_todo_switches_hidden_view_to_all_and_selects_task() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        workspace.layout(Rect::new(0, 0, 120, 40), &mut LayoutCtx::new());
        let created = Task::quick_capture(
            "task-2".to_string(),
            "Captured".to_string(),
            String::new(),
            TaskSize::Small,
        );

        store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(created.clone()));
        workspace.layout(Rect::new(0, 0, 120, 40), &mut LayoutCtx::new());

        assert_eq!(
            store.borrow().state().selected_task_id.as_deref(),
            Some("task-2")
        );
        assert_eq!(workspace.task_view, TaskView::All);
        assert_eq!(*workspace.active_task_view.borrow(), TaskView::All);
        assert_eq!(
            workspace.table_mut().highlighted_id().as_deref(),
            Some("task-2")
        );
        assert_eq!(
            workspace.table_mut().selected_id().as_deref(),
            Some("task-2")
        );
        assert_eq!(workspace.detail_mut().task_id.as_deref(), Some("task-2"));
    }

    #[test]
    fn created_todo_stays_in_todo_view_and_selects_task() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        *workspace.pending_task_view.borrow_mut() = Some(TaskView::Todo);
        workspace.sync_task_view_change();
        workspace.layout(Rect::new(0, 0, 120, 40), &mut LayoutCtx::new());
        let created = Task::quick_capture(
            "task-2".to_string(),
            "Captured".to_string(),
            String::new(),
            TaskSize::Small,
        );

        store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(created.clone()));
        workspace.layout(Rect::new(0, 0, 120, 40), &mut LayoutCtx::new());

        assert_eq!(workspace.task_view, TaskView::Todo);
        assert_eq!(
            workspace.table().highlighted_id().as_deref(),
            Some("task-2")
        );
        assert_eq!(workspace.detail().task_id.as_deref(), Some("task-2"));
    }

    #[test]
    fn escape_keeps_task_table_focused_as_tab_root() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        workspace.table_focused = true;
        let mut ctx = EventCtx::default();

        let outcome = workspace.event(&TuiEvent::Key(KeyEvent::from(Key::Esc)), &mut ctx);

        assert!(outcome.handled());
        assert_eq!(ctx.propagation(), Propagation::Stopped);
    }

    #[test]
    fn delete_opens_confirmation_from_focused_task_table() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        workspace.table_focused = true;
        let mut ctx = EventCtx::default();

        let outcome = workspace.event(&TuiEvent::Key(KeyEvent::from(Key::Delete)), &mut ctx);

        assert!(outcome.handled());
        assert!(matches!(
            ctx.messages(),
            [AppMsg::OpenDeleteTask(task_id)] if task_id == "task-1"
        ));
    }

    #[test]
    fn snooze_opens_only_from_focused_table_with_visible_selection() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let event = TuiEvent::Key(KeyEvent::from(Key::Char('b')));

        let mut unfocused = EventCtx::default();
        assert_eq!(
            workspace.event(&event, &mut unfocused),
            EventOutcome::Ignored
        );
        assert!(unfocused.messages().is_empty());

        workspace.table_focused = true;
        let mut focused = EventCtx::default();
        assert!(workspace.event(&event, &mut focused).handled());
        assert!(matches!(
            focused.messages(),
            [AppMsg::OpenTaskSnooze(task_id)] if task_id == "task-1"
        ));

        *workspace.visible_selection.borrow_mut() = None;
        let mut hidden = EventCtx::default();
        assert_eq!(workspace.event(&event, &mut hidden), EventOutcome::Ignored);
        assert!(hidden.messages().is_empty());
    }

    #[test]
    fn snooze_dialog_uses_open_filled_dropdown_and_quick_selection_message() {
        let now = time::macros::datetime!(2026-07-23 12:00);
        let mut dialog = SnoozeDialog::new("task-1".into(), now, None, false);
        let hint = dialog.measure(LayoutProposal::unbounded());
        assert_eq!(hint.preferred, tuicore::LayoutSize::new(46, 12));

        let mut ctx = EventCtx::default();
        dialog.event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
        assert!(matches!(
            ctx.messages(),
            [AppMsg::SnoozeTask { task_id, until, remember_custom: None }]
                if task_id == "task-1" && *until == time::macros::datetime!(2026-07-24 8:00)
        ));
    }

    #[test]
    fn snooze_dialog_renders_search_and_options_through_modal_portals() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut app = App::new(context.store, context.coordinator);
        let mut activation = EventCtx::default();
        let primary = app.primary_dialog();
        primary.replace_layer(
            AppDialog::Snooze(Box::new(SnoozeDialog::new(
                "task-1".into(),
                time::macros::datetime!(2026-07-23 12:00),
                None,
                false,
            ))),
            &mut activation,
        );
        primary.set_fit_content(true);
        primary.set_active_with_context(true, &mut activation);
        let area = Rect::new(0, 0, 100, 30);
        app.layout(area, &mut LayoutCtx::new());
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))
            .expect("terminal should build");
        terminal
            .draw(|frame| {
                let mut ctx = RenderCtx::new();
                app.render(frame, area, &mut ctx);
                ctx.flush(frame);
            })
            .expect("app should render");
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("Search..."));
        assert!(text.contains("Tomorrow"));
        assert!(text.contains("This weekend"));
        assert!(text.contains("Pick date & time"));
    }

    #[test]
    fn snoozed_detail_places_datetime_field_after_end_date_and_queues_selection() {
        let until = time::macros::datetime!(2026-07-24 8:00);
        let mut task = test_task();
        task.state = TaskState::Snoozed;
        task.snoozed_until = Some(until);
        let mut detail = TaskDetailForm::new(Some(&task), &[], &[], &[], None);
        let area = Rect::new(0, 0, 80, 120);
        detail.layout(area, &mut LayoutCtx::new());
        let text = rendered_text(&detail, area);
        let end_date = text.find("End date").expect("end date should render");
        let snoozed_until = text
            .find("Snoozed until")
            .expect("snoozed datetime should render");
        assert!(end_date < snoozed_until);

        let mut layout = LayoutCtx::new();
        detail.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.id.as_str() == "date-time-picker-dropdown")
            .expect("snoozed datetime should be focusable")
            .clone();
        assert_eq!(target.hotkey_sequences, ["su"]);
        let mut focus = FocusManager::new();
        let transition = focus
            .apply_request(
                &FocusRequest::TargetAt {
                    path: target.path.clone(),
                    id: target.id.clone(),
                },
                layout.focus_targets(),
            )
            .expect("datetime focus should apply");
        let mut dispatcher = TreeDispatcher::new();
        dispatcher.dispatch_focus(&mut detail, transition, AnimationSettings::default());
        let route = EventRoute::new(focus.current_path());
        for key in [Key::Enter, Key::Right, Key::Enter, Key::Enter] {
            let effects = dispatcher.dispatch_event(
                &mut detail,
                &route,
                &TuiEvent::Key(key.into()),
                AnimationSettings::default(),
            );
            assert!(effects.outcome.handled());
        }

        assert!(matches!(
            detail.take_patches().as_slice(),
            [(task_id, TaskPatch::Snooze { until, remember_custom: None })]
                if task_id == "task-1" && *until == time::macros::datetime!(2026-07-25 8:00)
        ));

        task.state = TaskState::Todo;
        task.snoozed_until = None;
        let mut active_detail = TaskDetailForm::new(Some(&task), &[], &[], &[], None);
        active_detail.layout(area, &mut LayoutCtx::new());
        assert!(!rendered_text(&active_detail, area).contains("Snoozed until"));
    }

    #[test]
    fn snoozed_until_hotkey_opens_detail_picker() {
        let mut task = test_task();
        task.state = TaskState::Snoozed;
        task.snoozed_until = Some(time::macros::datetime!(2026-07-24 8:00));
        let mut detail = TaskDetailForm::new(Some(&task), &[], &[], &[], None);
        let area = Rect::new(0, 0, 80, 120);
        let mut layout = LayoutCtx::new();
        detail.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.id.as_str() == "date-time-picker-dropdown")
            .expect("snoozed datetime should be focusable");

        let effects = TreeDispatcher::new().dispatch_event(
            &mut detail,
            &EventRoute::new(target.path.clone()),
            &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_SNOOZED_UNTIL_FIELD.hotkey())),
            AnimationSettings::default(),
        );

        assert!(effects.outcome.handled());
        assert!(effects.layout);
    }

    #[test]
    fn active_snooze_modal_receives_routed_navigation_and_selection() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut app = App::new(context.store, context.coordinator);
        let mut activation = EventCtx::default();
        let primary = app.primary_dialog();
        primary.replace_layer(
            AppDialog::Snooze(Box::new(SnoozeDialog::new(
                "task-1".into(),
                time::macros::datetime!(2026-07-23 12:00),
                None,
                false,
            ))),
            &mut activation,
        );
        primary.set_fit_content(true);
        primary.set_active_with_context(true, &mut activation);
        let area = Rect::new(0, 0, 100, 30);
        let mut layout = LayoutCtx::new();
        app.layout(area, &mut layout);
        let menu = layout
            .focus_targets()
            .iter()
            .rev()
            .find(|target| target.id.as_str() == "input")
            .expect("active modal dropdown search should be focusable")
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
            .expect("modal menu focus should apply");
        dispatcher.dispatch_focus(&mut app, transition, AnimationSettings::default());
        let route = EventRoute::new(focus.current_path());
        let down = dispatcher.dispatch_event(
            &mut app,
            &route,
            &TuiEvent::Key(KeyEvent {
                code: Key::Char('j'),
                modifiers: KeyModifiers::CONTROL,
            }),
            AnimationSettings::default(),
        );
        assert!(down.outcome.handled());
        let select = dispatcher.dispatch_event(
            &mut app,
            &route,
            &TuiEvent::Key(Key::Enter.into()),
            AnimationSettings::default(),
        );
        assert!(matches!(
            select.messages.as_slice(),
            [AppMsg::SnoozeTask {
                task_id,
                until,
                remember_custom: None
            }] if task_id == "task-1" && *until == time::macros::datetime!(2026-07-25 8:00)
        ));
    }

    #[test]
    fn backspace_shortcuts_do_not_open_task_dialogs() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        workspace.table_focused = true;
        for modifiers in [KeyModifiers::NONE, KeyModifiers::CONTROL] {
            let mut ctx = EventCtx::default();
            workspace.event(
                &TuiEvent::Key(KeyEvent {
                    code: Key::Backspace,
                    modifiers,
                }),
                &mut ctx,
            );
            assert!(ctx.messages().is_empty());
        }
    }

    #[test]
    fn x_targets_visible_task_even_when_store_selection_is_stale() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![
                test_task(),
                task_with("task-2", "Second", TaskState::InProgress),
            ],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        workspace.table_focused = true;
        workspace.table_mut().highlight_id(&"task-2".to_string());
        workspace.table_mut().select_id("task-2".to_string());
        workspace.table_mut().take_events();
        *workspace.visible_selection.borrow_mut() = Some("task-2".to_string());
        let mut ctx = EventCtx::default();

        let outcome = workspace.event(&TuiEvent::Key(KeyEvent::from(Key::Char('x'))), &mut ctx);

        assert!(outcome.handled());
        assert!(matches!(
            ctx.messages(),
            [AppMsg::OpenDeleteTask(task_id)] if task_id == "task-2"
        ));
    }

    #[test]
    fn ctrl_x_opens_confirmation_from_focused_task_table() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        workspace.table_focused = true;
        let mut ctx = EventCtx::default();
        let key = KeyEvent {
            code: Key::Char('x'),
            modifiers: KeyModifiers::CONTROL,
        };

        let outcome = workspace.event(&TuiEvent::Key(key), &mut ctx);

        assert!(outcome.handled());
        assert!(matches!(
            ctx.messages(),
            [AppMsg::OpenDeleteTask(task_id)] if task_id == "task-1"
        ));
    }

    #[test]
    fn completed_task_moves_from_in_progress_to_archived_view() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let area = Rect::new(0, 0, 120, 40);
        workspace.layout(area, &mut LayoutCtx::new());

        store.borrow_mut().dispatch(AppEvent::PatchTask {
            task_id: "task-1".to_string(),
            patch: TaskPatch::State(TaskState::Done),
        });
        workspace.layout(area, &mut LayoutCtx::new());

        let text = rendered_text(&workspace, area);
        assert!(!text.contains("Original"));
        assert!(text.contains("No results found."));

        *workspace.pending_task_view.borrow_mut() = Some(TaskView::Archived);
        assert!(workspace.sync_task_view_change());
        workspace.layout(area, &mut LayoutCtx::new());

        let text = rendered_text(&workspace, area);
        assert!(text.contains("Original"));
        assert!(text.contains("Done"));
    }

    #[test]
    fn confirmed_delete_removes_task_from_state_immediately() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut app = App::new(context.store, context.coordinator);

        app.delete_task("task-1".to_string(), &mut EventCtx::default());

        assert!(app.context.store.borrow().state().tasks.is_empty());
    }

    #[test]
    fn management_create_dialog_layers_over_management_workspace() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut app = App::new(context.store, context.coordinator);
        let mut ctx = EventCtx::default();

        app.open_management_dialog(ManagementDialogKind::People, &mut ctx);
        app.open_create_management_dialog(ManagementDialogKind::People, &mut ctx);

        assert!(app.root.is_active());
        assert!(app.root.base().is_active());
        assert!(matches!(app.root.layer(), AppDialog::CreateManagement(_)));
        assert!(matches!(app.root.base().layer(), AppDialog::People(_)));

        app.close_management_overlay(&mut ctx);

        assert!(!app.root.is_active());
        assert!(app.root.base().is_active());
        assert!(matches!(app.root.base().layer(), AppDialog::People(_)));
    }

    #[test]
    fn delete_confirmation_uses_d_shortcut() {
        let mut dialog = delete_task_dialog(&test_task());
        let mut ctx = EventCtx::default();

        let outcome = dialog.event(&TuiEvent::Key(KeyEvent::from(Key::Char('d'))), &mut ctx);

        assert!(outcome.handled());
        assert!(matches!(
            ctx.messages(),
            [AppMsg::DeleteTaskConfirmed(task_id)] if task_id == "task-1"
        ));

        let mut dialog = delete_task_dialog(&test_task());
        let mut old_shortcut_ctx = EventCtx::default();
        dialog.event(
            &TuiEvent::Key(KeyEvent::from(Key::Char('o'))),
            &mut old_shortcut_ctx,
        );
        assert!(old_shortcut_ctx.messages().is_empty());
    }

    #[test]
    fn delete_task_dialog_fits_its_content() {
        let snapshot = WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        };
        let (_runtime, context, _store) = test_context(snapshot);
        let mut app = App::new(context.store, context.coordinator);
        let area = Rect::new(0, 0, 120, 40);

        app.open_delete_task_dialog("task-1", &mut EventCtx::default());
        let mut delete_layout = LayoutCtx::new();
        app.layout(area, &mut delete_layout);
        let delete_area = delete_layout
            .overlays()
            .first()
            .expect("delete dialog should register an overlay")
            .area;

        assert!(delete_area.width > 20);
        assert!(delete_area.height >= 3);
        assert!(delete_area.width < area.width / 2);
        assert!(delete_area.height < area.height / 4);
    }

    #[test]
    fn create_task_dialog_fits_its_content_height() {
        let snapshot = WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        };
        let (_runtime, context, _store) = test_context(snapshot);
        let mut app = App::new(context.store, context.coordinator);
        let area = Rect::new(0, 0, 120, 40);

        app.open_create_task_dialog(&mut EventCtx::default());
        let mut layout = LayoutCtx::new();
        app.layout(area, &mut layout);
        let dialog_area = layout
            .overlays()
            .first()
            .expect("create task dialog should register an overlay")
            .area;

        assert_eq!(dialog_area.width, 80);
        assert_eq!(dialog_area.height, 14);
    }

    #[test]
    fn creation_dialogs_close_from_nested_control_focus_mode() {
        let area = Rect::new(0, 0, 80, 24);
        let cases = [
            (
                create_management_dialog_host(ManagementDialogKind::Projects),
                KeyEvent::from(Key::Esc),
                true,
            ),
            (
                create_task_dialog_host(),
                KeyEvent {
                    code: Key::Char('['),
                    modifiers: KeyModifiers::CONTROL,
                },
                false,
            ),
        ];

        for (mut dialog, key, management) in cases {
            let mut layout = LayoutCtx::new();
            dialog.layout(area, &mut layout);
            let target = layout
                .focus_targets()
                .iter()
                .find(|target| target.id.as_str() == "textarea")
                .expect("creation textarea should be focusable")
                .clone();
            let mut ctx = EventCtx::default();

            let outcome =
                dialog.dispatch_event(&EventRoute::new(target.path), &TuiEvent::Key(key), &mut ctx);

            assert!(outcome.handled());
            if management {
                assert!(matches!(ctx.messages(), [AppMsg::CloseManagementOverlay]));
            } else {
                assert!(matches!(ctx.messages(), [AppMsg::CloseDialog]));
            }
        }
    }

    #[test]
    fn management_dialogs_close_from_nested_control_focus_mode() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: vec![Person::new("person-1".into(), "Ada".into(), String::new())],
            projects: vec![Project::new(
                "project-1".into(),
                "CORE".into(),
                "Core".into(),
                String::new(),
            )],
            tags: vec![Tag::new("tag-1".into(), "api".into())],
        });
        let area = Rect::new(0, 0, 100, 30);
        let cases = [
            (ManagementDialogKind::People, KeyEvent::from(Key::Esc)),
            (
                ManagementDialogKind::Projects,
                KeyEvent {
                    code: Key::Char('['),
                    modifiers: KeyModifiers::CONTROL,
                },
            ),
            (ManagementDialogKind::Tags, KeyEvent::from(Key::Esc)),
        ];

        for (kind, key) in cases {
            let mut dialog = management_dialog(context.clone(), kind);
            let mut layout = LayoutCtx::new();
            dialog.layout(area, &mut layout);
            let target = layout
                .focus_targets()
                .iter()
                .find(|target| target.id.as_str() == "input")
                .expect("management detail input should be focusable")
                .clone();
            let mut ctx = EventCtx::default();

            let outcome =
                dialog.dispatch_event(&EventRoute::new(target.path), &TuiEvent::Key(key), &mut ctx);

            assert!(outcome.handled(), "{kind:?} should close");
            assert!(matches!(ctx.messages(), [AppMsg::CloseDialog]));
        }
    }

    #[test]
    fn created_task_state_hotkey_focuses_open_dropdown() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let area = Rect::new(0, 0, 120, 40);
        workspace.layout(area, &mut LayoutCtx::new());
        store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(Task::quick_capture(
                "task-2".to_string(),
                "Captured".to_string(),
                String::new(),
                TaskSize::Small,
            )));
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let task_state = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target.id.as_str() == "field"
                    && target.path.keys().iter().any(|key| key.as_str() == "state")
            })
            .expect("task state should be focusable")
            .clone();
        let mut dispatcher = TreeDispatcher::new();

        let effects = dispatcher.dispatch_event(
            &mut workspace,
            &EventRoute::new(task_state.path),
            &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_STATE_FIELD.hotkey())),
            AnimationSettings::default(),
        );

        assert!(effects.layout);
        let focus_request = effects
            .focus_request
            .as_ref()
            .expect("state hotkey should request dropdown search focus");
        assert!(matches!(
            focus_request,
            FocusRequest::TargetAt { id, .. } if id.as_str() == "input"
        ));
        let mut open_layout = LayoutCtx::new();
        workspace.layout(area, &mut open_layout);
        let mut focus = FocusManager::new();
        let transition = focus
            .apply_request(focus_request, open_layout.focus_targets())
            .expect("open dropdown search should accept focus");
        dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());

        assert_eq!(
            focus.current().map(|target| target.id.as_str()),
            Some("input")
        );
        assert!(rendered_text(&workspace, area).contains("Search..."));
    }

    #[test]
    fn task_state_switcher_excludes_snoozed() {
        let choice_ids = state_choices()
            .into_iter()
            .map(|choice| choice.id)
            .collect::<Vec<_>>();

        assert_eq!(choice_ids, ["todo", "in_progress", "done", "rejected"]);
    }

    #[test]
    fn task_description_registers_edit_and_editor_hotkeys_and_requests_existing_value() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let area = Rect::new(0, 0, 120, 40);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let description = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target.id.as_str() == "textarea"
                    && target
                        .path
                        .keys()
                        .iter()
                        .any(|key| key.as_str() == "description")
            })
            .expect("description input should be focusable");

        assert_eq!(description.hotkey_sequences, ["dd", "do"]);

        let effects = TreeDispatcher::new().dispatch_event(
            &mut workspace,
            &EventRoute::new(description.path.clone()),
            &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_DESCRIPTION_EDITOR.hotkey())),
            AnimationSettings::default(),
        );

        assert_eq!(
            effects
                .external_editor
                .expect("editor hotkey should request external editor")
                .value,
            "Existing detail"
        );
    }

    #[test]
    fn title_blur_during_description_hotkey_preserves_description_focus() {
        sqlx::any::install_default_drivers();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime should build");
        let _runtime_guard = runtime.enter();
        let pool = AnyPoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .expect("lazy pool should build");
        let store = Rc::new(RefCell::new(Store::new(
            AppState::from_snapshot(WorkspaceSnapshot {
                tasks: vec![Task {
                    id: "task-1".to_string(),
                    title: "Original".to_string(),
                    state: TaskState::InProgress,
                    size: TaskSize::Small,
                    priority: TaskPriority::Medium,
                    start_date: None,
                    due_date: None,
                    snoozed_until: None,
                    people_ids: Vec::new(),
                    project_ids: Vec::new(),
                    tag_ids: Vec::new(),
                    detail: "Existing detail".to_string(),
                }],
                people: Vec::new(),
                projects: Vec::new(),
                tags: Vec::new(),
            }),
            reduce_app_state as fn(&mut AppState, AppEvent) -> tuicore::DispatchOutcome,
        )));
        let coordinator = Rc::new(RefCell::new(PersistenceCoordinator::new(
            Rc::clone(&store),
            pool,
            crate::storage::SqlDialect::Sqlite,
            runtime.handle().clone(),
            None,
        )));
        let mut workspace = TaskWorkspace::new(AppContext {
            store: Rc::clone(&store),
            coordinator,
        });
        let area = Rect::new(0, 0, 120, 40);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let title = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target.id.as_str() == "input"
                    && target.path.keys().iter().any(|key| key.as_str() == "title")
            })
            .expect("title input should be focusable")
            .clone();
        let description = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target.id.as_str() == "textarea"
                    && target
                        .path
                        .keys()
                        .iter()
                        .any(|key| key.as_str() == "description")
            })
            .expect("description input should be focusable")
            .clone();
        let mut focus = FocusManager::new();
        let mut dispatcher = TreeDispatcher::new();
        let transition = focus
            .apply_request(
                &FocusRequest::TargetAt {
                    path: title.path.clone(),
                    id: title.id.clone(),
                },
                layout.focus_targets(),
            )
            .expect("title focus should change");
        dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());

        let title_route = EventRoute::new(title.path);
        for key in [Key::Enter, Key::Char('!'), Key::Esc] {
            let effects = dispatcher.dispatch_event(
                &mut workspace,
                &title_route,
                &TuiEvent::Key(key.into()),
                AnimationSettings::default(),
            );
            assert_eq!(effects.outcome, EventOutcome::Handled);
        }

        let description_route = EventRoute::new(description.path.clone());
        let hotkey_effects = dispatcher.dispatch_event(
            &mut workspace,
            &description_route,
            &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_DESCRIPTION_FIELD.hotkey())),
            AnimationSettings::default(),
        );
        assert!(hotkey_effects.layout);

        let mut first_transition_layout = LayoutCtx::new();
        workspace.layout(area, &mut first_transition_layout);
        let transition = focus
            .apply_request(
                &FocusRequest::TargetAt {
                    path: description.path.clone(),
                    id: description.id.clone(),
                },
                first_transition_layout.focus_targets(),
            )
            .expect("description focus should change");
        let focus_effects =
            dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
        assert!(focus_effects.layout);

        let mut post_transition_layout = LayoutCtx::new();
        workspace.layout(area, &mut post_transition_layout);
        assert!(
            focus
                .validate(post_transition_layout.focus_targets())
                .is_none()
        );

        let store_ref = store.borrow();
        assert_eq!(store_ref.state().tasks[0].title, "Original!");
        drop(store_ref);

        let printable_effects = dispatcher.dispatch_event(
            &mut workspace,
            &EventRoute::new(focus.current_path()),
            &TuiEvent::Key(Key::Char('x').into()),
            AnimationSettings::default(),
        );
        assert_eq!(printable_effects.outcome, EventOutcome::Handled);

        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))
            .expect("terminal should build");
        terminal
            .draw(|frame| workspace.render(frame, area, &mut RenderCtx::new()))
            .expect("workspace should render");
        let buffer = terminal.backend().buffer();
        let mut rendered_table = String::new();
        for y in 0..area.height {
            for x in 0..70 {
                rendered_table.push_str(
                    buffer
                        .cell((x, y))
                        .expect("table cell should exist")
                        .symbol(),
                );
            }
        }
        assert!(rendered_table.contains("Original!"));
    }

    #[test]
    fn save_failure_and_recovery_preserve_focused_task_description_state() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let area = Rect::new(0, 0, 120, 40);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let description = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target.id.as_str() == "textarea"
                    && target
                        .path
                        .keys()
                        .iter()
                        .any(|key| key.as_str() == "description")
            })
            .expect("description should be focusable")
            .clone();
        let mut focus = FocusManager::new();
        let mut dispatcher = TreeDispatcher::new();
        let transition = focus
            .apply_request(
                &FocusRequest::TargetAt {
                    path: description.path.clone(),
                    id: description.id.clone(),
                },
                layout.focus_targets(),
            )
            .expect("description focus should change");
        dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
        for key in [Key::Enter, Key::Char('x')] {
            assert_eq!(
                dispatcher
                    .dispatch_event(
                        &mut workspace,
                        &EventRoute::new(focus.current_path()),
                        &TuiEvent::Key(key.into()),
                        AnimationSettings::default(),
                    )
                    .outcome,
                EventOutcome::Handled
            );
        }
        assert!(rendered_area_has_focus_style(
            &workspace,
            area,
            description.area
        ));

        store.borrow_mut().dispatch(AppEvent::SaveCompleted {
            target: SaveTarget::task("task-1".to_string(), TaskField::Detail),
            error: Some("offline".to_string()),
        });
        let mut failed_layout = LayoutCtx::new();
        workspace.layout(area, &mut failed_layout);
        assert!(focus.validate(failed_layout.focus_targets()).is_none());
        assert!(rendered_text(&workspace, area).contains("Save failed for task-1"));
        assert!(rendered_area_has_focus_style(
            &workspace,
            area,
            description.area
        ));

        let after_failure = dispatcher.dispatch_event(
            &mut workspace,
            &EventRoute::new(focus.current_path()),
            &TuiEvent::Key(Key::Char('y').into()),
            AnimationSettings::default(),
        );
        assert_eq!(after_failure.outcome, EventOutcome::Handled);

        store.borrow_mut().dispatch(AppEvent::SaveCompleted {
            target: SaveTarget::task("task-1".to_string(), TaskField::Detail),
            error: None,
        });
        let mut recovered_layout = LayoutCtx::new();
        workspace.layout(area, &mut recovered_layout);
        assert!(focus.validate(recovered_layout.focus_targets()).is_none());
        assert!(rendered_area_has_focus_style(
            &workspace,
            area,
            description.area
        ));

        let tab = dispatcher.dispatch_event(
            &mut workspace,
            &EventRoute::new(focus.current_path()),
            &TuiEvent::Key(Key::Tab.into()),
            AnimationSettings::default(),
        );
        let transition = focus
            .apply_request(
                tab.focus_request.as_ref().unwrap_or(&FocusRequest::Next),
                recovered_layout.focus_targets(),
            )
            .expect("tab should move focus");
        dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
        let back_tab = dispatcher.dispatch_event(
            &mut workspace,
            &EventRoute::new(focus.current_path()),
            &TuiEvent::Key(Key::BackTab.into()),
            AnimationSettings::default(),
        );
        let transition = focus
            .apply_request(
                back_tab
                    .focus_request
                    .as_ref()
                    .unwrap_or(&FocusRequest::Previous),
                recovered_layout.focus_targets(),
            )
            .expect("shift-tab should restore description focus");
        dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
        assert_eq!(
            focus
                .current()
                .expect("focus should remain set")
                .id
                .as_str(),
            "textarea"
        );
        for key in [Key::Enter, Key::Char('z')] {
            assert_eq!(
                dispatcher
                    .dispatch_event(
                        &mut workspace,
                        &EventRoute::new(focus.current_path()),
                        &TuiEvent::Key(key.into()),
                        AnimationSettings::default(),
                    )
                    .outcome,
                EventOutcome::Handled
            );
        }
    }

    #[test]
    fn task_dropdown_save_completion_tabs_to_next_control_without_reset() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let area = Rect::new(0, 0, 120, 40);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let task_size = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target.id.as_str() == "field"
                    && target.path.keys().iter().any(|key| key.as_str() == "size")
            })
            .expect("task size should be focusable")
            .clone();
        let mut focus = FocusManager::new();
        let mut dispatcher = TreeDispatcher::new();
        let transition = focus
            .apply_request(
                &FocusRequest::TargetAt {
                    path: task_size.path.clone(),
                    id: task_size.id.clone(),
                },
                layout.focus_targets(),
            )
            .expect("size focus should change");
        dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
        workspace
            .detail_mut()
            .patches
            .borrow_mut()
            .push(TaskPatch::Size(TaskSize::Big));
        assert!(workspace.sync_detail_changes().changed);
        assert_eq!(store.borrow().state().tasks[0].size, TaskSize::Big);

        store.borrow_mut().dispatch(AppEvent::SaveCompleted {
            target: SaveTarget::task("task-1".to_string(), TaskField::Size),
            error: Some("offline".to_string()),
        });
        let mut post_save_layout = LayoutCtx::new();
        workspace.layout(area, &mut post_save_layout);
        assert!(focus.validate(post_save_layout.focus_targets()).is_none());

        let tab = dispatcher.dispatch_event(
            &mut workspace,
            &EventRoute::new(focus.current_path()),
            &TuiEvent::Key(Key::Tab.into()),
            AnimationSettings::default(),
        );
        let transition = focus
            .apply_request(
                tab.focus_request.as_ref().unwrap_or(&FocusRequest::Next),
                post_save_layout.focus_targets(),
            )
            .expect("tab should move to priority");
        dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
        assert!(
            focus
                .current()
                .expect("next control should be focused")
                .path
                .keys()
                .iter()
                .any(|key| key.as_str() == "priority")
        );
        assert_eq!(store.borrow().state().tasks[0].size, TaskSize::Big);
    }

    #[test]
    fn management_routing_builds_concrete_dialog_variant() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });

        assert!(matches!(
            management_dialog(context, ManagementDialogKind::Projects),
            AppDialog::Projects(_)
        ));
    }
}
