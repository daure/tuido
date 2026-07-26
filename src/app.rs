use std::{cell::RefCell, error::Error, rc::Rc, time::Duration};

use crate::app_keymap::{self, keys};
use crate::calendar::{CalendarWorkspace, SHOW_WEEKENDS_SETTING, parse_show_weekends_setting};
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
use crate::task_title::format_title;
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
    TagInputEvent, TextInput, TextareaInput, TickResult, TreeApp, TreePath, TuiEvent, TuiNode,
    WeatherProviderConfig,
};
use uuid::Uuid;

mod task_copy;
mod task_links_input;

use task_copy::TaskCopyContext;
use task_links_input::TaskLinksInput;

const PEOPLE_MENU_ID: &str = "people";
const PROJECTS_MENU_ID: &str = "projects";
const TAGS_MENU_ID: &str = "tags";
const STATUS_BAR_MENU_ITEMS: [StatusBarMenuItem; 5] = [
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
    StatusBarMenuItem::WeatherForecast,
];

fn weather_provider_config() -> WeatherProviderConfig {
    WeatherProviderConfig::new().enabled(true)
}

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
    let service = TuidoService::from_storage(&storage);
    let startup_expiry_error = runtime
        .block_on(service.process_snooze_expirations())
        .err()
        .map(|error| format!("Snooze expiry processing failed: {error}"));
    let workspace = runtime.block_on(service.consistent_workspace())?;
    let show_calendar_weekends = parse_show_weekends_setting(
        runtime
            .block_on(service.app_setting(SHOW_WEEKENDS_SETTING))?
            .as_deref(),
    )
    .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidData, message))?;
    let mut app_state = AppState::from_snapshot(workspace.snapshot);
    app_state.app_setting_values.insert(
        SHOW_WEEKENDS_SETTING.to_string(),
        show_calendar_weekends.to_string(),
    );
    app_state.app_setting_confirmed_values.insert(
        SHOW_WEEKENDS_SETTING.to_string(),
        show_calendar_weekends.to_string(),
    );
    app_state.app_setting_desired_values.insert(
        SHOW_WEEKENDS_SETTING.to_string(),
        show_calendar_weekends.to_string(),
    );
    app_state.refresh_error = startup_expiry_error;
    app_state.workspace_revision = workspace.revision;
    app_state.entity_revisions = workspace.entity_revisions;
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
    let run_result = TreeApp::new(App::new_with_calendar_weekends(
        store,
        Rc::clone(&coordinator),
        show_calendar_weekends,
    ))
    .initial_focus(initial_task_table_focus_request())
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
    #[cfg(test)]
    fn new(store: AppStore, coordinator: Rc<RefCell<PersistenceCoordinator>>) -> Self {
        Self::new_with_calendar_weekends(store, coordinator, true)
    }

    fn new_with_calendar_weekends(
        store: AppStore,
        coordinator: Rc<RefCell<PersistenceCoordinator>>,
        show_calendar_weekends: bool,
    ) -> Self {
        let context = AppContext { store, coordinator };
        let tabs = Tabs::new(vec![
            Tab::new("Tasks", TaskWorkspace::new(context.clone()))
                .hotkey(keys::APP_TASKS_TAB.hotkey()),
            Tab::new(
                "Calendar",
                CalendarWorkspace::new(context.clone(), show_calendar_weekends),
            )
            .hotkey(keys::APP_CALENDAR_TAB.hotkey()),
        ])
        .selected(0)
        .variant(TabsVariant::Underline)
        .bordered(true);

        let root = Flex::column().child("tabs", tabs, FlexItem::fill(1)).child(
            "footer",
            StatusBar::new()
                .menu_items(STATUS_BAR_MENU_ITEMS)
                .weather_provider(weather_provider_config())
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
        let title = format_title(&draft.title);
        if title.is_empty() {
            ctx.notify(tuicore::Notification::warning(
                "Task title required",
                "Enter a title before creating the task.",
            ));
            return;
        }

        let task = Task::quick_capture(
            Uuid::new_v4().to_string(),
            title,
            String::new(),
            TaskSize::Small,
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
        focus_task_table(ctx);
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
        focus_task_table(ctx);
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

fn initial_task_table_focus_request() -> FocusRequest {
    FocusRequest::TargetAt {
        path: TreePath::from_keys([
            ChildKey::first(),
            ChildKey::first(),
            ChildKey::new("tabs"),
            ChildKey::new("tab-0"),
            ChildKey::second(),
            ChildKey::first(),
        ]),
        id: FocusId::new("data-view"),
    }
}
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
        let rows = task_rows_for_view(&state.tasks, task_view);
        let selected_task_id = rows.first().map(|task| task.id.clone());
        let visible_task_ids = rows.iter().map(|task| task.id.clone()).collect();
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
        let rows = task_rows_for_view(&state.tasks, self.task_view);
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
            ctx.focus(initial_task_table_focus_request());
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
            ctx.focus(initial_task_table_focus_request());
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
            ctx.focus(initial_task_table_focus_request());
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
            Self::CreateTask(dialog) => measure_dialog_host(dialog, proposal),
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
    let rows = task_rows_for_view(&state.tasks, task_view);
    let selected = state.selected_task_id.as_deref().filter(|id| {
        state
            .tasks
            .iter()
            .any(|task| task.id == **id && task_view.contains(task))
    });
    let copy_context = TaskCopyContext::new(&state.people, &state.projects, &state.tags);
    let table = task_table_with_copy_context(rows, selected, copy_context);
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

fn task_rows_for_view(tasks: &[Task], task_view: TaskView) -> Vec<TaskRow> {
    let mut rows = tasks
        .iter()
        .rev()
        .filter(|task| task_view.contains(task))
        .cloned()
        .collect::<Vec<_>>();
    rows.sort_by_key(|task| match task.priority {
        TaskPriority::High => 0,
        TaskPriority::Medium => 1,
        TaskPriority::Low => 2,
    });
    rows
}

#[cfg(test)]
fn task_table(rows: Vec<TaskRow>, selected_id: Option<&str>) -> DataView<TaskRow, String> {
    task_table_with_copy_context(rows, selected_id, TaskCopyContext::default())
}

fn task_table_with_copy_context(
    rows: Vec<TaskRow>,
    selected_id: Option<&str>,
    copy_context: TaskCopyContext,
) -> DataView<TaskRow, String> {
    let mut table = DataView::new(rows, |row: &TaskRow| row.id.clone())
        .copy_with(move |row| copy_context.export(row))
        .action_bar(true)
        .filter_controls(false)
        .focused_events_before_global_hotkeys(false)
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
            .sortable(|row| match row.priority {
                TaskPriority::Low => "2".to_string(),
                TaskPriority::Medium => "1".to_string(),
                TaskPriority::High => "0".to_string(),
            })
            .filter_key(|row| row.priority.label().to_string()),
            Column::rich(
                "size",
                "Size",
                Constraint::Length(3),
                |row: &TaskRow, _: &CellContext<String>| {
                    chip_line(row.size.label(), row.size.role())
                },
            )
            .constrained()
            .filter_key(|row| row.size.label().to_string()),
            Column::text("title", "Task", Constraint::Fill(1), |row: &TaskRow| {
                row.title.clone()
            })
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
                .value(task.description.clone())
                .panel("Description")
                .hotkey(keys::TASK_DESCRIPTION_FIELD.hotkey())
                .editor_hotkey(keys::TASK_DESCRIPTION_EDITOR.hotkey())
                .on_edit_end({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| {
                        patch_sink.borrow_mut().push(TaskPatch::Description(value));
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
    form.child(
        "links",
        TaskLinksInput::new(task, Rc::clone(&patch_sink)),
        FlexItem::content(),
    )
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
    if detail_escape(event) {
        focus_task_table(ctx);
        return EventOutcome::Handled;
    }
    outcome
}

fn focus_task_table(ctx: &mut EventCtx<AppMsg>) {
    ctx.focus(initial_task_table_focus_request());
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
#[path = "app/tests.rs"]
pub(crate) mod tests;
