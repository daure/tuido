use std::{cell::RefCell, collections::HashMap, error::Error, rc::Rc, sync::mpsc, time::Duration};

use crate::app_keymap::{self, keys};
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
    DataViewTypedEvent, DatePickerDropdown, Dropdown, DropdownCommitMode, DropdownSearchMode,
    EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusId, FocusRequest,
    FocusTarget, Key, KeyModifiers, LayoutCtx, LayoutResult, LifecycleCtx, Paragraph, RenderCtx,
    SelectionMode, SelectionTrigger, Separator, Split, StatusBar, Store, Tab, Tabs, TabsVariant,
    TextInput, TextareaInput, TickResult, TuiEvent, TuiNode,
};

pub fn run() -> Result<(), Box<dyn Error>> {
    tuicore::try_init()?;
    app_keymap::try_init()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let storage = runtime.block_on(Storage::connect_from_env())?;
    runtime.block_on(storage.migrate())?;
    let snapshot = runtime.block_on(storage.load_workspace())?;
    tuicore::run(App::new(
        snapshot,
        storage.pool(),
        storage.dialect(),
        runtime.handle().clone(),
    ))?;
    Ok(())
}

struct App {
    root: Flex,
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
            Tab::new("Projects", ProjectsWorkspace::new(context.clone()))
                .hotkey(keys::APP_PROJECTS_TAB.hotkey()),
            Tab::new("People", PeopleWorkspace::new(context)).hotkey(keys::APP_PEOPLE_TAB.hotkey()),
        ])
        .selected(1)
        .variant(TabsVariant::Underline)
        .bordered(true);

        let root = Flex::column().child("tabs", tabs, FlexItem::fill(1)).child(
            "footer",
            StatusBar::new(),
            FlexItem::fixed(1),
        );

        Self { root }
    }
}

impl TuiNode for App {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.root.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.root.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.root.event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.root.dispatch_event(route, event, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.root.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.root.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.destroy(ctx);
    }
}

struct InboxWorkspace {
    root: Flex,
}

