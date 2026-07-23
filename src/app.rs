use std::{cell::RefCell, collections::HashMap, error::Error, rc::Rc, sync::mpsc, time::Duration};

use crate::app_keymap::{self, keys};
use crate::create_task_dialog::CreateTaskDialog;
use crate::domain::{
    AppEvent, AppState, Person, PersonField, PersonPatch, Project, ProjectField, ProjectPatch,
    SaveTarget, Task, TaskField, TaskPatch, TaskSize, TaskState, TaskSubtype, TaskType,
    WorkspaceSnapshot, reduce_app_state,
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
    ActivationMode, AnimationSettings, Button, CellContext, ChipColorRole, Column, DataView,
    DataViewTypedEvent, DatePickerDropdown, Dialog, DialogBackdrop, DialogHost, DialogLayer,
    Dropdown, DropdownCommitMode, DropdownSearchMode, EventCtx, EventOutcome, EventRoute, Flex,
    FlexItem, FocusCtx, FocusId, FocusRequest, FocusTarget, LayoutCtx, LayoutResult, LifecycleCtx,
    Paragraph, RenderCtx, SelectionMode, SelectionTrigger, Separator, Split, StatusBar,
    StatusBarMenuItem, Store, Tab, Tabs, TabsVariant, TextInput, TextareaInput, TickResult,
    TreeApp, TuiEvent, TuiNode,
};
use uuid::Uuid;

const PEOPLE_MENU_ID: &str = "people";
const PROJECTS_MENU_ID: &str = "projects";

