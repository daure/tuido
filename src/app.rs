use std::{cell::RefCell, collections::HashMap, error::Error, rc::Rc, sync::mpsc, time::Duration};

use crate::app_keymap::{self, keys};
use crate::create_task_dialog::{CreateTaskDialog, CreateTaskDraft};
use crate::domain::{
    AppEvent, AppState, Person, PersonField, PersonPatch, Project, ProjectField, ProjectPatch,
    SaveTarget, Tag, TagField, TagPatch, Task, TaskField, TaskPatch, TaskPriority, TaskSize,
    TaskState, WorkspaceSnapshot, reduce_app_state,
};
use crate::storage::{self, SqlDialect, Storage};
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
};
use sqlx::AnyPool;
use time::Date;
use tokio::runtime::Handle;
use tuicore::{
    ActivationMode, AnimationSettings, AxisProposal, Button, CellContext, ChildKey, ChipColorRole,
    Column, ConfirmationDialog, ConfirmationDialogOutcome, CrossAlign, DataView,
    DataViewTypedEvent, DatePickerDropdown, Dialog, DialogAction, DialogBackdrop, DialogHost,
    DialogLayer, Dropdown, DropdownCommitMode, DropdownSearchMode, EventCtx, EventOutcome,
    EventRoute, Flex, FlexItem, FocusCtx, FocusId, FocusRequest, FocusTarget, HotkeyLabelMode,
    KeySpec, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, Menu, MenuItem,
    MenuSearchMode, Paragraph, RenderCtx, SelectedTag, SelectionMode, SelectionTrigger, Separator,
    Split, StatusBar, StatusBarMenuItem, Store, Tab, Tabs, TabsVariant, TagInput, TagInputEvent,
    TextInput, TextareaInput, TickResult, TreeApp, TuiEvent, TuiNode,
};
use uuid::Uuid;

const PEOPLE_MENU_ID: &str = "people";
const PROJECTS_MENU_ID: &str = "projects";
const TAGS_MENU_ID: &str = "tags";

#[derive(Debug)]
pub(crate) enum AppMsg {
    Noop,
    OpenManagementDialog(ManagementDialogKind),
    OpenCreateTask,
    CreateTaskSubmitted(CreateTaskDraft),
    OpenDeleteTask,
    DeleteTaskConfirmed(String),
    OpenTaskDisposition,
    SetTaskState { task_id: String, state: TaskState },
    CloseDialog,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ManagementDialogKind {
    People,
    Projects,
    Tags,
}

impl ManagementDialogKind {
    fn title(self) -> &'static str {
        match self {
            Self::People => "People",
            Self::Projects => "Projects",
            Self::Tags => "Tags",
        }
    }
}

pub fn run() -> Result<(), Box<dyn Error>> {
    tuicore::try_init()?;
    app_keymap::try_init()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let storage = runtime.block_on(Storage::connect_from_env())?;
    runtime.block_on(storage.migrate())?;
    let snapshot = runtime.block_on(storage.load_workspace())?;
    TreeApp::new(App::new(
        snapshot,
        storage.pool(),
        storage.dialect(),
        runtime.handle().clone(),
    ))
    .initial_focus(FocusRequest::Target(FocusId::new("data-view")))
    .on_message(|app, message, ctx| match message {
        AppMsg::Noop => {}
        AppMsg::OpenManagementDialog(kind) => app.open_management_dialog(kind, ctx),
        AppMsg::OpenCreateTask => app.open_create_task_dialog(ctx),
        AppMsg::CreateTaskSubmitted(draft) => app.submit_create_task(draft, ctx),
        AppMsg::OpenDeleteTask => app.open_delete_task_dialog(ctx),
        AppMsg::DeleteTaskConfirmed(task_id) => app.delete_task(task_id, ctx),
        AppMsg::OpenTaskDisposition => app.open_task_disposition_dialog(ctx),
        AppMsg::SetTaskState { task_id, state } => app.set_task_state(task_id, state, ctx),
        AppMsg::CloseDialog => app.close_dialog(ctx),
    })
    .run()?;
    Ok(())
}

struct App {
    root: DialogLayer<Flex<AppMsg>, AppDialog>,
    context: AppContext,
    task_operation_tx: mpsc::Sender<TaskOperationResult>,
    task_operation_rx: mpsc::Receiver<TaskOperationResult>,
    pending_task_operations: usize,
}

#[derive(Debug)]
enum TaskOperationResult {
    Created {
        task_id: String,
        error: Option<String>,
    },
    Deleted {
        task: Box<Task>,
        error: Option<String>,
    },
    StateSaved {
        task_id: String,
        error: Option<String>,
    },
}

impl App {
    fn new(
        snapshot: WorkspaceSnapshot,
        pool: AnyPool,
        dialect: SqlDialect,
        runtime: Handle,
    ) -> Self {
        let store = Rc::new(RefCell::new(Store::new(
            AppState::from_snapshot(snapshot),
            reduce_app_state as fn(&mut AppState, AppEvent) -> tuicore::DispatchOutcome,
        )));
        let context = AppContext {
            store,
            pool,
            dialect,
            runtime,
        };
        let tabs = Tabs::new(vec![
            Tab::new("Tasks", TaskWorkspace::new(context.clone()))
                .hotkey(keys::APP_TASKS_TAB.hotkey()),
            Tab::text("Calendar", "Time-aware planning comes later.")
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
                    StatusBarMenuItem::WeatherForecast,
                ])
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

        let dialog = management_dialog(context.clone(), ManagementDialogKind::People);
        let (task_operation_tx, task_operation_rx) = mpsc::channel();

        Self {
            root: DialogLayer::new(root, dialog)
                .active(false)
                .layer_percent(80)
                .layer_cross_percent(80)
                .backdrop(DialogBackdrop::dim().amount(0.5)),
            context,
            task_operation_tx,
            task_operation_rx,
            pending_task_operations: 0,
        }
    }

    fn open_management_dialog(&mut self, kind: ManagementDialogKind, ctx: &mut EventCtx<AppMsg>) {
        self.root
            .replace_layer(management_dialog(self.context.clone(), kind), ctx);
        self.root.set_layer_percent(80);
        self.root.set_layer_cross_percent(80);
        self.root.set_active_immediate_with_context(true, ctx);
    }

    fn open_create_task_dialog(&mut self, ctx: &mut EventCtx<AppMsg>) {
        self.root.replace_layer(create_task_dialog_host(), ctx);
        self.root.set_layer_percent(60);
        self.root.set_layer_cross_percent(50);
        self.root.set_fit_content(true);
        self.root.set_active_with_context(true, ctx);
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
        let pool = self.context.pool.clone();
        let dialect = self.context.dialect;
        let tx = self.task_operation_tx.clone();
        self.pending_task_operations += 1;
        self.context.runtime.spawn(async move {
            let task_id = task.id.clone();
            let error = storage::create_task(pool, dialect, task)
                .await
                .err()
                .map(|error| error.to_string());
            let _ = tx.send(TaskOperationResult::Created { task_id, error });
        });
        self.close_dialog(ctx);
    }

    fn open_delete_task_dialog(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let Some(task) = self.selected_task() else {
            return;
        };
        self.root.replace_layer(delete_task_dialog(&task), ctx);
        self.root.set_fit_content(true);
        self.root.set_active_with_context(true, ctx);
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
        let pool = self.context.pool.clone();
        let dialect = self.context.dialect;
        let tx = self.task_operation_tx.clone();
        self.pending_task_operations += 1;
        self.context.runtime.spawn(async move {
            let error = storage::delete_task(pool, dialect, task_id.clone())
                .await
                .err()
                .map(|error| error.to_string());
            let _ = tx.send(TaskOperationResult::Deleted {
                task: Box::new(task),
                error,
            });
        });
        self.close_dialog(ctx);
    }