impl InboxWorkspace {
    fn new() -> Self {
        let root = Flex::column()
            .gap(1)
            .child("actions", Button::new("Process"), FlexItem::fixed(1))
            .child(
                "capture",
                TextareaInput::new()
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

impl TuiNode for InboxWorkspace {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.root.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.root.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        self.root.event(event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        self.root.dispatch_event(route, event, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.root.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.root.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
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
        let version = self.context.store.borrow().state().version;
        if self.observed_version != version {
            self.split = task_split(&self.context.store);
            self.observed_version = version;
        }
    }

    fn sync_table_events(&mut self, ctx: &mut EventCtx<()>) {
        let events = self.split.first_mut().take_events();
        let mut focus_detail = false;
        let mut selected_changed = false;

        for event in events {
            match &event {
                DataViewTypedEvent::HighlightChanged { row_id: Some(id) }
                | DataViewTypedEvent::Activated { row_id: id } => {
                    selected_changed |= self.select_task(&id);
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

    fn select_task(&mut self, id: &str) -> bool {
        let outcome = self
            .context
            .store
            .borrow_mut()
            .dispatch(AppEvent::SelectTask(id.to_string()));
        if outcome.changed {
            let store = self.context.store.borrow();
            let state = store.state();
            let selected_task = state.tasks.iter().find(|task| task.id == id);
            let save_error = selected_task.and_then(|task| state.task_save_error(&task.id));
            self.split.second_mut().set_task(
                selected_task,
                &state.people,
                &state.projects,
                save_error,
            );
        }
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

    fn sync_detail_events(&mut self, ctx: &mut EventCtx<()>) {
        if self.drain_detail_patches() {
            let store = self.context.store.borrow();
            let state = store.state();
            self.split.first_mut().set_rows(state.tasks.clone());
            self.observed_version = state.version;
            ctx.request_redraw();
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
            self.context
                .store
                .borrow_mut()
                .dispatch(AppEvent::SaveCompleted {
                    target: SaveTarget::task(result.task_id, result.field),
                    error: result.error,
                });
            changed = true;
        }
        changed
    }
}

impl TuiNode for TaskWorkspace {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync_store_version();
        self.split.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.split.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let outcome = self.split.event(event, ctx);
        self.sync_detail_events(ctx);
        self.sync_table_events(ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let outcome = self.split.dispatch_event(route, event, ctx);
        self.sync_detail_events(ctx);
        self.sync_table_events(ctx);
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.split.dispatch_focus(target, focused, ctx);
        if self.drain_detail_patches() {
            ctx.request_redraw();
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let mut result = self.split.tick(dt, settings);
        if self.poll_save_results() {
            result = result.merge(TickResult::CHANGED);
        }
        if !self.latest_save_seq.is_empty() {
            result = result.merge(TickResult::scheduled_after(Duration::from_millis(50)));
        }
        result
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.split.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.split.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.split.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
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
        let version = self.context.store.borrow().state().version;
        if self.observed_version != version {
            self.split = person_split(&self.context.store);
            self.observed_version = version;
        }
    }

    fn sync_table_events(&mut self, ctx: &mut EventCtx<()>) {
        let events = self.split.first_mut().take_events();
        let mut focus_detail = false;
        let mut selected_changed = false;
        for event in events {
            match &event {
                DataViewTypedEvent::HighlightChanged { row_id: Some(id) }
                | DataViewTypedEvent::Activated { row_id: id } => {
                    selected_changed |= self.select_person(&id);
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

    fn select_person(&mut self, id: &str) -> bool {
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
                .set_person(selected_person, save_error);
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

    fn sync_detail_events(&mut self, ctx: &mut EventCtx<()>) {
        if self.drain_detail_patches() {
            let store = self.context.store.borrow();
            let state = store.state();
            self.split.first_mut().set_rows(state.people.clone());
            self.observed_version = state.version;
            ctx.request_redraw();
        }
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
            self.context
                .store
                .borrow_mut()
                .dispatch(AppEvent::SaveCompleted {
                    target: SaveTarget::person(result.person_id, result.field),
                    error: result.error,
                });
            changed = true;
        }
        changed
    }
}

impl TuiNode for PeopleWorkspace {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync_store_version();
        self.split.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.split.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let outcome = self.split.event(event, ctx);
        self.sync_detail_events(ctx);
        self.sync_table_events(ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let outcome = self.split.dispatch_event(route, event, ctx);
        self.sync_detail_events(ctx);
        self.sync_table_events(ctx);
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.split.dispatch_focus(target, focused, ctx);
        if self.drain_detail_patches() {
            ctx.request_redraw();
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let mut result = self.split.tick(dt, settings);
        if self.poll_save_results() {
            result = result.merge(TickResult::CHANGED);
        }
        if !self.latest_save_seq.is_empty() {
            result = result.merge(TickResult::scheduled_after(Duration::from_millis(50)));
        }
        result
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.split.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.split.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.split.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
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
        let version = self.context.store.borrow().state().version;
        if self.observed_version != version {
            self.split = project_split(&self.context.store);
            self.observed_version = version;
        }
    }

    fn sync_table_events(&mut self, ctx: &mut EventCtx<()>) {
        let events = self.split.first_mut().take_events();
        let mut focus_detail = false;
        let mut selected_changed = false;
        for event in events {
            match &event {
                DataViewTypedEvent::HighlightChanged { row_id: Some(id) }
                | DataViewTypedEvent::Activated { row_id: id } => {
                    selected_changed |= self.select_project(&id);
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

    fn select_project(&mut self, id: &str) -> bool {
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
                .set_project(selected_project, &state.people, save_error);
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

    fn sync_detail_events(&mut self, ctx: &mut EventCtx<()>) {
        if self.drain_detail_patches() {
            let store = self.context.store.borrow();
            let state = store.state();
            self.split.first_mut().set_rows(state.projects.clone());
            self.observed_version = state.version;
            ctx.request_redraw();
        }
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
            self.context
                .store
                .borrow_mut()
                .dispatch(AppEvent::SaveCompleted {
                    target: SaveTarget::project(result.project_id, result.field),
                    error: result.error,
                });
            changed = true;
        }
        changed
    }
}

impl TuiNode for ProjectsWorkspace {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync_store_version();
        self.split.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.split.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        let outcome = self.split.event(event, ctx);
        self.sync_detail_events(ctx);
        self.sync_table_events(ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        let outcome = self.split.dispatch_event(route, event, ctx);
        self.sync_detail_events(ctx);
        self.sync_table_events(ctx);
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.split.dispatch_focus(target, focused, ctx);
        if self.drain_detail_patches() {
            ctx.request_redraw();
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let mut result = self.split.tick(dt, settings);
        if self.poll_save_results() {
            result = result.merge(TickResult::CHANGED);
        }
        if !self.latest_save_seq.is_empty() {
            result = result.merge(TickResult::scheduled_after(Duration::from_millis(50)));
        }
        result
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.split.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.split.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.split.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.split.destroy(ctx);
    }
}

struct TaskDetailForm {
    root: Flex,
    task_id: Option<String>,
    patches: PatchSink,
}

impl TaskDetailForm {
    fn new(
        task: Option<&TaskRow>,
        people: &[Person],
        projects: &[Project],
        save_error: Option<&str>,
    ) -> Self {
        let patches = Rc::new(RefCell::new(Vec::new()));
        Self {
            root: detail_form(task, people, projects, Rc::clone(&patches), save_error),
            task_id: task.map(|task| task.id.clone()),
            patches,
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
    ) {
        self.patches = Rc::new(RefCell::new(Vec::new()));
        self.task_id = task.map(|task| task.id.clone());
        self.root = detail_form(task, people, projects, Rc::clone(&self.patches), save_error);
    }
}

impl TuiNode for TaskDetailForm {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.root.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.root.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if detail_escape(event) {
            focus_task_table(ctx);
            return EventOutcome::Handled;
        }
        if tab_navigation_event(event) {
            return EventOutcome::Ignored;
        }
        let outcome = self.root.event(event, ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if detail_escape(event) {
            focus_task_table(ctx);
            return EventOutcome::Handled;
        }
        if tab_navigation_event(event) {
            return EventOutcome::Ignored;
        }
        let outcome = self.root.dispatch_event(route, event, ctx);
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.root.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.root.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.destroy(ctx);
    }
}

struct PersonDetailForm {
    root: Flex,
    person_id: Option<String>,
    patches: PersonPatchSink,
}

impl PersonDetailForm {
    fn new(person: Option<&Person>, save_error: Option<&str>) -> Self {
        let patches = Rc::new(RefCell::new(Vec::new()));
        Self {
            root: person_detail_form(person, Rc::clone(&patches), save_error),
            person_id: person.map(|person| person.id.clone()),
            patches,
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

    fn set_person(&mut self, person: Option<&Person>, save_error: Option<&str>) {
        self.patches = Rc::new(RefCell::new(Vec::new()));
        self.person_id = person.map(|person| person.id.clone());
        self.root = person_detail_form(person, Rc::clone(&self.patches), save_error);
    }
}

impl TuiNode for PersonDetailForm {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.root.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.root.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if detail_escape(event) {
            focus_task_table(ctx);
            return EventOutcome::Handled;
        }
        if tab_navigation_event(event) {
            return EventOutcome::Ignored;
        }
        let outcome = self.root.event(event, ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if detail_escape(event) {
            focus_task_table(ctx);
            return EventOutcome::Handled;
        }
        if tab_navigation_event(event) {
            return EventOutcome::Ignored;
        }
        let outcome = self.root.dispatch_event(route, event, ctx);
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.root.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.root.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.destroy(ctx);
    }
}

struct ProjectDetailForm {
    root: Flex,
    project_id: Option<String>,
    patches: ProjectPatchSink,
}

impl ProjectDetailForm {
    fn new(project: Option<&Project>, people: &[Person], save_error: Option<&str>) -> Self {
        let patches = Rc::new(RefCell::new(Vec::new()));
        Self {
            root: project_detail_form(project, people, Rc::clone(&patches), save_error),
            project_id: project.map(|project| project.id.clone()),
            patches,
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
    ) {
        self.patches = Rc::new(RefCell::new(Vec::new()));
        self.project_id = project.map(|project| project.id.clone());
        self.root = project_detail_form(project, people, Rc::clone(&self.patches), save_error);
    }
}

impl TuiNode for ProjectDetailForm {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.root.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.root.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<()>) -> EventOutcome {
        if detail_escape(event) {
            focus_task_table(ctx);
            return EventOutcome::Handled;
        }
        if tab_navigation_event(event) {
            return EventOutcome::Ignored;
        }
        let outcome = self.root.event(event, ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<()>,
    ) -> EventOutcome {
        if detail_escape(event) {
            focus_task_table(ctx);
            return EventOutcome::Handled;
        }
        if tab_navigation_event(event) {
            return EventOutcome::Ignored;
        }
        let outcome = self.root.dispatch_event(route, event, ctx);
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<()>) {
        self.root.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.root.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<()>) {
        self.root.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<()>) {
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
    save_error: Option<&str>,
) -> Flex {
    let Some(person) = person else {
        return Flex::column().child(
            "empty",
            Paragraph::new("No person selected."),
            FlexItem::fixed(1),
        );
    };
    let status = save_error.unwrap_or("Saved changes update immediately.");

    Flex::column()
        .gap(0)
        .child("save-status", Paragraph::new(status), FlexItem::fixed(1))
        .child(
            "name",
            TextInput::new()
                .value(person.name.clone())
                .panel("Name")
                .on_submit({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| patch_sink.borrow_mut().push(PersonPatch::Name(value))
                })
                .on_blur({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| patch_sink.borrow_mut().push(PersonPatch::Name(value))
                }),
            FlexItem::fixed(3),
        )
        .child(
            "email",
            TextInput::new()
                .value(person.email.clone())
                .panel("Email")
                .on_submit({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| patch_sink.borrow_mut().push(PersonPatch::Email(value))
                })
                .on_blur({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| patch_sink.borrow_mut().push(PersonPatch::Email(value))
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
    save_error: Option<&str>,
) -> Flex {
    let Some(project) = project else {
        return Flex::column().child(
            "empty",
            Paragraph::new("No project selected."),
            FlexItem::fixed(1),
        );
    };
    let status = save_error.unwrap_or("Saved changes update immediately.");

    Flex::column()
        .gap(0)
        .child("save-status", Paragraph::new(status), FlexItem::fixed(1))
        .child(
            "key",
            TextInput::new()
                .value(project.key.clone())
                .panel("Key")
                .on_submit({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| patch_sink.borrow_mut().push(ProjectPatch::Key(value))
                })
                .on_blur({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| patch_sink.borrow_mut().push(ProjectPatch::Key(value))
                }),
            FlexItem::fixed(3),
        )
        .child(
            "name",
            TextInput::new()
                .value(project.name.clone())
                .panel("Name")
                .on_submit({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| patch_sink.borrow_mut().push(ProjectPatch::Name(value))
                })
                .on_blur({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| patch_sink.borrow_mut().push(ProjectPatch::Name(value))
                }),
            FlexItem::fixed(3),
        )
        .child(
            "description",
            TextareaInput::new()
                .value(project.description.clone())
                .panel("Description")
                .on_submit({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| {
                        patch_sink
                            .borrow_mut()
                            .push(ProjectPatch::Description(value))
                    }
                })
                .on_blur({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| {
                        patch_sink
                            .borrow_mut()
                            .push(ProjectPatch::Description(value))
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
    save_error: Option<&str>,
) -> Flex {
    let Some(task) = task else {
        return Flex::column().child(
            "empty",
            Paragraph::new("No task selected."),
            FlexItem::fixed(1),
        );
    };

    let status = save_error.unwrap_or("Saved changes update immediately.");

    Flex::column()
        .gap(0)
        .child("save-status", Paragraph::new(status), FlexItem::fixed(1))
        .child(
            "title",
            TextInput::new()
                .value(task.title.clone())
                .panel("Title")
                .hotkey(keys::TASK_TITLE_FIELD.hotkey())
                .on_submit({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| patch_sink.borrow_mut().push(TaskPatch::Title(value))
                })
                .on_blur({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| patch_sink.borrow_mut().push(TaskPatch::Title(value))
                }),
            FlexItem::fixed(3),
        )
        .child(
            "description",
            TextareaInput::new()
                .value(task.detail.clone())
                .panel("Description")
                .hotkey(keys::TASK_DESCRIPTION_FIELD.hotkey())
                .on_submit({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| patch_sink.borrow_mut().push(TaskPatch::Detail(value))
                })
                .on_blur({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |value| patch_sink.borrow_mut().push(TaskPatch::Detail(value))
                })
                .min_rows(4)
                .max_rows(8),
            FlexItem::fixed(6),
        )
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
            DatePickerDropdown::new()
                .value(parse_date(task.start_date.as_deref()))
                .panel("Start date")
                .hotkey(keys::TASK_START_DATE_FIELD.hotkey())
                .on_select({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |date| {
                        patch_sink
                            .borrow_mut()
                            .push(TaskPatch::StartDate(Some(date.to_string())));
                    }
                }),
            FlexItem::fixed(3),
        )
        .child(
            "end-date",
            DatePickerDropdown::new()
                .value(parse_date(task.due_date.as_deref()))
                .panel("End date")
                .hotkey(keys::TASK_END_DATE_FIELD.hotkey())
                .on_select({
                    let patch_sink = Rc::clone(&patch_sink);
                    move |date| {
                        patch_sink
                            .borrow_mut()
                            .push(TaskPatch::EndDate(Some(date.to_string())));
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

fn tab_navigation_event(event: &TuiEvent) -> bool {
    let TuiEvent::Key(key) = event else {
        return false;
    };
    key.modifiers == KeyModifiers::NONE && matches!(key.code, Key::Char('[' | ']'))
}

fn focus_task_table(ctx: &mut EventCtx<()>) {
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