#[derive(Debug)]
pub(crate) enum AppMsg {
    Noop,
    OpenManagementDialog(ManagementDialogKind),
    OpenCreateTask,
    CreateTaskSubmitted(String),
    CloseDialog,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ManagementDialogKind {
    People,
    Projects,
}

impl ManagementDialogKind {
    fn title(self) -> &'static str {
        match self {
            Self::People => "People",
            Self::Projects => "Projects",
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
    .on_message(|app, message, ctx| match message {
        AppMsg::Noop => {}
        AppMsg::OpenManagementDialog(kind) => app.open_management_dialog(kind, ctx),
        AppMsg::OpenCreateTask => app.open_create_task_dialog(ctx),
        AppMsg::CreateTaskSubmitted(title) => app.submit_create_task(title, ctx),
        AppMsg::CloseDialog => app.close_dialog(ctx),
    })
    .run()?;
    Ok(())
}

struct App {
    root: DialogLayer<Flex<AppMsg>, DialogHost<AppDialog, AppMsg>>,
    context: AppContext,
    create_task_tx: mpsc::Sender<CreateTaskResult>,
    create_task_rx: mpsc::Receiver<CreateTaskResult>,
    pending_create_tasks: usize,
}

#[derive(Debug)]
struct CreateTaskResult {
    task_id: String,
    error: Option<String>,
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
            Tab::new("Inbox", InboxWorkspace::new()).hotkey(keys::APP_INBOX_TAB.hotkey()),
            Tab::new("Tasks", TaskWorkspace::new(context.clone()))
                .hotkey(keys::APP_TASKS_TAB.hotkey()),
            Tab::text("Notes", "Clarified reference notes live outside the board.")
                .hotkey(keys::APP_NOTES_TAB.hotkey()),
            Tab::text("Calendar", "Time-aware planning comes later.")
                .hotkey(keys::APP_CALENDAR_TAB.hotkey()),
        ])
        .selected(1)
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
                    StatusBarMenuItem::Theme,
                    StatusBarMenuItem::WeatherForecast,
                ])
                .on_custom_menu_item(|id| match id {
                    PEOPLE_MENU_ID => AppMsg::OpenManagementDialog(ManagementDialogKind::People),
                    PROJECTS_MENU_ID => {
                        AppMsg::OpenManagementDialog(ManagementDialogKind::Projects)
                    }
                    _ => AppMsg::OpenManagementDialog(ManagementDialogKind::People),
                }),
            FlexItem::fixed(1),
        );

        let dialog = management_dialog(context.clone(), ManagementDialogKind::People);
        let (create_task_tx, create_task_rx) = mpsc::channel();

        Self {
            root: DialogLayer::new(root, dialog)
                .active(false)
                .layer_percent(80)
                .layer_cross_percent(80)
                .backdrop(DialogBackdrop::dim().amount(0.5)),
            context,
            create_task_tx,
            create_task_rx,
            pending_create_tasks: 0,
        }
    }

    fn open_management_dialog(&mut self, kind: ManagementDialogKind, ctx: &mut EventCtx<AppMsg>) {
        self.root
            .replace_layer(management_dialog(self.context.clone(), kind), ctx);
        self.root.set_layer_percent(80);
        self.root.set_layer_cross_percent(80);
        self.root.set_active_with_context(true, ctx);
    }

    fn open_create_task_dialog(&mut self, ctx: &mut EventCtx<AppMsg>) {
        self.root.replace_layer(create_task_dialog_host(), ctx);
        self.root.set_layer_percent(35);
        self.root.set_layer_cross_percent(50);
        self.root.set_active_with_context(true, ctx);
    }

    fn submit_create_task(&mut self, title: String, ctx: &mut EventCtx<AppMsg>) {
        let title = title.trim();
        if title.is_empty() {
            ctx.notify(tuicore::Notification::warning(
                "Task title required",
                "Enter a title before creating the task.",
            ));
            return;
        }

        let task = Task::quick_capture(Uuid::new_v4().to_string(), title.to_string());
        self.context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(task.clone()));
        let pool = self.context.pool.clone();
        let dialect = self.context.dialect;
        let tx = self.create_task_tx.clone();
        self.pending_create_tasks += 1;
        self.context.runtime.spawn(async move {
            let task_id = task.id.clone();
            let error = storage::create_task(pool, dialect, task)
                .await
                .err()
                .map(|error| error.to_string());
            let _ = tx.send(CreateTaskResult { task_id, error });
        });
        self.close_dialog(ctx);
    }

    fn close_dialog(&mut self, ctx: &mut EventCtx<AppMsg>) {
        self.root.set_active_with_context(false, ctx);
    }

    fn poll_create_task_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.create_task_rx.try_recv() {
            self.pending_create_tasks = self.pending_create_tasks.saturating_sub(1);
            if let Some(error) = result.error {
                let outcome = self
                    .context
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::SaveCompleted {
                        target: SaveTarget::task(result.task_id, TaskField::Title),
                        error: Some(format!("Create failed: {error}")),
                    });
                changed |= outcome.changed;
            }
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
        if self.poll_create_task_results() {
            result = result.merge(TickResult::CHANGED);
        }
        if self.pending_create_tasks > 0 {
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

struct InboxWorkspace {
    root: Flex<AppMsg>,
}

impl InboxWorkspace {
    fn new() -> Self {
        let root = Flex::column()
            .gap(1)
            .child("actions", Button::new("Process"), FlexItem::fixed(1))
            .child(
                "capture",
                TextareaInput::<AppMsg>::new()
                    .placeholder(
                        "Paste messy email, Slack note, voicemail transcript, or raw thought...",
                    )
                    .hotkey(keys::INBOX_CAPTURE.hotkey())
                    .min_rows(8),
                FlexItem::fill(1),
            );

        Self { root }
    }
}

impl TuiNode<AppMsg> for InboxWorkspace {
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

type TaskRow = Task;
type TaskTable = DataView<TaskRow, String>;
type TaskDetail = TaskDetailForm;
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
    split: Split<TaskTable, TaskDetail>,
    observed_version: u64,
    save_tx: mpsc::Sender<SaveResult>,
    save_rx: mpsc::Receiver<SaveResult>,
    next_save_seq: u64,
    latest_save_seq: HashMap<(String, TaskField), u64>,
    pending_saves: HashMap<(String, TaskField), TaskPatch>,
}

impl TaskWorkspace {
    fn new(context: AppContext) -> Self {
        let split = task_split(&context.store);
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
            let rows = state.tasks.clone();
            let selected_task_id = state.selected_task_id.clone();
            let selected_task = selected_task_id
                .as_deref()
                .and_then(|id| state.tasks.iter().find(|task| task.id == id))
                .cloned();
            let save_error = state
                .selected_task_id
                .as_deref()
                .and_then(|id| state.task_save_error(id))
                .map(str::to_string);
            let people = state.people.clone();
            let projects = state.projects.clone();
            drop(store);
            self.split.first_mut().set_rows(rows);
            if let Some(id) = selected_task_id.as_ref() {
                self.split.first_mut().highlight_id(id);
                self.split.first_mut().select_id(id.clone());
            }
            self.split.first_mut().take_events();
            if self.split.second_mut().task_id.as_deref() != selected_task_id.as_deref() {
                self.split.second_mut().set_task(
                    selected_task.as_ref(),
                    &people,
                    &projects,
                    save_error.as_deref(),
                    &mut EventCtx::default(),
                );
            }
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
                    selected_changed |= self.select_task(id, ctx);
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

    fn select_task(&mut self, id: &str, ctx: &mut EventCtx<AppMsg>) -> bool {
        let outcome = self
            .context
            .store
            .borrow_mut()
            .dispatch(AppEvent::SelectTask(id.to_string()));
        let store = self.context.store.borrow();
        let state = store.state();
        let selected_task = state.tasks.iter().find(|task| task.id == id);
        let save_error = selected_task.and_then(|task| state.task_save_error(&task.id));
        self.split.second_mut().set_task(
            selected_task,
            &state.people,
            &state.projects,
            save_error,
            ctx,
        );
        outcome.changed
    }

    fn drain_detail_patches(&mut self) -> bool {
        let patches = self.split.second_mut().take_patches();
        let mut changed = false;
        for (task_id, patch) in patches {
            changed |= self.apply_patch(task_id, patch);
        }
        changed
    }

    fn sync_detail_changes(&mut self) -> bool {
        if !self.drain_detail_patches() {
            return false;
        }
        let store = self.context.store.borrow();
        let state = store.state();
        self.split.first_mut().set_rows(state.tasks.clone());
        self.split.second_mut().set_save_error(
            state
                .selected_task_id
                .as_deref()
                .and_then(|id| state.task_save_error(id)),
        );
        self.observed_version = state.version;
        true
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
}

impl TuiNode<AppMsg> for TaskWorkspace {
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
        if !outcome.handled() && keys::TASK_QUICK_CREATE.matches(event) {
            ctx.emit(AppMsg::OpenCreateTask);
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
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
        if !outcome.handled() && keys::TASK_QUICK_CREATE.matches(event) {
            ctx.emit(AppMsg::OpenCreateTask);
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
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

type PersonTable = DataView<Person, String>;
type ProjectTable = DataView<Project, String>;

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

enum AppDialog {
    Management(Box<ManagementDialog>),
    CreateTask(CreateTaskDialog),
}

impl TuiNode<AppMsg> for AppDialog {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        match self {
            Self::Management(dialog) => dialog.layout(area, ctx),
            Self::CreateTask(dialog) => dialog.layout(area, ctx),
        }
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        match self {
            Self::Management(dialog) => dialog.render(frame, area, ctx),
            Self::CreateTask(dialog) => dialog.render(frame, area, ctx),
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        match self {
            Self::Management(dialog) => dialog.event(event, ctx),
            Self::CreateTask(dialog) => dialog.event(event, ctx),
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
        }
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        match self {
            Self::Management(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::CreateTask(dialog) => dialog.dispatch_focus(target, focused, ctx),
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        match self {
            Self::Management(dialog) => dialog.tick(dt, settings),
            Self::CreateTask(dialog) => dialog.tick(dt, settings),
        }
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::Management(dialog) => dialog.init(ctx),
            Self::CreateTask(dialog) => dialog.init(ctx),
        }
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::Management(dialog) => dialog.mount(ctx),
            Self::CreateTask(dialog) => dialog.mount(ctx),
        }
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::Management(dialog) => dialog.unmount(ctx),
            Self::CreateTask(dialog) => dialog.unmount(ctx),
        }
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::Management(dialog) => dialog.destroy(ctx),
            Self::CreateTask(dialog) => dialog.destroy(ctx),
        }
    }
}

fn management_dialog(
    context: AppContext,
    kind: ManagementDialogKind,
) -> DialogHost<AppDialog, AppMsg> {
    let mut management = ManagementDialog::new(context);
    management.set_active(kind);
    Dialog::new()
        .top_left(kind.title())
        .on_close(|_| AppMsg::CloseDialog)
        .host(AppDialog::Management(Box::new(management)))
}

fn create_task_dialog_host() -> DialogHost<AppDialog, AppMsg> {
    let create_task = CreateTaskDialog::new();
    let actions = create_task.actions();
    Dialog::new()
        .top_left("Create task")
        .actions(actions)
        .on_close(|_| AppMsg::CloseDialog)
        .host(AppDialog::CreateTask(create_task))
}

struct ManagementDialog {
    people: PeopleWorkspace,
    projects: ProjectsWorkspace,
    active: ManagementDialogKind,
}

impl ManagementDialog {
    fn new(context: AppContext) -> Self {
        Self {
            people: PeopleWorkspace::new(context.clone()),
            projects: ProjectsWorkspace::new(context),
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
        }
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        match self.active {
            ManagementDialogKind::People => self.people.render(frame, area, ctx),
            ManagementDialogKind::Projects => self.projects.render(frame, area, ctx),
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        match self.active {
            ManagementDialogKind::People => self.people.event(event, ctx),
            ManagementDialogKind::Projects => self.projects.event(event, ctx),
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
        }
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        match self.active {
            ManagementDialogKind::People => self.people.dispatch_focus(target, focused, ctx),
            ManagementDialogKind::Projects => self.projects.dispatch_focus(target, focused, ctx),
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.people
            .tick(dt, settings)
            .merge(self.projects.tick(dt, settings))
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.people.init(ctx);
        self.projects.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.people.mount(ctx);
        self.projects.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.people.unmount(ctx);
        self.projects.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.people.destroy(ctx);
        self.projects.destroy(ctx);
    }
}

struct TaskDetailForm {
    root: Flex<AppMsg>,
    task_id: Option<String>,
    patches: PatchSink,
    save_status: SaveStatusLine,
}

impl TaskDetailForm {
    fn new(
        task: Option<&TaskRow>,
        people: &[Person],
        projects: &[Project],
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
                    Rc::clone(&patches),
                    save_status.clone(),
                ),
                FlexItem::content(),
            ),
            task_id: task.map(|task| task.id.clone()),
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
        save_error: Option<&str>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.patches = Rc::new(RefCell::new(Vec::new()));
        self.task_id = task.map(|task| task.id.clone());
        self.save_status = SaveStatusLine::new(save_error);
        self.root
            .replace(
                "form",
                detail_form(
                    task,
                    people,
                    projects,
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

fn task_split(store: &AppStore) -> Split<TaskTable, TaskDetail> {
    let store_ref = store.borrow();
    let state = store_ref.state();
    let selected = state.selected_task_id.as_deref();
    let table = task_table(
        state.tasks.clone(),
        &state.people,
        &state.projects,
        selected,
    );
    let selected_task = selected.and_then(|id| state.tasks.iter().find(|task| task.id == id));
    let save_error = selected_task.and_then(|task| state.task_save_error(&task.id));
    let detail = TaskDetailForm::new(selected_task, &state.people, &state.projects, save_error);
    Split::horizontal(table, detail)
        .ratio(65, 35)
        .separator(Separator::new())
}

fn task_table(
    rows: Vec<TaskRow>,
    people: &[Person],
    projects: &[Project],
    selected_id: Option<&str>,
) -> DataView<TaskRow, String> {
    let people_names: HashMap<String, String> = people
        .iter()
        .map(|person| (person.id.clone(), person.name.clone()))
        .collect();
    let people_filter_names = people_names.clone();
    let project_names: HashMap<String, String> = projects
        .iter()
        .map(|project| (project.id.clone(), project.name.clone()))
        .collect();
    let project_filter_names = project_names.clone();
    let mut table = DataView::new(rows, |row: &TaskRow| row.id.clone())
        .headers(true)
        .action_bar(true)
        .activation_mode(ActivationMode::OnActivateKey)
        .selection_mode(SelectionMode::Single)
        .selection_trigger(SelectionTrigger::OnNavigate)
        .columns(vec![
            Column::rich(
                "state",
                "State",
                Constraint::Percentage(16),
                |row: &TaskRow, _: &CellContext<String>| {
                    chip_line(row.state.label(), row.state.role())
                },
            )
            .filter_key(|row| row.state.label().to_string()),
            Column::text(
                "title",
                "Task",
                Constraint::Percentage(34),
                |row: &TaskRow| row.title.clone(),
            )
            .sortable(|row| row.title.clone())
            .filter_key(|row| row.title.clone()),
            Column::rich(
                "size",
                "Size",
                Constraint::Percentage(10),
                |row: &TaskRow, _: &CellContext<String>| {
                    chip_line(row.size.label(), row.size.role())
                },
            )
            .filter_key(|row| row.size.label().to_string()),
            Column::text("due", "Due", Constraint::Percentage(12), |row: &TaskRow| {
                row.due_date.as_deref().unwrap_or("—").to_string()
            })
            .filter_key(|row| row.due_date.clone().unwrap_or_else(|| "none".to_string())),
            Column::text(
                "people",
                "People",
                Constraint::Percentage(14),
                move |row: &TaskRow| task_people_summary(row, &people_names),
            )
            .filter_key(move |row| task_people_summary(row, &people_filter_names)),
            Column::text(
                "projects",
                "Projects",
                Constraint::Percentage(14),
                move |row: &TaskRow| task_projects_summary(row, &project_names),
            )
            .filter_key(move |row| task_projects_summary(row, &project_filter_names)),
        ]);
    if let Some(id) = selected_id {
        table = table.selected([id.to_string()]);
    }
    table
}

fn detail_form(
    task: Option<&TaskRow>,
    people: &[Person],
    projects: &[Project],
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

    let ai_rationale = format!("AI rationale: {}", task.ai_rationale);
    let swap_note = format!("Swap note: {}", task.swap_note);

    Flex::<AppMsg>::column()
        .gap(0)
        .child("save-status", save_status, FlexItem::fixed(1))
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
                .max_rows(8),
            FlexItem::fixed(6),
        )
        .child(
            "ai-rationale",
            Paragraph::new(ai_rationale),
            FlexItem::fixed(1),
        )
        .child("swap-note", Paragraph::new(swap_note), FlexItem::fixed(1))
        .child(
            "type",
            dropdown_single("Type", type_choices(), task.task_type.id(), {
                let patch_sink = Rc::clone(&patch_sink);
                move |id| {
                    if let Some(value) = TaskType::parse(&id) {
                        patch_sink.borrow_mut().push(TaskPatch::Type(value));
                    }
                }
            })
            .hotkey(keys::TASK_TYPE_FIELD.hotkey()),
            FlexItem::fixed(3),
        )
        .child(
            "subtype",
            dropdown_single("Subtype", subtype_choices(), task.subtype.id(), {
                let patch_sink = Rc::clone(&patch_sink);
                move |id| {
                    if let Some(value) = TaskSubtype::parse(&id) {
                        patch_sink.borrow_mut().push(TaskPatch::Subtype(value));
                    }
                }
            }),
            FlexItem::fixed(3),
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

fn task_people_summary(task: &TaskRow, people_names: &HashMap<String, String>) -> String {
    if task.people_ids.is_empty() {
        return "—".to_string();
    }
    task.people_ids
        .iter()
        .map(|id| people_names.get(id).unwrap_or(id).clone())
        .collect::<Vec<_>>()
        .join(", ")
}

fn task_projects_summary(task: &TaskRow, project_names: &HashMap<String, String>) -> String {
    if task.project_ids.is_empty() {
        return "—".to_string();
    }
    task.project_ids
        .iter()
        .map(|id| project_names.get(id).unwrap_or(id).clone())
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug, Clone)]
struct Choice {
    id: String,
    label: String,
}

fn type_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "action".to_string(),
            label: "Action".to_string(),
        },
        Choice {
            id: "note".to_string(),
            label: "Note".to_string(),
        },
    ]
}

fn subtype_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "task".to_string(),
            label: "Task".to_string(),
        },
        Choice {
            id: "waiting".to_string(),
            label: "Waiting".to_string(),
        },
        Choice {
            id: "follow_up".to_string(),
            label: "Follow-up".to_string(),
        },
        Choice {
            id: "artifact_update".to_string(),
            label: "Artifact update".to_string(),
        },
    ]
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
    use tuicore::{FocusManager, HotkeyEvent, Key, TreeDispatcher};

    fn test_task() -> Task {
        Task {
            id: "task-1".to_string(),
            title: "Original".to_string(),
            task_type: TaskType::Action,
            subtype: TaskSubtype::Task,
            state: TaskState::Todo,
            size: TaskSize::Small,
            start_date: None,
            due_date: None,
            people_ids: Vec::new(),
            project_ids: Vec::new(),
            entity_labels: Vec::new(),
            focus_today: false,
            frog_candidate: false,
            detail: "Existing detail".to_string(),
            ai_rationale: String::new(),
            swap_note: String::new(),
        }
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
    fn created_task_becomes_selected_and_highlighted_in_task_view() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let created = Task::quick_capture("task-2".to_string(), "Captured".to_string());

        store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(created.clone()));
        workspace.layout(Rect::new(0, 0, 120, 40), &mut LayoutCtx::new());

        assert_eq!(
            store.borrow().state().selected_task_id.as_deref(),
            Some("task-2")
        );
        assert_eq!(
            workspace.split.first_mut().highlighted_id().as_deref(),
            Some("task-2")
        );
        assert_eq!(
            workspace.split.first_mut().selected_id().as_deref(),
            Some("task-2")
        );
        assert_eq!(
            workspace.split.second_mut().task_id.as_deref(),
            Some("task-2")
        );
    }

    #[test]
    fn created_task_type_hotkey_focuses_open_dropdown() {
        let (_runtime, context, store) = test_context(WorkspaceSnapshot {
            tasks: vec![test_task()],
            people: Vec::new(),
            projects: Vec::new(),
        });
        let mut workspace = TaskWorkspace::new(context);
        let area = Rect::new(0, 0, 120, 40);
        workspace.layout(area, &mut LayoutCtx::new());
        store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(Task::quick_capture(
                "task-2".to_string(),
                "Captured".to_string(),
            )));
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let task_type = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target.id.as_str() == "field"
                    && target.path.keys().iter().any(|key| key.as_str() == "type")
            })
            .expect("task type should be focusable")
            .clone();
        let mut dispatcher = TreeDispatcher::new();

        let effects = dispatcher.dispatch_event(
            &mut workspace,
            &EventRoute::new(task_type.path),
            &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::TASK_TYPE_FIELD.hotkey())),
            AnimationSettings::default(),
        );

        assert!(effects.layout);
        let focus_request = effects
            .focus_request
            .as_ref()
            .expect("type hotkey should request dropdown search focus");
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
                    task_type: TaskType::Action,
                    subtype: TaskSubtype::Task,
                    state: TaskState::Todo,
                    size: TaskSize::Small,
                    start_date: None,
                    due_date: None,
                    people_ids: Vec::new(),
                    project_ids: Vec::new(),
                    entity_labels: Vec::new(),
                    focus_today: false,
                    frog_candidate: false,
                    detail: "Existing detail".to_string(),
                    ai_rationale: String::new(),
                    swap_note: String::new(),
                }],
                people: Vec::new(),
                projects: Vec::new(),
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
        });
        let mut workspace = TaskWorkspace::new(context);
        let area = Rect::new(0, 0, 120, 40);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let task_type = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target.id.as_str() == "field"
                    && target.path.keys().iter().any(|key| key.as_str() == "type")
            })
            .expect("task type should be focusable")
            .clone();
        let mut focus = FocusManager::new();
        let mut dispatcher = TreeDispatcher::new();
        let transition = focus
            .apply_request(
                &FocusRequest::TargetAt {
                    path: task_type.path.clone(),
                    id: task_type.id.clone(),
                },
                layout.focus_targets(),
            )
            .expect("type focus should change");
        dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
        workspace
            .split
            .second_mut()
            .patches
            .borrow_mut()
            .push(TaskPatch::Type(TaskType::Note));
        assert!(workspace.sync_detail_changes());
        assert_eq!(store.borrow().state().tasks[0].task_type, TaskType::Note);

        store.borrow_mut().dispatch(AppEvent::SaveCompleted {
            target: SaveTarget::task("task-1".to_string(), TaskField::Type),
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
            .expect("tab should move to subtype");
        dispatcher.dispatch_focus(&mut workspace, transition, AnimationSettings::default());
        assert!(
            focus
                .current()
                .expect("next control should be focused")
                .path
                .keys()
                .iter()
                .any(|key| key.as_str() == "subtype")
        );
        assert_eq!(store.borrow().state().tasks[0].task_type, TaskType::Note);
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
}