    fn open_task_disposition_dialog(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let Some(task) = self.selected_task() else {
            return;
        };
        self.root.replace_layer(task_disposition_dialog(&task), ctx);
        self.root.set_fit_content(true);
        self.root.set_active_with_context(true, ctx);
    }

    fn set_task_state(&mut self, task_id: String, state: TaskState, ctx: &mut EventCtx<AppMsg>) {
        let outcome = self
            .context
            .store
            .borrow_mut()
            .dispatch(AppEvent::PatchTask {
                task_id: task_id.clone(),
                patch: TaskPatch::State(state),
            });
        if outcome.changed {
            let pool = self.context.pool.clone();
            let dialect = self.context.dialect;
            let tx = self.task_operation_tx.clone();
            self.pending_task_operations += 1;
            self.context.runtime.spawn(async move {
                let error =
                    storage::save_patch(pool, dialect, task_id.clone(), TaskPatch::State(state))
                        .await
                        .err()
                        .map(|error| error.to_string());
                let _ = tx.send(TaskOperationResult::StateSaved { task_id, error });
            });
        }
        self.close_dialog(ctx);
    }

    fn selected_task(&self) -> Option<Task> {
        let store = self.context.store.borrow();
        let state = store.state();
        state
            .selected_task_id
            .as_deref()
            .and_then(|id| state.tasks.iter().find(|task| task.id == id))
            .cloned()
    }

    fn close_dialog(&mut self, ctx: &mut EventCtx<AppMsg>) {
        self.root.set_active_with_context(false, ctx);
    }

    fn poll_task_operation_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.task_operation_rx.try_recv() {
            self.pending_task_operations = self.pending_task_operations.saturating_sub(1);
            let outcome = match result {
                TaskOperationResult::Created { task_id, error } => self
                    .context
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::SaveCompleted {
                        target: SaveTarget::task(task_id, TaskField::Title),
                        error: error.map(|error| format!("Create failed: {error}")),
                    }),
                TaskOperationResult::Deleted { task, error } => match error {
                    Some(error) => {
                        let task_id = task.id.clone();
                        let mut store = self.context.store.borrow_mut();
                        let outcome = store.dispatch(AppEvent::TaskCreated(*task));
                        store.dispatch(AppEvent::SaveCompleted {
                            target: SaveTarget::task(task_id, TaskField::Title),
                            error: Some(format!("Delete failed: {error}")),
                        });
                        outcome
                    }
                    None => tuicore::DispatchOutcome::unchanged(),
                },
                TaskOperationResult::StateSaved { task_id, error } => self
                    .context
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::SaveCompleted {
                        target: SaveTarget::task(task_id, TaskField::State),
                        error: error.map(|error| format!("State update failed: {error}")),
                    }),
            };
            changed |= outcome.changed;
        }
        changed
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
        if self.poll_task_operation_results() {
            result = result.merge(TickResult::CHANGED);
        }
        if self.pending_task_operations > 0 {
            result = result.merge(TickResult::scheduled_after(Duration::from_millis(50)));
        }
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
type VisibleTaskSelection = Rc<RefCell<Option<String>>>;
type PatchSink = Rc<RefCell<Vec<TaskPatch>>>;
type PersonPatchSink = Rc<RefCell<Vec<PersonPatch>>>;
type ProjectPatchSink = Rc<RefCell<Vec<ProjectPatch>>>;
type AppStore =
    Rc<RefCell<Store<AppState, AppEvent, fn(&mut AppState, AppEvent) -> tuicore::DispatchOutcome>>>;

#[derive(Clone)]
struct AppContext {
    store: AppStore,
    pool: AnyPool,
    dialect: SqlDialect,
    runtime: Handle,
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
        Self::Todo,
        Self::Snoozed,
        Self::InProgress,
        Self::Archived,
        Self::All,
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
            Self::All => true,
        }
    }
}

const TASK_VIEW_MENU_TRIGGER: &str = "trigger";
const TASK_VIEW_MENU_PANEL: &str = "menu";

struct TaskViewMenu {
    trigger: Button<AppMsg>,
    menu: Menu<TaskView>,
    pending_view: TaskViewChange,
}

impl TaskViewMenu {
    fn new(pending_view: TaskViewChange, selected: TaskView) -> Self {
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
        }
    }

    fn sync_activated(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let Some(view) = self.menu.take_activated().into_iter().last() else {
            return;
        };
        self.trigger.set_label(view.menu_label());
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

#[derive(Debug)]
struct SaveResult {
    task_id: String,
    field: TaskField,
    seq: u64,
    error: Option<String>,
}

#[derive(Clone)]
struct SaveStatusLine {
    value: Rc<RefCell<(String, bool)>>,
}

impl SaveStatusLine {
    fn new(error: Option<&str>) -> Self {
        let line = Self {
            value: Rc::new(RefCell::new((String::new(), false))),
        };
        line.set_error(error);
        line
    }

    fn set_error(&self, error: Option<&str>) {
        *self.value.borrow_mut() = match error {
            Some(error) => (error.to_string(), true),
            None => (String::new(), false),
        };
    }
}

impl TuiNode<AppMsg> for SaveStatusLine {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let value = self.value.borrow();
        let height = u16::from(!value.0.is_empty());
        LayoutSizeHint::content(value.0.chars().count() as u16, height).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, _ctx: &mut LayoutCtx) -> LayoutResult {
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, _ctx: &mut RenderCtx<'a>) {
        let value = self.value.borrow();
        let color = if value.1 {
            tuicore::theme().error_fg()
        } else {
            tuicore::theme().muted_fg()
        };
        frame.render_widget(
            ratatui::widgets::Paragraph::new(value.0.as_str()).style(Style::default().fg(color)),
            area,
        );
    }
}

struct TaskWorkspace {
    context: AppContext,
    layout: TaskWorkspaceLayout,
    task_view: TaskView,
    pending_task_view: TaskViewChange,
    visible_task_ids: Vec<String>,
    visible_selection: VisibleTaskSelection,
    table_focused: bool,
    observed_version: u64,
    save_tx: mpsc::Sender<SaveResult>,
    save_rx: mpsc::Receiver<SaveResult>,
    next_save_seq: u64,
    latest_save_seq: HashMap<(String, TaskField), u64>,
    pending_saves: HashMap<(String, TaskField), TaskPatch>,
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
        let visible_selection = Rc::new(RefCell::new(selected_task_id.clone()));
        let toolbar = task_toolbar(Rc::clone(&pending_task_view), task_view);
        let pane = task_split(&context.store, task_view);
        let layout =
            Split::vertical(toolbar, pane).constraints(Constraint::Length(1), Constraint::Min(1));
        let observed_version = context.store.borrow().state().version;
        let (save_tx, save_rx) = mpsc::channel();
        Self {
            context,
            layout,
            task_view,
            pending_task_view,
            visible_task_ids,
            visible_selection,
            table_focused: false,
            observed_version,
            save_tx,
            save_rx,
            next_save_seq: 0,
            latest_save_seq: HashMap::new(),
            pending_saves: HashMap::new(),
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
        if self.observed_version != state.version {
            self.refresh_from_state(&state, false);
        }
    }

    fn refresh_from_state(&mut self, state: &AppState, select_first: bool) {
        let previous_task_id = self.table().highlighted_id();
        let previous_index = previous_task_id.as_ref().and_then(|id| {
            self.visible_task_ids
                .iter()
                .position(|visible_id| visible_id == id)
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
            previous_task_id
                .filter(|id| contains_id(id))
                .or_else(|| {
                    previous_index.and_then(|index| {
                        rows.get(index.min(rows.len().saturating_sub(1)))
                            .map(|task| task.id.clone())
                    })
                })
                .or_else(|| {
                    state
                        .selected_task_id
                        .as_deref()
                        .filter(|id| contains_id(id))
                        .map(str::to_string)
                })
                .or_else(|| rows.first().map(|task| task.id.clone()))
        };
        let selected_task = selected_task_id
            .as_deref()
            .and_then(|id| state.tasks.iter().find(|task| task.id == id));
        let save_error = selected_task
            .and_then(|task| state.task_save_error(&task.id))
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
        if detail_needs_refresh {
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
        self.observed_version = state.version;
    }

    fn sync_task_view_change(&mut self) -> bool {
        let Some(next_view) = self.pending_task_view.borrow_mut().take() else {
            return false;
        };
        if next_view == self.task_view {
            return false;
        }
        self.task_view = next_view;
        let state = self.context.store.borrow().state().clone();
        self.refresh_from_state(&state, true);
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
        let save_error = selected_task.and_then(|task| state.task_save_error(&task.id));
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
        let previous_task_id = self.table().highlighted_id();
        let state = self.context.store.borrow().state().clone();
        self.refresh_from_state(&state, false);
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
        self.spawn_save(task_id, patch);
        true
    }

    fn spawn_save(&mut self, task_id: String, patch: TaskPatch) {
        let field = patch.field();
        let key = (task_id.clone(), field);
        if self.latest_save_seq.contains_key(&key) {
            self.pending_saves.insert(key, patch);
            return;
        }
        self.start_save(task_id, patch);
    }

    fn start_save(&mut self, task_id: String, patch: TaskPatch) {
        let field = patch.field();
        let seq = self.next_save_seq;
        self.next_save_seq += 1;
        self.latest_save_seq.insert((task_id.clone(), field), seq);
        let pool = self.context.pool.clone();
        let dialect = self.context.dialect;
        let tx = self.save_tx.clone();
        self.context.runtime.spawn(async move {
            let error = storage::save_patch(pool, dialect, task_id.clone(), patch)
                .await
                .err()
                .map(|error| error.to_string());
            let _ = tx.send(SaveResult {
                task_id,
                field,
                seq,
                error,
            });
        });
    }

    fn poll_save_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.save_rx.try_recv() {
            let key = (result.task_id.clone(), result.field);
            let latest = self.latest_save_seq.get(&key).copied();
            if latest != Some(result.seq) {
                continue;
            }
            self.latest_save_seq.remove(&key);
            if let Some(patch) = self.pending_saves.remove(&key) {
                self.start_save(result.task_id, patch);
                changed = true;
                continue;
            }
            let outcome = self
                .context
                .store
                .borrow_mut()
                .dispatch(AppEvent::SaveCompleted {
                    target: SaveTarget::task(result.task_id, result.field),
                    error: result.error,
                });
            changed |= outcome.changed;
        }
        changed
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
        let has_visible_task = self.visible_selection.borrow().is_some();
        let message = if self.table_focused
            && has_visible_task
            && app_keymap::matches_any(event, &[keys::TASK_DELETE, keys::TASK_DELETE_ALT])
        {
            Some(AppMsg::OpenDeleteTask)
        } else if self.table_focused && has_visible_task && keys::TASK_DISPOSITION.matches(event) {
            Some(AppMsg::OpenTaskDisposition)
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
        let mut result = self.layout.tick(dt, settings);
        if self.poll_save_results() {
            self.sync_store_version();
            result = result.merge(TickResult::CHANGED);
        }
        if !self.latest_save_seq.is_empty() {
            result = result.merge(TickResult::scheduled_after(Duration::from_millis(50)));
        }
        result
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

type PersonTable = DataView<Person, String>;
type ProjectTable = DataView<Project, String>;
type TagTable = DataView<Tag, String>;
type TagPatchSink = Rc<RefCell<Vec<TagPatch>>>;

#[derive(Debug)]
struct PersonSaveResult {
    person_id: String,
    field: PersonField,
    seq: u64,
    error: Option<String>,
}

#[derive(Debug)]
struct ProjectSaveResult {
    project_id: String,
    field: ProjectField,
    seq: u64,
    error: Option<String>,
}

#[derive(Debug)]
struct TagSaveResult {
    tag_id: String,
    field: TagField,
    seq: u64,
    error: Option<String>,
}

struct PeopleWorkspace {
    context: AppContext,
    split: Split<PersonTable, PersonDetailForm>,
    observed_version: u64,
    save_tx: mpsc::Sender<PersonSaveResult>,
    save_rx: mpsc::Receiver<PersonSaveResult>,
    next_save_seq: u64,
    latest_save_seq: HashMap<(String, PersonField), u64>,
    pending_saves: HashMap<(String, PersonField), PersonPatch>,
}

impl PeopleWorkspace {
    fn new(context: AppContext) -> Self {
        let split = person_split(&context.store);
        let observed_version = context.store.borrow().state().version;
        let (save_tx, save_rx) = mpsc::channel();
        Self {
            context,
            split,
            observed_version,
            save_tx,
            save_rx,
            next_save_seq: 0,
            latest_save_seq: HashMap::new(),
            pending_saves: HashMap::new(),
        }
    }

    fn sync_store_version(&mut self) {
        let store = self.context.store.borrow();
        let state = store.state();
        let version = state.version;
        if self.observed_version != version {
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
                    if matches!(event, DataViewTypedEvent::Activated { .. }) {
                        focus_detail = true;
                    }
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
            let selected_person = state.people.iter().find(|person| person.id == id);
            let save_error = selected_person.and_then(|person| state.person_save_error(&person.id));
            self.split
                .second_mut()
                .set_person(selected_person, save_error, ctx);
        }
        outcome.changed
    }

    fn drain_detail_patches(&mut self) -> bool {
        let patches = self.split.second_mut().take_patches();
        let mut changed = false;
        for (person_id, patch) in patches {
            changed |= self.apply_patch(person_id, patch);
        }
        changed
    }

    fn sync_detail_changes(&mut self) -> bool {
        if !self.drain_detail_patches() {
            return false;
        }
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
        true
    }

    fn apply_patch(&mut self, person_id: String, patch: PersonPatch) -> bool {
        let outcome = self
            .context
            .store
            .borrow_mut()
            .dispatch(AppEvent::PatchPerson {
                person_id: person_id.clone(),
                patch: patch.clone(),
            });
        if !outcome.changed {
            return false;
        }
        self.spawn_save(person_id, patch);
        true
    }

    fn spawn_save(&mut self, person_id: String, patch: PersonPatch) {
        let field = patch.field();
        let key = (person_id.clone(), field);
        if self.latest_save_seq.contains_key(&key) {
            self.pending_saves.insert(key, patch);
            return;
        }
        self.start_save(person_id, patch);
    }

    fn start_save(&mut self, person_id: String, patch: PersonPatch) {
        let field = patch.field();
        let seq = self.next_save_seq;
        self.next_save_seq += 1;
        self.latest_save_seq.insert((person_id.clone(), field), seq);
        let pool = self.context.pool.clone();
        let dialect = self.context.dialect;
        let tx = self.save_tx.clone();
        self.context.runtime.spawn(async move {
            let error = storage::save_person_patch(pool, dialect, person_id.clone(), patch)
                .await
                .err()
                .map(|error| error.to_string());
            let _ = tx.send(PersonSaveResult {
                person_id,
                field,
                seq,
                error,
            });
        });
    }

    fn poll_save_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.save_rx.try_recv() {
            let key = (result.person_id.clone(), result.field);
            let latest = self.latest_save_seq.get(&key).copied();
            if latest != Some(result.seq) {
                continue;
            }
            self.latest_save_seq.remove(&key);
            if let Some(patch) = self.pending_saves.remove(&key) {
                self.start_save(result.person_id, patch);
                changed = true;
                continue;
            }
            let outcome = self
                .context
                .store
                .borrow_mut()
                .dispatch(AppEvent::SaveCompleted {
                    target: SaveTarget::person(result.person_id, result.field),
                    error: result.error,
                });
            changed |= outcome.changed;
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
        let mut result = self.split.tick(dt, settings);
        if self.poll_save_results() {
            self.sync_store_version();
            result = result.merge(TickResult::CHANGED);
        }
        if !self.latest_save_seq.is_empty() {
            result = result.merge(TickResult::scheduled_after(Duration::from_millis(50)));
        }
        result
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

struct ProjectsWorkspace {
    context: AppContext,
    split: Split<ProjectTable, ProjectDetailForm>,
    observed_version: u64,
    save_tx: mpsc::Sender<ProjectSaveResult>,
    save_rx: mpsc::Receiver<ProjectSaveResult>,
    next_save_seq: u64,
    latest_save_seq: HashMap<(String, ProjectField), u64>,
    pending_saves: HashMap<(String, ProjectField), ProjectPatch>,
}

impl ProjectsWorkspace {
    fn new(context: AppContext) -> Self {
        let split = project_split(&context.store);
        let observed_version = context.store.borrow().state().version;
        let (save_tx, save_rx) = mpsc::channel();
        Self {
            context,
            split,
            observed_version,
            save_tx,
            save_rx,
            next_save_seq: 0,
            latest_save_seq: HashMap::new(),
            pending_saves: HashMap::new(),
        }
    }

    fn sync_store_version(&mut self) {
        let store = self.context.store.borrow();
        let state = store.state();
        let version = state.version;
        if self.observed_version != version {
            let rows = state.projects.clone();
            let save_error = state
                .selected_project_id
                .as_deref()
                .and_then(|id| state.project_save_error(id))
                .map(str::to_string);
            drop(store);
            self.split.first_mut().set_rows(rows);
            self.split
                .second_mut()
                .set_save_error(save_error.as_deref());
            self.observed_version = version;
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
                    if matches!(event, DataViewTypedEvent::Activated { .. }) {
                        focus_detail = true;
                    }
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
            let selected_project = state.projects.iter().find(|project| project.id == id);
            let save_error =
                selected_project.and_then(|project| state.project_save_error(&project.id));
            self.split
                .second_mut()
                .set_project(selected_project, &state.people, save_error, ctx);
        }
        outcome.changed
    }

    fn drain_detail_patches(&mut self) -> bool {
        let patches = self.split.second_mut().take_patches();
        let mut changed = false;
        for (project_id, patch) in patches {
            changed |= self.apply_patch(project_id, patch);
        }
        changed
    }

    fn sync_detail_changes(&mut self) -> bool {
        if !self.drain_detail_patches() {
            return false;
        }
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
        true
    }

    fn apply_patch(&mut self, project_id: String, patch: ProjectPatch) -> bool {
        let outcome = self
            .context
            .store
            .borrow_mut()
            .dispatch(AppEvent::PatchProject {
                project_id: project_id.clone(),
                patch: patch.clone(),
            });
        if !outcome.changed {
            return false;
        }
        self.spawn_save(project_id, patch);
        true
    }

    fn spawn_save(&mut self, project_id: String, patch: ProjectPatch) {
        let field = patch.field();
        let key = (project_id.clone(), field);
        if self.latest_save_seq.contains_key(&key) {
            self.pending_saves.insert(key, patch);
            return;
        }
        self.start_save(project_id, patch);
    }

    fn start_save(&mut self, project_id: String, patch: ProjectPatch) {
        let field = patch.field();
        let seq = self.next_save_seq;
        self.next_save_seq += 1;
        self.latest_save_seq
            .insert((project_id.clone(), field), seq);
        let pool = self.context.pool.clone();
        let dialect = self.context.dialect;
        let tx = self.save_tx.clone();
        self.context.runtime.spawn(async move {
            let error = storage::save_project_patch(pool, dialect, project_id.clone(), patch)
                .await
                .err()
                .map(|error| error.to_string());
            let _ = tx.send(ProjectSaveResult {
                project_id,
                field,
                seq,
                error,
            });
        });
    }

    fn poll_save_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.save_rx.try_recv() {
            let key = (result.project_id.clone(), result.field);
            let latest = self.latest_save_seq.get(&key).copied();
            if latest != Some(result.seq) {
                continue;
            }
            self.latest_save_seq.remove(&key);
            if let Some(patch) = self.pending_saves.remove(&key) {
                self.start_save(result.project_id, patch);
                changed = true;
                continue;
            }
            let outcome = self
                .context
                .store
                .borrow_mut()
                .dispatch(AppEvent::SaveCompleted {
                    target: SaveTarget::project(result.project_id, result.field),
                    error: result.error,
                });
            changed |= outcome.changed;
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
        let mut result = self.split.tick(dt, settings);
        if self.poll_save_results() {
            self.sync_store_version();
            result = result.merge(TickResult::CHANGED);
        }
        if !self.latest_save_seq.is_empty() {
            result = result.merge(TickResult::scheduled_after(Duration::from_millis(50)));
        }
        result
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

struct TagsWorkspace {
    context: AppContext,
    split: Split<TagTable, TagDetailForm>,
    observed_version: u64,
    save_tx: mpsc::Sender<TagSaveResult>,
    save_rx: mpsc::Receiver<TagSaveResult>,
    next_save_seq: u64,
    latest_save_seq: HashMap<(String, TagField), u64>,
    pending_saves: HashMap<(String, TagField), TagPatch>,
}

impl TagsWorkspace {
    fn new(context: AppContext) -> Self {
        let split = tag_split(&context.store);
        let observed_version = context.store.borrow().state().version;
        let (save_tx, save_rx) = mpsc::channel();
        Self {
            context,
            split,
            observed_version,
            save_tx,
            save_rx,
            next_save_seq: 0,
            latest_save_seq: HashMap::new(),
            pending_saves: HashMap::new(),
        }
    }

    fn sync_store_version(&mut self) {
        let store = self.context.store.borrow();
        let state = store.state();
        let version = state.version;
        if self.observed_version != version {
            let rows = state.tags.clone();
            let save_error = state
                .selected_tag_id
                .as_deref()
                .and_then(|id| state.tag_save_error(id))
                .map(str::to_string);
            drop(store);
            self.split.first_mut().set_rows(rows);
            self.split
                .second_mut()
                .set_save_error(save_error.as_deref());
            self.observed_version = version;
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
                    if matches!(event, DataViewTypedEvent::Activated { .. }) {
                        focus_detail = true;
                    }
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
            let selected_tag = state.tags.iter().find(|tag| tag.id == id);
            let save_error = selected_tag.and_then(|tag| state.tag_save_error(&tag.id));
            self.split
                .second_mut()
                .set_tag(selected_tag, save_error, ctx);
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
                self.spawn_save(tag_id, patch);
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

    fn spawn_save(&mut self, tag_id: String, patch: TagPatch) {
        let field = patch.field();
        let key = (tag_id.clone(), field);
        if self.latest_save_seq.contains_key(&key) {
            self.pending_saves.insert(key, patch);
            return;
        }
        self.start_save(tag_id, patch);
    }

    fn start_save(&mut self, tag_id: String, patch: TagPatch) {
        let field = patch.field();
        let seq = self.next_save_seq;
        self.next_save_seq += 1;
        self.latest_save_seq.insert((tag_id.clone(), field), seq);
        let pool = self.context.pool.clone();
        let dialect = self.context.dialect;
        let tx = self.save_tx.clone();
        self.context.runtime.spawn(async move {
            let error = storage::save_tag_patch(pool, dialect, tag_id.clone(), patch)
                .await
                .err()
                .map(|error| error.to_string());
            let _ = tx.send(TagSaveResult {
                tag_id,
                field,
                seq,
                error,
            });
        });
    }

    fn poll_save_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.save_rx.try_recv() {
            let key = (result.tag_id.clone(), result.field);
            if self.latest_save_seq.get(&key).copied() != Some(result.seq) {
                continue;
            }
            self.latest_save_seq.remove(&key);
            if let Some(patch) = self.pending_saves.remove(&key) {
                self.start_save(result.tag_id, patch);
                changed = true;
                continue;
            }
            let outcome = self
                .context
                .store
                .borrow_mut()
                .dispatch(AppEvent::SaveCompleted {
                    target: SaveTarget::tag(result.tag_id, result.field),
                    error: result.error,
                });
            changed |= outcome.changed;
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
        let mut result = self.split.tick(dt, settings);
        if self.poll_save_results() {
            self.sync_store_version();
            result = result.merge(TickResult::CHANGED);
        }
        if !self.latest_save_seq.is_empty() {
            result = result.merge(TickResult::scheduled_after(Duration::from_millis(50)));
        }
        result
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

enum AppDialog {
    Management(Box<DialogHost<ManagementDialog, AppMsg>>),
    CreateTask(DialogHost<CreateTaskDialog, AppMsg>),
    DeleteTask(ConfirmationDialog<AppMsg>),
    TaskDisposition(Dialog<AppMsg>),
}

impl TuiNode<AppMsg> for AppDialog {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        match self {
            Self::Management(dialog) => dialog.measure(proposal),
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
            Self::TaskDisposition(dialog) => dialog.measure(proposal),
        }
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        match self {
            Self::Management(dialog) => dialog.layout(area, ctx),
            Self::CreateTask(dialog) => dialog.layout(area, ctx),
            Self::DeleteTask(dialog) => dialog.layout(area, ctx),
            Self::TaskDisposition(dialog) => dialog.layout(area, ctx),
        }
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        match self {
            Self::Management(dialog) => dialog.render(frame, area, ctx),
            Self::CreateTask(dialog) => dialog.render(frame, area, ctx),
            Self::DeleteTask(dialog) => dialog.render(frame, area),
            Self::TaskDisposition(dialog) => dialog.render(frame, area),
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        match self {
            Self::Management(dialog) => dialog.event(event, ctx),
            Self::CreateTask(dialog) => dialog.event(event, ctx),
            Self::DeleteTask(dialog) => dialog.event(event, ctx),
            Self::TaskDisposition(dialog) => dialog.event(event, ctx),
        }
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        match self {
            Self::Management(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::CreateTask(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::DeleteTask(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::TaskDisposition(dialog) => dialog.dispatch_event(route, event, ctx),
        }
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        match self {
            Self::Management(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::CreateTask(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::DeleteTask(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::TaskDisposition(dialog) => dialog.dispatch_focus(target, focused, ctx),
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        match self {
            Self::Management(dialog) => dialog.tick(dt, settings),
            Self::CreateTask(dialog) => dialog.tick(dt, settings),
            Self::DeleteTask(dialog) => dialog.tick(dt, settings),
            Self::TaskDisposition(dialog) => dialog.tick(dt, settings),
        }
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::Management(dialog) => dialog.init(ctx),
            Self::CreateTask(dialog) => dialog.init(ctx),
            Self::DeleteTask(dialog) => dialog.init(ctx),
            Self::TaskDisposition(dialog) => dialog.init(ctx),
        }
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::Management(dialog) => dialog.mount(ctx),
            Self::CreateTask(dialog) => dialog.mount(ctx),
            Self::DeleteTask(dialog) => dialog.mount(ctx),
            Self::TaskDisposition(dialog) => dialog.mount(ctx),
        }
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::Management(dialog) => dialog.unmount(ctx),
            Self::CreateTask(dialog) => dialog.unmount(ctx),
            Self::DeleteTask(dialog) => dialog.unmount(ctx),
            Self::TaskDisposition(dialog) => dialog.unmount(ctx),
        }
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::Management(dialog) => dialog.destroy(ctx),
            Self::CreateTask(dialog) => dialog.destroy(ctx),
            Self::DeleteTask(dialog) => dialog.destroy(ctx),
            Self::TaskDisposition(dialog) => dialog.destroy(ctx),
        }
    }
}

fn management_dialog(context: AppContext, kind: ManagementDialogKind) -> AppDialog {
    let mut management = ManagementDialog::new(context);
    management.set_active(kind);
    AppDialog::Management(Box::new(
        Dialog::new()
            .top_left(kind.title())
            .on_close(|_| AppMsg::CloseDialog)
            .host(management),
    ))
}

fn create_task_dialog_host() -> AppDialog {
    let create_task = CreateTaskDialog::new();
    let actions = create_task.actions();
    AppDialog::CreateTask(
        Dialog::new()
            .top_left("Create task")
            .actions(actions)
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

fn task_disposition_dialog(task: &Task) -> AppDialog {
    let done_task_id = task.id.clone();
    let rejected_task_id = task.id.clone();
    AppDialog::TaskDisposition(
        Dialog::new()
            .top_left("Resolve task")
            .content([format!("Mark “{}” as done, or reject it?", task.title)])
            .actions([
                DialogAction::new("Done")
                    .hotkey(KeySpec::plain('d'))
                    .on_trigger(move || AppMsg::SetTaskState {
                        task_id: done_task_id.clone(),
                        state: TaskState::Done,
                    }),
                DialogAction::new("Reject")
                    .hotkey(KeySpec::plain('r'))
                    .on_trigger(move || AppMsg::SetTaskState {
                        task_id: rejected_task_id.clone(),
                        state: TaskState::Rejected,
                    }),
                DialogAction::new("Cancel")
                    .hotkey(KeySpec::plain('c'))
                    .on_trigger(|| AppMsg::CloseDialog),
            ])
            .on_close(|_| AppMsg::CloseDialog),
    )
}

struct ManagementDialog {
    people: PeopleWorkspace,
    projects: ProjectsWorkspace,
    tags: TagsWorkspace,
    active: ManagementDialogKind,
}

impl ManagementDialog {
    fn new(context: AppContext) -> Self {
        Self {
            people: PeopleWorkspace::new(context.clone()),
            projects: ProjectsWorkspace::new(context.clone()),
            tags: TagsWorkspace::new(context),
            active: ManagementDialogKind::People,
        }
    }

    fn set_active(&mut self, active: ManagementDialogKind) {
        self.active = active;
    }
}

impl TuiNode<AppMsg> for ManagementDialog {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        match self.active {
            ManagementDialogKind::People => self.people.layout(area, ctx),
            ManagementDialogKind::Projects => self.projects.layout(area, ctx),
            ManagementDialogKind::Tags => self.tags.layout(area, ctx),
        }
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        match self.active {
            ManagementDialogKind::People => self.people.render(frame, area, ctx),
            ManagementDialogKind::Projects => self.projects.render(frame, area, ctx),
            ManagementDialogKind::Tags => self.tags.render(frame, area, ctx),
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        match self.active {
            ManagementDialogKind::People => self.people.event(event, ctx),
            ManagementDialogKind::Projects => self.projects.event(event, ctx),
            ManagementDialogKind::Tags => self.tags.event(event, ctx),
        }
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        match self.active {
            ManagementDialogKind::People => self.people.dispatch_event(route, event, ctx),
            ManagementDialogKind::Projects => self.projects.dispatch_event(route, event, ctx),
            ManagementDialogKind::Tags => self.tags.dispatch_event(route, event, ctx),
        }
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        match self.active {
            ManagementDialogKind::People => self.people.dispatch_focus(target, focused, ctx),
            ManagementDialogKind::Projects => self.projects.dispatch_focus(target, focused, ctx),
            ManagementDialogKind::Tags => self.tags.dispatch_focus(target, focused, ctx),
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.people
            .tick(dt, settings)
            .merge(self.projects.tick(dt, settings))
            .merge(self.tags.tick(dt, settings))
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.people.init(ctx);
        self.projects.init(ctx);
        self.tags.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.people.mount(ctx);
        self.projects.mount(ctx);
        self.tags.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.people.unmount(ctx);
        self.projects.unmount(ctx);
        self.tags.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.people.destroy(ctx);
        self.projects.destroy(ctx);
        self.tags.destroy(ctx);
    }
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
        let Some(person_id) = self.person_id.clone() else {
            self.patches.borrow_mut().clear();
            return Vec::new();
        };
        self.patches
            .borrow_mut()
            .drain(..)
            .map(|patch| (person_id.clone(), patch))
            .collect()
    }

    fn set_person(
        &mut self,
        person: Option<&Person>,
        save_error: Option<&str>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.patches = Rc::new(RefCell::new(Vec::new()));
        self.person_id = person.map(|person| person.id.clone());
        self.save_status = SaveStatusLine::new(save_error);
        self.root
            .replace(
                "form",
                person_detail_form(person, Rc::clone(&self.patches), self.save_status.clone()),
                FlexItem::content(),
                ctx,
            )
            .expect("person detail form host should contain form child");
    }

    fn set_save_error(&self, save_error: Option<&str>) {
        self.save_status.set_error(save_error);
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

struct ProjectDetailForm {
    root: Flex<AppMsg>,
    project_id: Option<String>,
    patches: ProjectPatchSink,
    save_status: SaveStatusLine,
}

impl ProjectDetailForm {
    fn new(project: Option<&Project>, people: &[Person], save_error: Option<&str>) -> Self {
        let patches = Rc::new(RefCell::new(Vec::new()));
        let save_status = SaveStatusLine::new(save_error);
        Self {
            root: Flex::column().child(
                "form",
                project_detail_form(project, people, Rc::clone(&patches), save_status.clone()),
                FlexItem::content(),
            ),
            project_id: project.map(|project| project.id.clone()),
            patches,
            save_status,
        }
    }

    fn take_patches(&mut self) -> Vec<(String, ProjectPatch)> {
        let Some(project_id) = self.project_id.clone() else {
            self.patches.borrow_mut().clear();
            return Vec::new();
        };
        self.patches
            .borrow_mut()
            .drain(..)
            .map(|patch| (project_id.clone(), patch))
            .collect()
    }

    fn set_project(
        &mut self,
        project: Option<&Project>,
        people: &[Person],
        save_error: Option<&str>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.patches = Rc::new(RefCell::new(Vec::new()));
        self.project_id = project.map(|project| project.id.clone());
        self.save_status = SaveStatusLine::new(save_error);
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

    fn set_save_error(&self, save_error: Option<&str>) {
        self.save_status.set_error(save_error);
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

struct TagDetailForm {
    root: Flex<AppMsg>,
    tag_id: Option<String>,
    patches: TagPatchSink,
    save_status: SaveStatusLine,
}

impl TagDetailForm {
    fn new(tag: Option<&Tag>, save_error: Option<&str>) -> Self {
        let patches = Rc::new(RefCell::new(Vec::new()));
        let save_status = SaveStatusLine::new(save_error);
        Self {
            root: Flex::column().child(
                "form",
                tag_detail_form(tag, Rc::clone(&patches), save_status.clone()),
                FlexItem::content(),
            ),
            tag_id: tag.map(|tag| tag.id.clone()),
            patches,
            save_status,
        }
    }

    fn take_patches(&mut self) -> Vec<(String, TagPatch)> {
        let Some(tag_id) = self.tag_id.clone() else {
            self.patches.borrow_mut().clear();
            return Vec::new();
        };
        self.patches
            .borrow_mut()
            .drain(..)
            .map(|patch| (tag_id.clone(), patch))
            .collect()
    }

    fn set_tag(&mut self, tag: Option<&Tag>, save_error: Option<&str>, ctx: &mut EventCtx<AppMsg>) {
        self.patches = Rc::new(RefCell::new(Vec::new()));
        self.tag_id = tag.map(|tag| tag.id.clone());
        self.save_status = SaveStatusLine::new(save_error);
        self.root
            .replace(
                "form",
                tag_detail_form(tag, Rc::clone(&self.patches), self.save_status.clone()),
                FlexItem::content(),
                ctx,
            )
            .expect("tag detail form host should contain form child");
    }

    fn set_save_error(&self, save_error: Option<&str>) {
        self.save_status.set_error(save_error);
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

fn person_split(store: &AppStore) -> Split<PersonTable, PersonDetailForm> {
    let store_ref = store.borrow();
    let state = store_ref.state();
    let selected = state.selected_person_id.as_deref();
    let table = person_table(state.people.clone(), selected);
    let selected_person =
        selected.and_then(|id| state.people.iter().find(|person| person.id == id));
    let save_error = selected_person.and_then(|person| state.person_save_error(&person.id));
    let detail = PersonDetailForm::new(selected_person, save_error);
    Split::horizontal(table, detail)
        .ratio(65, 35)
        .separator(Separator::new())
}

fn project_split(store: &AppStore) -> Split<ProjectTable, ProjectDetailForm> {
    let store_ref = store.borrow();
    let state = store_ref.state();
    let selected = state.selected_project_id.as_deref();
    let table = project_table(state.projects.clone(), &state.people, selected);
    let selected_project =
        selected.and_then(|id| state.projects.iter().find(|project| project.id == id));
    let save_error = selected_project.and_then(|project| state.project_save_error(&project.id));
    let detail = ProjectDetailForm::new(selected_project, &state.people, save_error);
    Split::horizontal(table, detail)
        .ratio(65, 35)
        .separator(Separator::new())
}

fn tag_split(store: &AppStore) -> Split<TagTable, TagDetailForm> {
    let store_ref = store.borrow();
    let state = store_ref.state();
    let selected = state.selected_tag_id.as_deref();
    let table = tag_table(state.tags.clone(), selected);
    let selected_tag = selected.and_then(|id| state.tags.iter().find(|tag| tag.id == id));
    let save_error = selected_tag.and_then(|tag| state.tag_save_error(&tag.id));
    let detail = TagDetailForm::new(selected_tag, save_error);
    Split::horizontal(table, detail)
        .ratio(65, 35)
        .separator(Separator::new())
}

fn person_table(rows: Vec<Person>, selected_id: Option<&str>) -> DataView<Person, String> {
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

fn project_table(
    rows: Vec<Project>,
    people: &[Person],
    selected_id: Option<&str>,
) -> DataView<Project, String> {
    let person_names: HashMap<String, String> = people
        .iter()
        .map(|person| (person.id.clone(), person.name.clone()))
        .collect();
    let lead_filter_names = person_names.clone();
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
                        .and_then(|id| person_names.get(id))
                        .cloned()
                        .unwrap_or_else(|| "—".to_string())
                },
            )
            .filter_key(move |row| {
                row.lead_person_id
                    .as_ref()
                    .and_then(|id| lead_filter_names.get(id))
                    .cloned()
                    .unwrap_or_else(|| "none".to_string())
            }),
        ]);
    if let Some(id) = selected_id {
        table = table.selected([id.to_string()]);
    }
    table
}

fn tag_table(rows: Vec<Tag>, selected_id: Option<&str>) -> DataView<Tag, String> {
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
    if let Some(id) = selected_id {
        table = table.selected([id.to_string()]);
    }
    table
}

fn person_detail_form(
    person: Option<&Person>,
    patch_sink: PersonPatchSink,
    save_status: SaveStatusLine,
) -> Flex<AppMsg> {
    let Some(person) = person else {
        return Flex::<AppMsg>::column().child(
            "empty",
            Paragraph::new("No person selected."),
            FlexItem::fixed(1),
        );
    };
    Flex::<AppMsg>::column()
        .gap(0)
        .child("save-status", save_status, FlexItem::fixed(1))
        .child(
            "name",
            TextInput::<AppMsg>::new()
                .value(person.name.clone())
                .panel("Name")
                .on_edit_end({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| {
                        patch_sink.borrow_mut().push(PersonPatch::Name(value));
                        AppMsg::Noop
                    }
                }),
            FlexItem::fixed(3),
        )
        .child(
            "email",
            TextInput::<AppMsg>::new()
                .value(person.email.clone())
                .panel("Email")
                .on_edit_end({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| {
                        patch_sink.borrow_mut().push(PersonPatch::Email(value));
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
                {
                    let patch_sink = Rc::clone(&patch_sink);
                    move |id| {
                        patch_sink
                            .borrow_mut()
                            .push(PersonPatch::Active(id == "true"))
                    }
                },
            ),
            FlexItem::fixed(3),
        )
}

fn project_detail_form(
    project: Option<&Project>,
    people: &[Person],
    patch_sink: ProjectPatchSink,
    save_status: SaveStatusLine,
) -> Flex<AppMsg> {
    let Some(project) = project else {
        return Flex::<AppMsg>::column().child(
            "empty",
            Paragraph::new("No project selected."),
            FlexItem::fixed(1),
        );
    };
    Flex::<AppMsg>::column()
        .gap(0)
        .child("save-status", save_status, FlexItem::fixed(1))
        .child(
            "key",
            TextInput::<AppMsg>::new()
                .value(project.key.clone())
                .panel("Key")
                .on_edit_end({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| {
                        patch_sink.borrow_mut().push(ProjectPatch::Key(value));
                        AppMsg::Noop
                    }
                }),
            FlexItem::fixed(3),
        )
        .child(
            "name",
            TextInput::<AppMsg>::new()
                .value(project.name.clone())
                .panel("Name")
                .on_edit_end({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| {
                        patch_sink.borrow_mut().push(ProjectPatch::Name(value));
                        AppMsg::Noop
                    }
                }),
            FlexItem::fixed(3),
        )
        .child(
            "description",
            TextareaInput::<AppMsg>::new()
                .value(project.description.clone())
                .panel("Description")
                .on_edit_end({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| {
                        patch_sink
                            .borrow_mut()
                            .push(ProjectPatch::Description(value));
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
                {
                    let patch_sink = Rc::clone(&patch_sink);
                    move |id| patch_sink.borrow_mut().push(ProjectPatch::LeadPerson(id))
                },
            ),
            FlexItem::fixed(3),
        )
}

fn tag_detail_form(
    tag: Option<&Tag>,
    patch_sink: TagPatchSink,
    save_status: SaveStatusLine,
) -> Flex<AppMsg> {
    let Some(tag) = tag else {
        return Flex::<AppMsg>::column().child(
            "empty",
            Paragraph::new("No tag selected."),
            FlexItem::fixed(1),
        );
    };
    Flex::<AppMsg>::column()
        .gap(0)
        .child("save-status", save_status, FlexItem::fixed(1))
        .child(
            "label",
            TextInput::<AppMsg>::new()
                .value(tag.label.clone())
                .panel("Label")
                .on_edit_end(move |value| {
                    patch_sink.borrow_mut().push(TagPatch::Label(value));
                    AppMsg::Noop
                }),
            FlexItem::fixed(3),
        )
}

fn task_toolbar(pending_view: TaskViewChange, selected_view: TaskView) -> Flex<AppMsg> {
    let view = TaskViewMenu::new(pending_view, selected_view);
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
    let save_error = selected_task.and_then(|task| state.task_save_error(&task.id));
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
        .headers(true)
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

    Flex::<AppMsg>::column()
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

fn detail_escape(event: &TuiEvent) -> bool {
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

fn dropdown_single_optional(
    label: &'static str,
    mut rows: Vec<Choice>,
    selected: Option<&str>,
    on_select: impl Fn(Option<String>) + 'static,
) -> Dropdown<Choice, String> {
    rows.insert(
        0,
        Choice {
            id: "".to_string(),
            label: "None".to_string(),
        },
    );
    Dropdown::single(rows, |row| row.id.clone(), |row| row.label.clone())
        .label(label)
        .selected_one(selected.unwrap_or_default().to_string())
        .search_mode(DropdownSearchMode::Contains)
        .commit_mode(DropdownCommitMode::Explicit)
        .on_select(move |ids| {
            if let Some(id) = ids.into_iter().next() {
                on_select((!id.is_empty()).then_some(id));
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
            id: "snoozed".to_string(),
            label: "Snoozed".to_string(),
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

fn active_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "true".to_string(),
            label: "Active".to_string(),
        },
        Choice {
            id: "false".to_string(),
            label: "Inactive".to_string(),
        },
    ]
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
mod tests {
    use super::*;
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
        let mut input = TaskTagsInput::new(&test_task(), &[api.clone()], Rc::clone(&patches));
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

    fn test_context(
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
        let context = AppContext {
            store: Rc::clone(&store),
            pool,
            dialect: SqlDialect::Sqlite,
            runtime: runtime.handle().clone(),
        };
        (runtime, context, store)
    }

    fn rendered_text(node: &impl TuiNode<AppMsg>, area: Rect) -> String {
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
                TaskView::Todo,
                TaskView::Snoozed,
                TaskView::InProgress,
                TaskView::Archived,
                TaskView::All,
            ]
        );
        assert_eq!(
            TaskView::OPTIONS.map(TaskView::label),
            ["Todo", "Snoozed", "In progress", "Archived", "All"]
        );
        assert_eq!(
            TaskView::OPTIONS.map(TaskView::icon),
            ["", "󰒲", "", "", ""]
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
            assert_eq!(buffer.cell((0, 2)).unwrap().symbol(), "");
            assert_eq!(buffer.cell((1, 2)).unwrap().symbol(), " ");
            assert_eq!(buffer.cell((2, 2)).unwrap().symbol(), "󰇼");
            assert_eq!(buffer.cell((3, 2)).unwrap().symbol(), " ");
            assert_eq!(buffer.cell((4, 2)).unwrap().symbol(), "S");
            assert_eq!(buffer.cell((8, 2)).unwrap().symbol(), "L");
            assert_eq!(buffer.cell((9, 2)).unwrap().symbol(), " ");
            assert_eq!(buffer.cell((10, 2)).unwrap().symbol(), "Z");
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
        for key in [next, KeyEvent::from(Key::Enter)] {
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
        for title in [
            "Active work",
            "Todo work",
            "Completed work",
            "Rejected work",
            "Snoozed work",
        ] {
            assert!(all.contains(title), "missing task in All view: {title}");
        }
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
    fn created_todo_task_does_not_replace_in_progress_selection() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
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
            Some("task-1")
        );
        assert_eq!(
            workspace.table_mut().highlighted_id().as_deref(),
            Some("task-1")
        );
        assert_eq!(
            workspace.table_mut().selected_id().as_deref(),
            Some("task-1")
        );
        assert_eq!(workspace.detail_mut().task_id.as_deref(), Some("task-1"));
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
        assert!(matches!(ctx.messages(), [AppMsg::OpenDeleteTask]));
    }

    #[test]
    fn ctrl_backspace_opens_confirmation_from_focused_task_table() {
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
            code: Key::Backspace,
            modifiers: KeyModifiers::CONTROL,
        };

        let outcome = workspace.event(&TuiEvent::Key(key), &mut ctx);

        assert!(outcome.handled());
        assert!(matches!(ctx.messages(), [AppMsg::OpenDeleteTask]));
    }

    #[test]
    fn backspace_opens_disposition_dialog_from_focused_task_table() {
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        workspace.table_focused = true;
        let mut ctx = EventCtx::default();

        let outcome = workspace.event(&TuiEvent::Key(KeyEvent::from(Key::Backspace)), &mut ctx);

        assert!(outcome.handled());
        assert!(matches!(ctx.messages(), [AppMsg::OpenTaskDisposition]));
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
        let (runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut app = App::new(
            WorkspaceSnapshot {
                tasks: vec![test_task()],
                people: Vec::new(),
                projects: Vec::new(),
                tags: Vec::new(),
            },
            context.pool,
            SqlDialect::Sqlite,
            runtime.handle().clone(),
        );

        app.delete_task("task-1".to_string(), &mut EventCtx::default());

        assert!(app.context.store.borrow().state().tasks.is_empty());
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
    fn task_action_dialogs_fit_their_content() {
        let snapshot = WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        };
        let (runtime, context, _store) = test_context(snapshot.clone());
        let mut app = App::new(
            snapshot,
            context.pool,
            SqlDialect::Sqlite,
            runtime.handle().clone(),
        );
        let area = Rect::new(0, 0, 120, 40);

        app.open_delete_task_dialog(&mut EventCtx::default());
        let mut delete_layout = LayoutCtx::new();
        app.layout(area, &mut delete_layout);
        let delete_area = delete_layout
            .overlays()
            .first()
            .expect("delete dialog should register an overlay")
            .area;

        app.open_task_disposition_dialog(&mut EventCtx::default());
        let mut resolve_layout = LayoutCtx::new();
        app.layout(area, &mut resolve_layout);
        let resolve_area = resolve_layout
            .overlays()
            .first()
            .expect("resolve dialog should register an overlay")
            .area;

        for dialog_area in [delete_area, resolve_area] {
            assert!(dialog_area.width > 20);
            assert!(dialog_area.height >= 3);
            assert!(dialog_area.width < area.width / 2);
            assert!(dialog_area.height < area.height / 4);
        }
    }

    #[test]
    fn create_task_dialog_fits_its_content_height() {
        let snapshot = WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
            tags: Vec::new(),
        };
        let (runtime, context, _store) = test_context(snapshot.clone());
        let mut app = App::new(
            snapshot,
            context.pool,
            SqlDialect::Sqlite,
            runtime.handle().clone(),
        );
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
    fn focused_detail_input_receives_tab_navigation_characters_before_ancestor_tabs() {
        let person = Person {
            id: "person-1".to_string(),
            name: "Ada".to_string(),
            email: "ada@example.com".to_string(),
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

        let patches = patches.borrow();
        let [PersonPatch::Name(value)] = patches.as_slice() else {
            panic!("expected one submitted name patch, got {patches:?}");
        };
        assert_eq!(value, "Ada[]");
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
        let mut workspace = TaskWorkspace::new(AppContext {
            store: Rc::clone(&store),
            pool,
            dialect: SqlDialect::Sqlite,
            runtime: runtime.handle().clone(),
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
    fn people_save_status_reconciliation_keeps_pending_detail_changes() {
        let person = Person {
            id: "person-1".to_string(),
            name: "Ada".to_string(),
            email: "ada@example.com".to_string(),
            active: true,
        };
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: vec![person],
            projects: Vec::new(),
            tags: Vec::new(),
        });
        let mut workspace = PeopleWorkspace::new(context);
        workspace
            .split
            .second_mut()
            .patches
            .borrow_mut()
            .push(PersonPatch::Name("Ada Lovelace".to_string()));

        store.borrow_mut().dispatch(AppEvent::SaveCompleted {
            target: SaveTarget::person("person-1".to_string(), PersonField::Email),
            error: Some("offline".to_string()),
        });
        workspace.layout(Rect::new(0, 0, 100, 30), &mut LayoutCtx::new());

        let patches = workspace.split.second_mut().take_patches();
        let [(person_id, PersonPatch::Name(name))] = patches.as_slice() else {
            panic!("expected pending person name patch, got {patches:?}");
        };
        assert_eq!(person_id, "person-1");
        assert_eq!(name, "Ada Lovelace");
        assert!(rendered_text(&workspace, Rect::new(0, 0, 100, 30)).contains("Save failed"));
    }

    #[test]
    fn tags_management_workspace_has_table_and_editable_detail() {
        let tags = vec![
            Tag {
                id: "tag-api".to_string(),
                label: "api".to_string(),
            },
            Tag {
                id: "tag-backend".to_string(),
                label: "backend".to_string(),
            },
        ];
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            projects: Vec::new(),
            tags,
        });
        let mut workspace = TagsWorkspace::new(context);
        let area = Rect::new(0, 0, 100, 30);

        workspace.layout(area, &mut LayoutCtx::new());
        let text = rendered_text(&workspace, area);
        assert!(text.contains("Tag"));
        assert!(text.contains("api"));
        assert!(text.contains("backend"));
        assert!(text.contains("Label"));

        workspace.select_tag("tag-backend", &mut EventCtx::default());
        workspace
            .split
            .second_mut()
            .patches
            .borrow_mut()
            .push(TagPatch::Label("platform".to_string()));
        assert!(workspace.sync_detail_changes());
        assert_eq!(store.borrow().state().tags[1].label, "platform");
        assert_eq!(ManagementDialogKind::Tags.title(), "Tags");
    }
}
