use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    error::Error,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use crate::app_keymap::{self, keys};
use crate::calendar::{
    CalendarCreateContext, CalendarWorkspace, SHOW_WEEKENDS_SETTING, parse_show_weekends_setting,
};
use crate::create_management_dialog::{CreateManagementDialog, ManagementEntityDraft};
use crate::create_task_dialog::{CreateTaskDialog, CreateTaskDraft};
use crate::domain::{
    AppEvent, AppState, DEFAULT_WORKSPACE_SETTING, Person, Tag, Task, TaskPatch, TaskPriority,
    TaskRank, TaskSize, TaskState, Workspace, reduce_app_state, task_display_id, task_identifier,
    task_number,
};
use crate::note_quick_menu::NoteQuickMenu;
use crate::notes_config::{DEFAULT_NOTE_EDITING_SETTING, NoteEditingMode, parse_note_editing_mode};
use crate::persistence_coordinator::{
    AppStore, PersistenceCommand, PersistenceCoordinator, PersistenceSelectionInvocation,
    PersistenceSelectionSource,
};
use crate::service::{NoteView, TuidoService, Versioned};
use crate::settings_dialog::SettingsDialog;
use crate::snooze::{
    DEFAULT_SNOOZE_TIME_SETTING, SnoozeDialog, format_datetime, format_default_snooze_time,
    local_now, parse_default_snooze_time,
};
use crate::speed_reader_settings::{
    MAX_MARKDOWN_BLOCK_PAUSE_MS, MAX_SPEED_READER_WPM, MIN_SPEED_READER_WPM,
    SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING, SPEED_READER_WPM_SETTING, SpeedReaderSettings,
    format_markdown_block_pause, format_speed_reader_wpm, parse_markdown_block_pause,
    parse_speed_reader_wpm,
};
use crate::storage::Storage;
use crate::task_quick_menu::{TaskQuickClipboard, TaskQuickMenu};
use crate::task_title::format_title;
use crate::ui::management::{ManagementDialogKind, people, tags, workspaces};
use crate::ui::notes_workspace::{
    NewNoteEditRequest, NoteChange, NotesWorkspace, note_path, note_placeholder,
};
use crate::ui::responsive_split::ResponsiveSplit;
use crate::ui::save_status::SaveStatusLine;
use crate::ui::task_detail::{PatchSink, TaskDetailCatalogs, TaskDetailForm};
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
};
use time::{Date, PrimitiveDateTime, Time};
use tuicore::{
    ActivationMode, AnimationSettings, AxisProposal, Button, CellContext, ChildKey, ChipColorRole,
    Column, ConfirmationDialog, ConfirmationDialogOutcome, CrossAlign, DataView,
    DataViewTypedEvent, DateTimePickerDropdown, Dialog, DialogBackdrop, DialogHost, DialogLayer,
    Dropdown, DropdownCommitMode, DropdownSearchMode, DropdownVariant, EventCtx, EventOutcome,
    EventRoute, Flex, FlexItem, FocusCtx, FocusId, FocusRequest, FocusTarget, HotkeyEvent,
    HotkeyLabelMode, Language, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint,
    LifecycleCtx, ListControl, ListControlEvent, ListControlField, ListControlKeyBindings,
    MainAlign, MenuButton, MenuItem, Paragraph, Propagation, RenderCtx, SeasonalEmptyState,
    SelectedTag, SelectionMode, SelectionTrigger, SpeedReader, StatusBar, StatusBarMenuItem, Store,
    Tab, Tabs, TabsVariant, TagInput, TagInputEvent, TextareaInput, TickResult, TreeApp, TreePath,
    TuiEvent, TuiNode, WeatherProviderConfig,
};
use uuid::Uuid;

mod task_checklist_input;
mod task_copy;
mod task_links_input;
mod task_relations_input;
mod task_title_input;

use task_checklist_input::TaskChecklistInput;
use task_copy::TaskCopyContext;
#[cfg(test)]
use task_links_input::LinkOpenMode;
use task_links_input::TaskLinksInput;
use task_relations_input::TaskRelationsInput;
use task_title_input::TaskTitleInput;

const PEOPLE_MENU_ID: &str = "people";
const WORKSPACES_MENU_ID: &str = "workspaces";
const TAGS_MENU_ID: &str = "tags";
const SETTINGS_MENU_ID: &str = "settings";
const TASKS_TAB_INDEX: usize = 0;
const CALENDAR_TAB_INDEX: usize = 1;
const NOTES_TAB_INDEX: usize = 2;
static NEXT_PENDING_TASK_ID: AtomicU64 = AtomicU64::new(1);
const LINK_TITLE_STALE_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const STATUS_BAR_MENU_ITEMS: [StatusBarMenuItem; 6] = [
    StatusBarMenuItem::Custom {
        id: SETTINGS_MENU_ID,
        label: " Settings",
    },
    StatusBarMenuItem::Custom {
        id: PEOPLE_MENU_ID,
        label: " People",
    },
    StatusBarMenuItem::Custom {
        id: WORKSPACES_MENU_ID,
        label: "󰲋 Spaces",
    },
    StatusBarMenuItem::Custom {
        id: TAGS_MENU_ID,
        label: " Tags",
    },
    StatusBarMenuItem::Theme,
    StatusBarMenuItem::WeatherForecast,
];

fn weather_provider_config() -> WeatherProviderConfig {
    WeatherProviderConfig::new().enabled(true)
}

fn default_snooze_time() -> Time {
    parse_default_snooze_time(None).expect("default snooze time should be valid")
}

fn stale_link_urls(task: &Task) -> Vec<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    task.links
        .iter()
        .filter(|link| {
            link.last_fetched
                .as_deref()
                .and_then(|value| value.parse::<u128>().ok())
                .is_none_or(|fetched| {
                    now.saturating_sub(fetched) >= LINK_TITLE_STALE_AFTER.as_nanos()
                })
        })
        .map(|link| link.url.clone())
        .collect()
}

fn seed_app_setting(state: &mut AppState, key: &str, value: String) {
    for values in [
        &mut state.app_setting_values,
        &mut state.app_setting_confirmed_values,
        &mut state.app_setting_desired_values,
    ] {
        values.insert(key.to_string(), value.clone());
    }
}

async fn load_speed_reader_settings(
    service: &TuidoService,
) -> Result<SpeedReaderSettings, Box<dyn Error>> {
    let wpm = parse_speed_reader_wpm(
        service
            .app_setting(SPEED_READER_WPM_SETTING)
            .await?
            .as_deref(),
    )
    .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidData, message))?;
    let markdown_block_pause = parse_markdown_block_pause(
        service
            .app_setting(SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING)
            .await?
            .as_deref(),
    )
    .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidData, message))?;
    Ok(SpeedReaderSettings {
        wpm,
        markdown_block_pause,
    })
}

#[derive(Debug)]
pub(crate) enum AppMsg {
    Noop,
    OpenSettings,
    SetShowCalendarWeekends(bool),
    SetDefaultSnoozeTime(Time),
    SetDefaultWorkspace(Option<String>),
    SetDefaultNoteEditing(NoteEditingMode),
    SetSpeedReaderWpm(String),
    SetMarkdownBlockPause(String),
    SetNotesZoom(crate::notes_config::NotesZoomLevels),
    CreateNote,
    PatchNote(NoteChange),
    OpenNoteQuickMenu(String),
    OpenDeleteNote(String),
    DeleteNoteConfirmed(String),
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
    OpenCreateTask {
        calendar_date: Option<Date>,
    },
    CreateTaskSubmitted(CreateTaskDraft),
    ScheduleCreatedTask {
        until: PrimitiveDateTime,
        remember_custom: Option<PrimitiveDateTime>,
    },
    OpenDeleteTask {
        task_id: String,
        return_focus: Option<TreePath>,
    },
    OpenCalendarDeleteTask {
        task_id: String,
        return_focus: Option<TreePath>,
    },
    DeleteTaskConfirmed(String),
    OpenDeleteTasks(Vec<String>),
    DeleteTasksConfirmed(Vec<String>),
    SelectionAction {
        invocation: PersistenceSelectionInvocation,
        action: Box<AppMsg>,
    },
    OpenTaskQuickMenu(String),
    OpenTasksQuickMenu(Vec<String>),
    OpenCalendarTasksQuickMenu {
        task_ids: Vec<String>,
        time: Option<PrimitiveDateTime>,
        selection_active: bool,
    },
    CopyTaskClipboard(String),
    OpenCalendarDeleteTasks(Vec<String>),
    OpenCalendarTasksSnooze(Vec<String>),
    OpenCalendarCompleteTasks(Vec<String>),
    MoveTaskToTop(String),
    MoveTaskToBottom(String),
    MoveTaskToTopAtTime {
        task_id: String,
        time: PrimitiveDateTime,
    },
    MoveTaskToBottomAtTime {
        task_id: String,
        time: PrimitiveDateTime,
    },
    MoveTasksToTop(Vec<String>),
    MoveTasksToBottom(Vec<String>),
    MoveTasksToTopAtTime {
        task_ids: Vec<String>,
        time: PrimitiveDateTime,
    },
    MoveTasksToBottomAtTime {
        task_ids: Vec<String>,
        time: PrimitiveDateTime,
    },
    OpenTaskSnooze {
        task_id: String,
        return_focus: Option<SnoozeReturnFocus>,
    },
    OpenTasksSnooze(Vec<String>),
    OpenCompleteTask {
        task_id: String,
        return_focus: Option<TreePath>,
    },
    OpenCalendarCompleteTask {
        task_id: String,
        return_focus: Option<TreePath>,
    },
    OpenCompleteTasks(Vec<String>),
    CompleteTask {
        task_id: String,
        state: TaskState,
    },
    CompleteTasks {
        task_ids: Vec<String>,
        state: TaskState,
    },
    CompleteCalendarTasks {
        task_ids: Vec<String>,
        state: TaskState,
    },
    ToggleTaskProgress(String),
    ToggleCalendarTaskProgress(String),
    NavigateToTask {
        source_task_id: String,
        target_task_id: String,
    },
    FetchTaskLinkTitle {
        task_id: String,
        url: String,
    },
    OpenTaskLink {
        url: String,
        background: bool,
    },
    OpenDescriptionSpeedReader(String),
    OpenNoteSpeedReader(String),
    SnoozeTask {
        task_id: String,
        until: PrimitiveDateTime,
        remember_custom: Option<PrimitiveDateTime>,
    },
    UnsnoozeTask(String),
    SnoozeTasks {
        task_ids: Vec<String>,
        until: PrimitiveDateTime,
        remember_custom: Option<PrimitiveDateTime>,
    },
    UnsnoozeTasks(Vec<String>),
    CloseManagementOverlay,
    CloseSnoozeDialog,
    CloseDeleteTaskDialog,
    CloseCompleteTaskDialog,
    CloseDialog,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SnoozeReturnFocus {
    Path(TreePath),
    CalendarDay {
        path: TreePath,
        date: Date,
        has_other_tasks: bool,
    },
}

impl SnoozeReturnFocus {
    fn path(&self) -> &TreePath {
        match self {
            Self::Path(path) | Self::CalendarDay { path, .. } => path,
        }
    }
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
    let default_snooze_time = parse_default_snooze_time(
        runtime
            .block_on(service.app_setting(DEFAULT_SNOOZE_TIME_SETTING))?
            .as_deref(),
    )
    .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidData, message))?;
    let default_workspace_id = runtime.block_on(service.default_workspace_id())?;
    let default_note_editing = parse_note_editing_mode(
        runtime
            .block_on(service.app_setting(DEFAULT_NOTE_EDITING_SETTING))?
            .as_deref(),
    )
    .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidData, message))?;
    let speed_reader_settings = runtime.block_on(load_speed_reader_settings(&service))?;
    let notes_zoom = crate::notes_config::load_notes_zoom()?;
    let mut app_state = AppState::from_snapshot(workspace.snapshot);
    app_state.notes = runtime.block_on(service.list_notes())?;
    app_state.notes_version = 1;
    seed_app_setting(
        &mut app_state,
        SHOW_WEEKENDS_SETTING,
        show_calendar_weekends.to_string(),
    );
    seed_app_setting(
        &mut app_state,
        DEFAULT_SNOOZE_TIME_SETTING,
        format_default_snooze_time(default_snooze_time),
    );
    seed_app_setting(
        &mut app_state,
        DEFAULT_WORKSPACE_SETTING,
        default_workspace_id.unwrap_or_default(),
    );
    seed_app_setting(
        &mut app_state,
        DEFAULT_NOTE_EDITING_SETTING,
        default_note_editing.setting_value().to_string(),
    );
    seed_app_setting(
        &mut app_state,
        SPEED_READER_WPM_SETTING,
        format_speed_reader_wpm(speed_reader_settings.wpm),
    );
    seed_app_setting(
        &mut app_state,
        SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING,
        format_markdown_block_pause(speed_reader_settings.markdown_block_pause),
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
        notes_zoom,
    ))
    .initial_focus(initial_task_table_focus_request())
    .on_message(|app, message, ctx| match message {
        AppMsg::Noop => {}
        AppMsg::OpenSettings => app.open_settings_dialog(ctx),
        AppMsg::SetShowCalendarWeekends(show) => app.set_show_calendar_weekends(show),
        AppMsg::SetDefaultSnoozeTime(time) => app.set_default_snooze_time(time),
        AppMsg::SetDefaultWorkspace(workspace_id) => app.set_default_workspace(workspace_id),
        AppMsg::SetDefaultNoteEditing(mode) => app.set_default_note_editing(mode),
        AppMsg::SetSpeedReaderWpm(value) => app.set_speed_reader_wpm(value, ctx),
        AppMsg::SetMarkdownBlockPause(value) => app.set_markdown_block_pause(value, ctx),
        AppMsg::SetNotesZoom(zoom) => app.set_notes_zoom(zoom, ctx),
        AppMsg::CreateNote => app.create_note(ctx),
        AppMsg::PatchNote(change) => app.patch_note(change),
        AppMsg::OpenNoteQuickMenu(note_id) => app.open_note_quick_menu(note_id, ctx),
        AppMsg::OpenDeleteNote(id) => app.open_delete_note_dialog(id, ctx),
        AppMsg::DeleteNoteConfirmed(id) => app.delete_note(id, ctx),
        AppMsg::OpenManagementDialog(kind) => app.open_management_dialog(kind, ctx),
        AppMsg::OpenCreateManagement(kind) => app.open_create_management_dialog(kind, ctx),
        AppMsg::CreateManagementSubmitted(draft) => app.submit_create_management(draft, ctx),
        AppMsg::OpenDeleteManagement { kind, entity_id } => {
            app.open_delete_management_dialog(kind, &entity_id, ctx)
        }
        AppMsg::DeleteManagementConfirmed { kind, entity_id } => {
            app.delete_management(kind, &entity_id, ctx)
        }
        AppMsg::OpenCreateTask { calendar_date } => app.open_create_task_dialog(calendar_date, ctx),
        AppMsg::CreateTaskSubmitted(draft) => app.submit_create_task(draft, ctx),
        AppMsg::ScheduleCreatedTask {
            until,
            remember_custom,
        } => app.schedule_created_task(until, remember_custom, ctx),
        AppMsg::OpenDeleteTask {
            task_id,
            return_focus,
        } => app.open_delete_task_dialog(&task_id, return_focus, ctx),
        AppMsg::OpenCalendarDeleteTask {
            task_id,
            return_focus,
        } => {
            app.begin_calendar_bulk_action();
            app.open_delete_task_dialog(&task_id, return_focus, ctx);
        }
        AppMsg::DeleteTaskConfirmed(task_id) => app.delete_task(task_id, ctx),
        AppMsg::OpenDeleteTasks(task_ids) => app.open_delete_tasks_dialog(&task_ids, None, ctx),
        AppMsg::DeleteTasksConfirmed(task_ids) => app.delete_tasks(task_ids, None, ctx),
        AppMsg::SelectionAction { invocation, action } => {
            app.dispatch_selection_action(invocation, *action, ctx)
        }
        AppMsg::OpenTaskQuickMenu(task_id) => app.open_task_quick_menu(&task_id, ctx),
        AppMsg::OpenTasksQuickMenu(task_ids) => app.open_tasks_quick_menu(task_ids, None, ctx),
        AppMsg::OpenCalendarTasksQuickMenu {
            task_ids,
            time,
            selection_active,
        } => app.open_calendar_tasks_quick_menu(task_ids, time, selection_active, None, ctx),
        AppMsg::CopyTaskClipboard(payload) => app.copy_task_clipboard(payload, None, ctx),
        AppMsg::OpenCalendarDeleteTasks(task_ids) => {
            app.begin_calendar_bulk_action();
            app.open_delete_tasks_dialog(&task_ids, None, ctx);
        }
        AppMsg::OpenCalendarTasksSnooze(task_ids) => {
            app.begin_calendar_bulk_action();
            app.open_tasks_snooze_dialog(task_ids, None, ctx);
        }
        AppMsg::OpenCalendarCompleteTasks(task_ids) => {
            app.begin_calendar_bulk_action();
            app.open_complete_tasks_dialog(&task_ids, None, ctx);
        }
        AppMsg::MoveTaskToTop(task_id) => app.move_task_to_edge(&task_id, true, ctx),
        AppMsg::MoveTaskToBottom(task_id) => app.move_task_to_edge(&task_id, false, ctx),
        AppMsg::MoveTaskToTopAtTime { task_id, time } => {
            app.move_calendar_task_to_edge(&task_id, time, true, ctx)
        }
        AppMsg::MoveTaskToBottomAtTime { task_id, time } => {
            app.move_calendar_task_to_edge(&task_id, time, false, ctx)
        }
        AppMsg::MoveTasksToTop(task_ids) => app.move_tasks_to_edge(task_ids, true, None, ctx),
        AppMsg::MoveTasksToBottom(task_ids) => app.move_tasks_to_edge(task_ids, false, None, ctx),
        AppMsg::MoveTasksToTopAtTime { task_ids, time } => {
            app.move_calendar_tasks_to_edge(task_ids, time, true, None, ctx)
        }
        AppMsg::MoveTasksToBottomAtTime { task_ids, time } => {
            app.move_calendar_tasks_to_edge(task_ids, time, false, None, ctx)
        }
        AppMsg::OpenTaskSnooze {
            task_id,
            return_focus,
        } => app.open_task_snooze_dialog(&task_id, return_focus, ctx),
        AppMsg::OpenTasksSnooze(task_ids) => app.open_tasks_snooze_dialog(task_ids, None, ctx),
        AppMsg::SnoozeTask {
            task_id,
            until,
            remember_custom,
        } => app.snooze_task(task_id, until, remember_custom, ctx),
        AppMsg::UnsnoozeTask(task_id) => app.unsnooze_task(task_id, ctx),
        AppMsg::SnoozeTasks {
            task_ids,
            until,
            remember_custom,
        } => app.snooze_tasks(task_ids, until, remember_custom, None, ctx),
        AppMsg::UnsnoozeTasks(task_ids) => app.unsnooze_tasks(task_ids, None, ctx),
        AppMsg::OpenCompleteTask {
            task_id,
            return_focus,
        } => {
            if app.calendar_bulk_origin {
                app.open_calendar_complete_task_dialog(&task_id, return_focus, ctx);
            } else {
                app.open_complete_task_dialog(&task_id, return_focus, ctx);
            }
        }
        AppMsg::OpenCalendarCompleteTask {
            task_id,
            return_focus,
        } => app.open_calendar_complete_task_dialog(&task_id, return_focus, ctx),
        AppMsg::OpenCompleteTasks(task_ids) => app.open_complete_tasks_dialog(&task_ids, None, ctx),
        AppMsg::CompleteTask { task_id, state } => app.complete_task(task_id, state, ctx),
        AppMsg::CompleteTasks { task_ids, state } => app.complete_tasks(task_ids, state, None, ctx),
        AppMsg::CompleteCalendarTasks { task_ids, state } => {
            app.begin_calendar_bulk_action();
            app.complete_tasks(task_ids, state, None, ctx);
        }
        AppMsg::ToggleTaskProgress(task_id) => app.toggle_task_progress(task_id, ctx),
        AppMsg::ToggleCalendarTaskProgress(task_id) => {
            app.begin_calendar_bulk_action();
            app.toggle_task_progress(task_id, ctx);
        }
        AppMsg::NavigateToTask {
            source_task_id,
            target_task_id,
        } => app.navigate_to_task(source_task_id, target_task_id, ctx),
        AppMsg::FetchTaskLinkTitle { task_id, url } => app
            .context
            .coordinator
            .borrow_mut()
            .fetch_task_link_title(task_id, url),
        AppMsg::OpenTaskLink { url, background } => app
            .context
            .coordinator
            .borrow_mut()
            .submit(PersistenceCommand::OpenBrowserLink { url, background }),
        AppMsg::OpenDescriptionSpeedReader(description) => {
            app.open_description_speed_reader(description, ctx)
        }
        AppMsg::OpenNoteSpeedReader(note) => app.open_note_speed_reader(note, ctx),
        AppMsg::CloseManagementOverlay => app.close_management_overlay(ctx),
        AppMsg::CloseSnoozeDialog => app.close_snooze_dialog(ctx),
        AppMsg::CloseDeleteTaskDialog => app.close_delete_task_dialog(ctx),
        AppMsg::CloseCompleteTaskDialog => app.close_complete_task_dialog(ctx),
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

struct TrackedTabs {
    tabs: Tabs<AppMsg>,
    selected: Rc<Cell<usize>>,
}

impl TrackedTabs {
    fn new(tabs: Tabs<AppMsg>, selected: Rc<Cell<usize>>) -> Self {
        selected.set(tabs.selected_index());
        Self { tabs, selected }
    }

    fn sync_selected(&self) {
        self.selected.set(self.tabs.selected_index());
    }

    fn sync_requested(&mut self) {
        let selected = self.selected.get();
        if self.tabs.selected_index() != selected {
            self.tabs.select_index(selected);
        }
    }
}

impl TuiNode<AppMsg> for TrackedTabs {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.tabs.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync_requested();
        self.tabs.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.tabs.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        let outcome = self.tabs.event(event, ctx);
        self.sync_selected();
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let outcome = self.tabs.dispatch_event(route, event, ctx);
        self.sync_selected();
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        self.tabs.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.tabs.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.tabs.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.tabs.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.tabs.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.tabs.destroy(ctx);
    }
}

type PrimaryDialogLayer = DialogLayer<Flex<AppMsg>, AppDialog>;
type AppDialogLayers = DialogLayer<PrimaryDialogLayer, AppDialog>;

struct App {
    root: AppDialogLayers,
    context: AppContext,
    calendar_create_context: CalendarCreateContext,
    create_task_calendar_date: Option<Date>,
    task_creation_active: bool,
    task_creation_return_focus: Option<TreePath>,
    pending_calendar_task: Option<CreateTaskDraft>,
    snooze_return_focus: Option<SnoozeReturnFocus>,
    delete_return_focus: Option<TreePath>,
    complete_return_focus: Option<CompleteReturnFocus>,
    complete_return_to_calendar: bool,
    calendar_bulk_origin: bool,
    active_tab: Rc<Cell<usize>>,
    active_focus_path: Option<TreePath>,
    note_focus_path: Rc<RefCell<TreePath>>,
    focus_note_request: Rc<RefCell<Option<String>>>,
    new_note_edit_request: Rc<RefCell<Option<NewNoteEditRequest>>>,
    pending_task_view: TaskViewChange,
    pending_task_navigation: PendingTaskNavigation,
    pending_focus_request: Option<FocusRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompleteReturnFocus {
    task_id: String,
    task_state: TaskState,
    task_selected_on_open: bool,
    path: TreePath,
}

fn toggled_task_progress_state(state: TaskState) -> TaskState {
    match state {
        TaskState::Todo => TaskState::InProgress,
        TaskState::Backlog
        | TaskState::InProgress
        | TaskState::Done
        | TaskState::Snoozed
        | TaskState::Rejected => TaskState::Todo,
    }
}

fn toggled_tasks_progress_state(tasks: &[Task]) -> TaskState {
    if tasks.iter().all(|task| task.state == TaskState::Todo) {
        TaskState::InProgress
    } else {
        TaskState::Todo
    }
}

fn task_agent_identifier(state: &AppState, task_id: &str) -> Option<String> {
    let task = state.tasks.iter().find(|task| task.id == task_id)?;
    let number = task_number(&task.id)?;
    let workspace_key = task.workspace_id.as_deref().and_then(|workspace_id| {
        state
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .map(|workspace| workspace.key.as_str())
    });
    Some(task_identifier(number, workspace_key))
}

fn task_agent_command_for(state: &AppState, task_id: &str, action: &str) -> Option<String> {
    let task = state.tasks.iter().find(|task| task.id == task_id)?;
    task_agent_commands_for(state, std::slice::from_ref(task), action)
}

pub(crate) fn task_agent_commands_for(
    state: &AppState,
    tasks: &[Task],
    action: &str,
) -> Option<String> {
    let entries = tasks
        .iter()
        .map(|task| {
            let identifier = task_agent_identifier(state, &task.id)?;
            Some(format!(
                "{identifier} {}",
                TaskCopyContext::quoted_title(task)
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    (!entries.is_empty()).then(|| format!("Tuido {action} {}", entries.join("; ")))
}

pub(crate) fn task_agent_command(state: &AppState, task_id: &str) -> Option<String> {
    task_agent_command_for(state, task_id, "execute")
}

pub(crate) fn task_agent_clarify_command(state: &AppState, task_id: &str) -> Option<String> {
    task_agent_command_for(state, task_id, "clarify")
}

pub(crate) fn task_reference(state: &AppState, task_id: &str) -> Option<String> {
    let task = state.tasks.iter().find(|task| task.id == task_id)?;
    Some(TaskCopyContext::new(&state.workspaces).reference(task))
}

pub(crate) fn task_references_for(state: &AppState, tasks: &[Task]) -> String {
    TaskCopyContext::new(&state.workspaces).references(tasks)
}

impl App {
    #[cfg(test)]
    fn new(store: AppStore, coordinator: Rc<RefCell<PersistenceCoordinator>>) -> Self {
        Self::new_with_calendar_weekends(
            store,
            coordinator,
            true,
            crate::notes_config::NotesZoomLevels::default(),
        )
    }

    fn new_with_calendar_weekends(
        store: AppStore,
        coordinator: Rc<RefCell<PersistenceCoordinator>>,
        show_calendar_weekends: bool,
        notes_zoom: crate::notes_config::NotesZoomLevels,
    ) -> Self {
        let context = AppContext::new(store, coordinator);
        let active_tab = Rc::new(Cell::new(0));
        let note_focus_path = Rc::new(RefCell::new(TreePath::new()));
        let focus_note_request = Rc::new(RefCell::new(None));
        let new_note_edit_request = Rc::new(RefCell::new(None));
        let pending_task_view = Rc::new(RefCell::new(None));
        let pending_task_navigation = Rc::new(RefCell::new(None));
        let active_workspace_filter = Rc::new(RefCell::new(None));
        let active_label_filter = Rc::new(RefCell::new(Vec::new()));
        let calendar_create_context = CalendarCreateContext::new();
        let tabs = Tabs::new(vec![
            Tab::new(
                "Tasks",
                TaskWorkspace::new_with_filters(
                    context.clone(),
                    Rc::clone(&active_workspace_filter),
                    Rc::clone(&active_label_filter),
                    Rc::clone(&pending_task_view),
                    Rc::clone(&pending_task_navigation),
                ),
            ),
            Tab::new(
                "Calendar",
                CalendarWorkspace::new_with_create_context_and_filters(
                    context.clone(),
                    show_calendar_weekends,
                    calendar_create_context.clone(),
                    Rc::clone(&active_workspace_filter),
                    Rc::clone(&active_label_filter),
                ),
            ),
            Tab::new(
                "Notes",
                NotesWorkspace::new()
                    .note_store(Rc::clone(&context.store))
                    .focus_path_sink(Rc::clone(&note_focus_path))
                    .focus_note_request(Rc::clone(&focus_note_request))
                    .new_note_edit_request(Rc::clone(&new_note_edit_request))
                    .zoom_levels(notes_zoom)
                    .on_zoom_change(AppMsg::SetNotesZoom)
                    .on_speed_read(AppMsg::OpenNoteSpeedReader)
                    .on_create(|| AppMsg::CreateNote)
                    .on_change(AppMsg::PatchNote)
                    .on_delete(AppMsg::OpenDeleteNote)
                    .on_quick_menu(AppMsg::OpenNoteQuickMenu),
            ),
        ])
        .selected(0)
        .variant(TabsVariant::OneRow)
        .bordered(true);
        let task_filters = TaskFilterControls::new(
            context.clone(),
            active_workspace_filter,
            active_label_filter,
            Rc::clone(&active_tab),
        );
        let new_actions = Flex::row()
            .gap(1)
            .child(
                "new-task",
                Button::new("Task")
                    .hotkey(keys::TASK_QUICK_CREATE.hotkey())
                    .hotkey_label_mode(HotkeyLabelMode::Inline)
                    .on_press({
                        let active_tab = Rc::clone(&active_tab);
                        let calendar_create_context = calendar_create_context.clone();
                        move || AppMsg::OpenCreateTask {
                            calendar_date: (active_tab.get() == CALENDAR_TAB_INDEX)
                                .then(|| calendar_create_context.selected_date()),
                        }
                    }),
                FlexItem::content(),
            )
            .child(
                "new-note",
                Button::new("Note")
                    .hotkey(keys::NOTE_QUICK_CREATE.hotkey())
                    .hotkey_label_mode(HotkeyLabelMode::Inline)
                    .on_press(|| AppMsg::CreateNote),
                FlexItem::content(),
            );
        let actions = Flex::row()
            .justify(MainAlign::SpaceBetween)
            .align(CrossAlign::Center)
            .gap(1)
            .child("new-actions", new_actions, FlexItem::content())
            .child("filters", task_filters, FlexItem::content());
        let content = Flex::column()
            .child("actions", actions, FlexItem::fixed(1))
            .child(
                "tabs",
                TrackedTabs::new(tabs, Rc::clone(&active_tab)),
                FlexItem::fill(1),
            );

        let root = Flex::column()
            .child("content", content, FlexItem::fill(1))
            .child(
                "footer",
                StatusBar::new()
                    .ai_enabled(false)
                    .menu_items(STATUS_BAR_MENU_ITEMS)
                    .weather_provider(weather_provider_config())
                    .on_custom_menu_item(|id| match id {
                        SETTINGS_MENU_ID => AppMsg::OpenSettings,
                        PEOPLE_MENU_ID => {
                            AppMsg::OpenManagementDialog(ManagementDialogKind::People)
                        }
                        WORKSPACES_MENU_ID => {
                            AppMsg::OpenManagementDialog(ManagementDialogKind::Workspaces)
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
            calendar_create_context,
            create_task_calendar_date: None,
            task_creation_active: false,
            task_creation_return_focus: None,
            pending_calendar_task: None,
            snooze_return_focus: None,
            delete_return_focus: None,
            complete_return_focus: None,
            complete_return_to_calendar: false,
            calendar_bulk_origin: false,
            active_tab,
            active_focus_path: None,
            note_focus_path,
            focus_note_request,
            new_note_edit_request,
            pending_task_view,
            pending_task_navigation,
            pending_focus_request: None,
        }
    }

    fn navigate_to_task(
        &mut self,
        source_task_id: String,
        target_task_id: String,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let state = self.context.store.borrow();
        let Some(task) = state
            .state()
            .tasks
            .iter()
            .find(|task| task.id == target_task_id)
        else {
            ctx.notify(tuicore::Notification::error(
                "Task not found",
                format!("Could not navigate to {target_task_id}."),
            ));
            return;
        };
        let view = match task.state {
            TaskState::Backlog => TaskView::Backlog,
            TaskState::Todo | TaskState::InProgress => TaskView::Active,
            TaskState::Snoozed => TaskView::Snoozed,
            TaskState::Done | TaskState::Rejected => TaskView::Archived,
        };
        drop(state);
        *self.pending_task_navigation.borrow_mut() = Some(TaskNavigation {
            target_task_id,
            source_task_id: Some(source_task_id),
            view,
        });
        self.active_tab.set(TASKS_TAB_INDEX);
        ctx.focus(issue_links_focus_request());
        ctx.request_layout();
        ctx.request_redraw();
    }

    fn primary_dialog(&mut self) -> &mut PrimaryDialogLayer {
        self.root.base_mut()
    }

    fn show_externally_started_task(&mut self, task_id: String) {
        *self.pending_task_navigation.borrow_mut() = Some(TaskNavigation {
            target_task_id: task_id,
            source_task_id: None,
            view: TaskView::Active,
        });
        self.active_tab.set(TASKS_TAB_INDEX);
        self.pending_focus_request = Some(initial_task_table_focus_request());
    }

    fn show_externally_created_backlog_task(&mut self, task_id: String) {
        *self.pending_task_navigation.borrow_mut() = Some(TaskNavigation {
            target_task_id: task_id,
            source_task_id: None,
            view: TaskView::Backlog,
        });
        self.active_tab.set(TASKS_TAB_INDEX);
        self.pending_focus_request = Some(initial_task_table_focus_request());
    }

    fn show_externally_changed_note(&mut self, note_id: String) {
        self.active_tab.set(NOTES_TAB_INDEX);
        *self.focus_note_request.borrow_mut() = Some(note_id.clone());
        self.pending_focus_request = Some(FocusRequest::Path(note_path(
            notes_workspace_focus_path(),
            &note_id,
        )));
    }

    fn route_external_navigation(&mut self) -> bool {
        let (note_id, started_task_id, created_backlog_task_id) = {
            let store = self.context.store.borrow();
            let state = store.state();
            (
                state.external_note_focus_id.clone(),
                state.external_started_task_id.clone(),
                state.external_created_backlog_task_id.clone(),
            )
        };
        let Some(route) = created_backlog_task_id
            .clone()
            .map(ExternalNavigation::CreatedBacklogTask)
            .or_else(|| started_task_id.clone().map(ExternalNavigation::StartedTask))
            .or_else(|| note_id.clone().map(ExternalNavigation::Note))
        else {
            return false;
        };

        let mut store = self.context.store.borrow_mut();
        if let Some(task_id) = created_backlog_task_id {
            store.dispatch(AppEvent::ExternalCreatedBacklogTaskHandled(task_id));
        }
        if let Some(task_id) = started_task_id {
            store.dispatch(AppEvent::ExternalStartedTaskHandled(task_id));
        }
        if let Some(note_id) = note_id {
            store.dispatch(AppEvent::ExternalNoteFocusHandled(note_id));
        }
        drop(store);

        match route {
            ExternalNavigation::CreatedBacklogTask(task_id) => {
                self.show_externally_created_backlog_task(task_id)
            }
            ExternalNavigation::StartedTask(task_id) => self.show_externally_started_task(task_id),
            ExternalNavigation::Note(note_id) => self.show_externally_changed_note(note_id),
        }
        true
    }

    fn open_settings_dialog(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let state = self.context.store.borrow();
        let show_weekends = state
            .state()
            .app_setting_values
            .get(SHOW_WEEKENDS_SETTING)
            .and_then(|value| parse_show_weekends_setting(Some(value)).ok())
            .unwrap_or(true);
        let default_time = state
            .state()
            .app_setting_values
            .get(DEFAULT_SNOOZE_TIME_SETTING)
            .and_then(|value| parse_default_snooze_time(Some(value)).ok())
            .unwrap_or(default_snooze_time());
        let workspaces = state.state().workspaces.clone();
        let default_workspace_id = state
            .state()
            .app_setting_values
            .get(DEFAULT_WORKSPACE_SETTING)
            .filter(|value| workspaces.iter().any(|workspace| workspace.id == **value))
            .cloned();
        let speed_reader_settings = speed_reader_settings(state.state());
        let default_note_editing = state
            .state()
            .app_setting_values
            .get(DEFAULT_NOTE_EDITING_SETTING)
            .and_then(|value| parse_note_editing_mode(Some(value)).ok())
            .unwrap_or_default();
        drop(state);
        let settings = SettingsDialog::new(
            Rc::clone(&self.context.store),
            show_weekends,
            default_time,
            &workspaces,
            default_workspace_id.as_deref(),
            default_note_editing,
            speed_reader_settings.wpm,
            speed_reader_settings.markdown_block_pause,
        );
        let dialog = Dialog::new()
            .top_left("Settings")
            .actions([tuicore::DialogAction::new("Close")
                .hotkey(keys::DIALOG_CANCEL.key_spec())
                .on_trigger(|| AppMsg::CloseDialog)])
            .close_on_unfocus_from_descendants(true)
            .on_close(|_| AppMsg::CloseDialog)
            .host(settings);
        let primary = self.primary_dialog();
        primary.replace_layer(AppDialog::Settings(dialog), ctx);
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
    }

    fn open_description_speed_reader(&mut self, description: String, ctx: &mut EventCtx<AppMsg>) {
        self.open_markdown_speed_reader("Description", description, ctx);
    }

    fn open_note_speed_reader(&mut self, note: String, ctx: &mut EventCtx<AppMsg>) {
        self.open_markdown_speed_reader("Note", note, ctx);
    }

    fn open_markdown_speed_reader(
        &mut self,
        title: &str,
        markdown: String,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let settings = speed_reader_settings(self.context.store.borrow().state());
        let reader = settings
            .apply(SpeedReader::markdown(markdown).title(title))
            .dialog(|_| AppMsg::CloseDialog);
        let primary = self.primary_dialog();
        primary.replace_layer(AppDialog::SpeedReader(reader), ctx);
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
    }

    fn set_show_calendar_weekends(&mut self, show: bool) {
        self.persist_app_setting(SHOW_WEEKENDS_SETTING, show.to_string());
    }

    fn set_default_snooze_time(&mut self, time: Time) {
        self.persist_app_setting(
            DEFAULT_SNOOZE_TIME_SETTING,
            format_default_snooze_time(time),
        );
    }

    fn set_default_workspace(&mut self, workspace_id: Option<String>) {
        self.persist_app_setting(DEFAULT_WORKSPACE_SETTING, workspace_id.unwrap_or_default());
    }

    fn set_default_note_editing(&mut self, mode: NoteEditingMode) {
        self.persist_app_setting(
            DEFAULT_NOTE_EDITING_SETTING,
            mode.setting_value().to_string(),
        );
    }

    fn set_speed_reader_wpm(&mut self, value: String, ctx: &mut EventCtx<AppMsg>) {
        let Ok(wpm) = parse_speed_reader_wpm(Some(&value)) else {
            ctx.notify(tuicore::Notification::warning(
                "Invalid speed reader WPM",
                format!(
                    "Enter a whole number from {MIN_SPEED_READER_WPM} to {MAX_SPEED_READER_WPM}."
                ),
            ));
            return;
        };
        self.persist_app_setting(SPEED_READER_WPM_SETTING, format_speed_reader_wpm(wpm));
    }

    fn set_markdown_block_pause(&mut self, value: String, ctx: &mut EventCtx<AppMsg>) {
        let Ok(delay) = parse_markdown_block_pause(Some(&value)) else {
            ctx.notify(tuicore::Notification::warning(
                "Invalid block delay",
                format!("Enter a whole number from 0 to {MAX_MARKDOWN_BLOCK_PAUSE_MS} ms."),
            ));
            return;
        };
        self.persist_app_setting(
            SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING,
            format_markdown_block_pause(delay),
        );
    }

    fn set_notes_zoom(
        &mut self,
        zoom: crate::notes_config::NotesZoomLevels,
        _ctx: &mut EventCtx<AppMsg>,
    ) {
        self.context
            .coordinator
            .borrow_mut()
            .submit(PersistenceCommand::SaveNotesZoom(zoom));
    }

    fn create_note(&mut self, ctx: &mut EventCtx<AppMsg>) {
        self.active_tab.set(NOTES_TAB_INDEX);
        let temporary_id = Uuid::new_v4().to_string();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_string();
        let notes = self.context.store.borrow().state().notes.clone();
        let removed_notes = notes
            .iter()
            .filter(|note| note.value.content.trim().is_empty())
            .cloned()
            .collect::<Vec<_>>();
        let focused_note_id = self.active_focus_path.as_ref().and_then(|path| {
            path.keys()
                .last()
                .and_then(|key| key.as_str().strip_prefix("panel-"))
        });
        let inherited_placeholder = removed_notes
            .iter()
            .find(|note| Some(note.value.id.as_str()) == focused_note_id)
            .or_else(|| removed_notes.first())
            .map(|note| note_placeholder(&note.value.id).to_string());
        let notes = notes
            .into_iter()
            .filter(|note| !note.value.content.trim().is_empty())
            .collect::<Vec<_>>();
        let position = notes.first().map_or(0, |note| note.value.position - 1);
        let temporary_note = Versioned {
            revision: 0,
            value: NoteView {
                id: temporary_id.clone(),
                position,
                content: String::new(),
                created_at: now.clone(),
                updated_at: now,
            },
        };
        self.context
            .store
            .borrow_mut()
            .dispatch(AppEvent::NoteCreateStarted {
                temporary_note,
                removed_notes: removed_notes.clone(),
                inherited_placeholder,
            });
        let mode = self
            .context
            .store
            .borrow()
            .state()
            .app_setting_values
            .get(DEFAULT_NOTE_EDITING_SETTING)
            .and_then(|value| parse_note_editing_mode(Some(value)).ok())
            .unwrap_or_default();
        *self.new_note_edit_request.borrow_mut() = Some(NewNoteEditRequest {
            id: temporary_id.clone(),
            mode,
            content: None,
        });
        ctx.focus(notes_first_child_focus_request());
        self.context
            .coordinator
            .borrow_mut()
            .submit(PersistenceCommand::CreateNote {
                temporary_id,
                content: String::new(),
                removed_notes,
            });
        ctx.request_layout();
        ctx.request_redraw();
    }

    fn patch_note(&mut self, change: NoteChange) {
        let current = self
            .context
            .store
            .borrow()
            .state()
            .notes
            .iter()
            .find(|note| note.value.id == change.id)
            .cloned();
        let Some(current) = current.filter(|note| note.value.content != change.content) else {
            return;
        };
        let mut before = current.clone();
        before.revision = change.revision;
        before.value.content = change.base_content;
        if current.revision != before.revision {
            self.context
                .store
                .borrow_mut()
                .dispatch(AppEvent::NotePatched {
                    before: before.clone(),
                    result: Err(format!(
                        "conflict: note changed elsewhere (expected revision {}, found {})",
                        before.revision, current.revision
                    )),
                });
        }
        let mut optimistic = before.clone();
        optimistic.value.content = change.content.clone();
        if current.revision == before.revision {
            self.context
                .store
                .borrow_mut()
                .dispatch(AppEvent::NotePatched {
                    before: before.clone(),
                    result: Ok(optimistic),
                });
        }
        self.context
            .coordinator
            .borrow_mut()
            .submit(PersistenceCommand::PatchNote {
                before,
                content: change.content,
            });
    }

    fn open_delete_note_dialog(&mut self, id: String, ctx: &mut EventCtx<AppMsg>) {
        if !self
            .context
            .store
            .borrow()
            .state()
            .notes
            .iter()
            .any(|note| note.value.id == id)
        {
            self.close_dialog(ctx);
            return;
        }
        let primary = self.primary_dialog();
        primary.replace_layer(delete_note_dialog(id), ctx);
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
    }

    fn open_note_quick_menu(&mut self, note_id: String, ctx: &mut EventCtx<AppMsg>) {
        if !self
            .context
            .store
            .borrow()
            .state()
            .notes
            .iter()
            .any(|note| note.value.id == note_id)
        {
            return;
        }
        let primary = self.primary_dialog();
        primary.replace_layer(
            AppDialog::NoteQuickMenu(Box::new(NoteQuickMenu::new(note_id))),
            ctx,
        );
        primary.set_layer_percent(40);
        primary.set_layer_cross_percent(35);
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
    }

    fn delete_note(&mut self, id: String, ctx: &mut EventCtx<AppMsg>) {
        let (note, index, return_focus) = {
            let state = self.context.store.borrow();
            let Some(index) = state
                .state()
                .notes
                .iter()
                .position(|note| note.value.id == id)
            else {
                drop(state);
                self.close_dialog(ctx);
                return;
            };
            let note = state.state().notes[index].clone();
            let return_focus = state
                .state()
                .notes
                .get(index + 1)
                .or_else(|| {
                    index
                        .checked_sub(1)
                        .and_then(|index| state.state().notes.get(index))
                })
                .map(|note| note.value.id.clone());
            (note, index, return_focus)
        };
        self.context
            .store
            .borrow_mut()
            .dispatch(AppEvent::NoteDeleted(id));
        self.context
            .coordinator
            .borrow_mut()
            .submit(PersistenceCommand::DeleteNote { note, index });
        self.close_dialog(ctx);
        if let Some(return_focus) = return_focus {
            ctx.focus(FocusRequest::Path(note_path(
                self.note_focus_path.borrow().clone(),
                &return_focus,
            )));
        } else {
            ctx.focus(app_tabs_focus_request());
        }
        ctx.request_layout();
    }

    fn persist_app_setting(&mut self, key: &str, value: String) {
        self.context
            .coordinator
            .borrow_mut()
            .submit(PersistenceCommand::SetAppSetting {
                key: key.to_string(),
                value,
                generation: 0,
            });
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
            ManagementEntityDraft::Person { name, email, about } => {
                if name.trim().is_empty() {
                    notify_required(
                        ctx,
                        "Person name required",
                        "Enter a name before creating the person.",
                    );
                    return;
                }
                let person = Person::with_about(Uuid::new_v4().to_string(), name, email, about);
                let person_name = person.name.clone();
                self.context
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::PersonCreated(person.clone()));
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::CreatePerson(person));
                ctx.notify(tuicore::Notification::success(
                    "Person created",
                    format!("“{person_name}” was created."),
                ));
            }
            ManagementEntityDraft::Workspace {
                key,
                name,
                description,
            } => {
                if !Workspace::is_valid_key(&key) {
                    notify_required(
                        ctx,
                        "Invalid space key",
                        "Use 2-5 characters without spaces.",
                    );
                    return;
                }
                if name.trim().is_empty() {
                    notify_required(
                        ctx,
                        "Space key and name required",
                        "Enter both a key and name before creating the space.",
                    );
                    return;
                }
                let workspace = Workspace::new(Uuid::new_v4().to_string(), key, name, description);
                let workspace_name = workspace.name.clone();
                self.context
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::WorkspaceCreated(workspace.clone()));
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::CreateWorkspace(workspace));
                ctx.notify(tuicore::Notification::success(
                    "Space created",
                    format!("“{workspace_name}” was created."),
                ));
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
                let tag_label = tag.label.clone();
                self.context
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::TagCreated(tag.clone()));
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::CreateTag(tag));
                ctx.notify(tuicore::Notification::success(
                    "Tag created",
                    format!("“{tag_label}” was created."),
                ));
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
                ManagementDialogKind::Workspaces => state
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.id == entity_id)
                    .map(|workspace| workspace.name.clone()),
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
                let person_name = deletion.person.name.clone();
                self.context
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::PersonDeleted(deletion.person.id.clone()));
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::DeletePerson(deletion));
                ctx.notify(tuicore::Notification::success(
                    "Person deleted",
                    format!("“{person_name}” was deleted."),
                ));
            }
            ManagementDialogKind::Workspaces => {
                let deletion = self
                    .context
                    .store
                    .borrow()
                    .state()
                    .workspace_deletion(entity_id);
                let Some(deletion) = deletion else { return };
                let workspace_name = deletion.workspace.name.clone();
                self.context
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::WorkspaceDeleted(deletion.workspace.id.clone()));
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::DeleteWorkspace(deletion));
                ctx.notify(tuicore::Notification::success(
                    "Space deleted",
                    format!("“{workspace_name}” was deleted."),
                ));
            }
            ManagementDialogKind::Tags => {
                let deletion = self.context.store.borrow().state().tag_deletion(entity_id);
                let Some(deletion) = deletion else { return };
                let tag_label = deletion.tag.label.clone();
                self.context
                    .store
                    .borrow_mut()
                    .dispatch(AppEvent::TagDeleted(deletion.tag.id.clone()));
                self.context
                    .coordinator
                    .borrow_mut()
                    .submit(PersistenceCommand::DeleteTag(deletion));
                ctx.notify(tuicore::Notification::success(
                    "Tag deleted",
                    format!("“{tag_label}” was deleted."),
                ));
            }
        }
        self.close_management_overlay(ctx);
    }

    fn open_create_task_dialog(&mut self, calendar_date: Option<Date>, ctx: &mut EventCtx<AppMsg>) {
        self.pending_calendar_task = None;
        self.create_task_calendar_date = calendar_date;
        self.task_creation_active = true;
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

        if let Some(date) = self.create_task_calendar_date {
            self.pending_calendar_task = Some(CreateTaskDraft { title });
            let now = match local_now() {
                Ok(now) => now,
                Err(error) => {
                    ctx.notify(tuicore::Notification::error(
                        "Local time unavailable",
                        format!("Cannot open schedule options: {error}"),
                    ));
                    return;
                }
            };
            let default_time = self
                .context
                .store
                .borrow()
                .state()
                .app_setting_values
                .get(DEFAULT_SNOOZE_TIME_SETTING)
                .and_then(|value| parse_default_snooze_time(Some(value)).ok())
                .unwrap_or(default_snooze_time());
            let last_custom = self.context.store.borrow().state().last_custom_snooze;
            let primary = self.primary_dialog();
            primary.replace_layer(
                AppDialog::Snooze(Box::new(SnoozeDialog::for_calendar(
                    now,
                    date.with_time(default_time),
                    last_custom,
                ))),
                ctx,
            );
            primary.set_fit_content(true);
            primary.set_active_with_context(true, ctx);
            return;
        }

        let task_id = self.create_task(CreateTaskDraft { title }, None, None, ctx);
        self.finish_task_creation(task_id, false, ctx);
    }

    fn schedule_created_task(
        &mut self,
        until: PrimitiveDateTime,
        remember_custom: Option<PrimitiveDateTime>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        if self.pending_calendar_task.is_none() {
            return;
        }
        let now = match local_now() {
            Ok(now) => now,
            Err(error) => {
                ctx.notify(tuicore::Notification::error(
                    "Local time unavailable",
                    format!("Cannot validate schedule time: {error}"),
                ));
                self.recover_calendar_scheduler(ctx);
                return;
            }
        };

        self.schedule_created_task_at(until, remember_custom, now, ctx);
    }

    fn schedule_created_task_at(
        &mut self,
        until: PrimitiveDateTime,
        remember_custom: Option<PrimitiveDateTime>,
        now: PrimitiveDateTime,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let Some(draft) = self.pending_calendar_task.clone() else {
            return;
        };
        if until <= now {
            ctx.notify(tuicore::Notification::warning(
                "Schedule time has passed",
                "Choose a future date and time.",
            ));
            self.recover_calendar_scheduler(ctx);
            return;
        }

        let task_id = self.create_task(draft, Some(until), remember_custom, ctx);
        self.finish_task_creation(task_id, true, ctx);
    }

    fn recover_calendar_scheduler(&mut self, ctx: &mut EventCtx<AppMsg>) {
        if let AppDialog::Snooze(dialog) = self.primary_dialog().layer_mut() {
            dialog.recover_after_invalid_selection(ctx);
        }
    }

    fn create_task(
        &mut self,
        draft: CreateTaskDraft,
        snoozed_until: Option<PrimitiveDateTime>,
        remember_custom: Option<PrimitiveDateTime>,
        ctx: &mut EventCtx<AppMsg>,
    ) -> String {
        let mut task = Task::quick_capture(
            format!(
                "pending-{}",
                NEXT_PENDING_TASK_ID.fetch_add(1, Ordering::Relaxed)
            ),
            draft.title,
            String::new(),
            TaskSize::Small,
        );
        task.workspace_id = self
            .context
            .store
            .borrow()
            .state()
            .app_setting_values
            .get(DEFAULT_WORKSPACE_SETTING)
            .filter(|value| !value.is_empty())
            .cloned();
        if let Some(snoozed_until) = snoozed_until {
            task.state = TaskState::Snoozed;
            task.snoozed_until = Some(snoozed_until);
            self.calendar_create_context
                .select_created_task(task.id.clone());
        }
        let task_title = task.title.clone();
        task.rank = self
            .context
            .store
            .borrow()
            .state()
            .tasks
            .iter()
            .map(|task| task.rank)
            .min()
            .unwrap_or(0)
            .saturating_sub(1);
        self.context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskCreated(task.clone()));
        let remembered_custom_patch = match (snoozed_until, remember_custom) {
            (Some(until), Some(custom)) => Some(TaskPatch::Snooze {
                until,
                remember_custom: Some(custom),
            }),
            _ => None,
        };
        if let Some(patch) = remembered_custom_patch.clone() {
            self.context
                .store
                .borrow_mut()
                .dispatch(AppEvent::PatchTask {
                    task_id: task.id.clone(),
                    patch,
                });
        }
        let task_id = task.id.clone();
        let created_task_id = task_id.clone();
        self.context
            .coordinator
            .borrow_mut()
            .submit(PersistenceCommand::CreateTask(task));
        if let Some(patch) = remembered_custom_patch {
            self.context
                .coordinator
                .borrow_mut()
                .submit(PersistenceCommand::PatchTask(task_id.clone(), patch));
        }
        let notification = if let Some(until) = snoozed_until {
            format!(
                "“{task_title}” was scheduled for {}.",
                format_datetime(until)
            )
        } else {
            format!("“{task_title}” was added to backlog.")
        };
        ctx.notify(tuicore::Notification::success("Task created", notification));
        created_task_id
    }

    fn finish_task_creation(
        &mut self,
        task_id: String,
        created_from_calendar: bool,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.task_creation_active = false;
        self.task_creation_return_focus = None;
        self.close_dialog(ctx);
        if created_from_calendar {
            self.active_tab.set(CALENDAR_TAB_INDEX);
            ctx.focus(initial_calendar_focus_request());
        } else {
            *self.pending_task_navigation.borrow_mut() = Some(TaskNavigation {
                target_task_id: task_id,
                source_task_id: None,
                view: TaskView::Backlog,
            });
            self.active_tab.set(TASKS_TAB_INDEX);
            focus_task_table(ctx);
        }
        ctx.request_layout();
        ctx.request_redraw();
    }

    fn open_delete_task_dialog(
        &mut self,
        task_id: &str,
        return_focus: Option<TreePath>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.delete_return_focus = None;
        let Some(task) = self.task(task_id) else {
            let calendar_origin = self.take_calendar_bulk_origin();
            self.close_dialog(ctx);
            if calendar_origin {
                self.focus_calendar_bulk_result(ctx);
            } else {
                focus_return_path(return_focus, ctx);
            }
            return;
        };
        let primary = self.primary_dialog();
        primary.replace_layer(delete_task_dialog(&task), ctx);
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
        self.delete_return_focus = return_focus;
    }

    fn open_task_quick_menu(&mut self, task_id: &str, ctx: &mut EventCtx<AppMsg>) {
        let store = self.context.store.borrow();
        let state = store.state();
        let Some(task) = state.tasks.iter().find(|task| task.id == task_id) else {
            return;
        };
        let ordered_task_ids = ordered_task_ids(state);
        let (can_move_to_top, can_move_to_bottom) =
            task_edge_availability(&ordered_task_ids, task_id);
        let menu = TaskQuickMenu::new(
            task_id.to_string(),
            task.state,
            TaskQuickClipboard {
                execute: task_agent_command(state, task_id),
                clarify: task_agent_clarify_command(state, task_id),
                reference: task_reference(state, task_id),
            },
            can_move_to_top,
            can_move_to_bottom,
        );
        drop(store);
        let primary = self.primary_dialog();
        primary.replace_layer(AppDialog::TaskQuickMenu(Box::new(menu)), ctx);
        primary.set_layer_percent(40);
        primary.set_layer_cross_percent(35);
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
    }

    fn open_tasks_quick_menu(
        &mut self,
        task_ids: Vec<String>,
        selection_invocation: Option<PersistenceSelectionInvocation>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let state = self.context.store.borrow().state().clone();
        let tasks = tasks_for_ids(&state, &task_ids);
        if tasks.is_empty() {
            self.finish_transient_selection_without_persistence(selection_invocation);
            return;
        }
        let task_states = tasks.iter().map(|task| task.state).collect::<Vec<_>>();
        let ordered_task_ids = ordered_task_ids(&state);
        let (can_move_to_top, can_move_to_bottom) =
            task_block_edge_availability(&ordered_task_ids, &task_ids);
        let clipboard = TaskQuickClipboard {
            execute: task_agent_commands_for(&state, &tasks, "execute"),
            clarify: task_agent_commands_for(&state, &tasks, "clarify"),
            reference: Some(task_references_for(&state, &tasks)),
        };
        let mut menu = TaskQuickMenu::new_multiple(
            task_ids,
            task_states,
            clipboard,
            can_move_to_top,
            can_move_to_bottom,
        );
        menu.set_selection_invocation(selection_invocation);
        let primary = self.primary_dialog();
        primary.replace_layer(AppDialog::TaskQuickMenu(Box::new(menu)), ctx);
        primary.set_layer_percent(40);
        primary.set_layer_cross_percent(35);
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
    }

    fn open_calendar_task_quick_menu(
        &mut self,
        task_id: &str,
        time: PrimitiveDateTime,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.begin_calendar_bulk_action();
        let store = self.context.store.borrow();
        let state = store.state();
        let Some(task) = state.tasks.iter().find(|task| task.id == task_id) else {
            self.finish_transient_selection_without_persistence(None);
            return;
        };
        let ordered_at_time = task_ids_at_snooze_time(state, time);
        let (can_move_to_top, can_move_to_bottom) =
            task_edge_availability(&ordered_at_time, task_id);
        let menu = TaskQuickMenu::new_calendar_at_time(
            task_id.to_string(),
            task.state,
            TaskQuickClipboard {
                execute: task_agent_command(state, task_id),
                clarify: task_agent_clarify_command(state, task_id),
                reference: task_reference(state, task_id),
            },
            time,
            can_move_to_top,
            can_move_to_bottom,
        );
        drop(store);
        let primary = self.primary_dialog();
        primary.replace_layer(AppDialog::TaskQuickMenu(Box::new(menu)), ctx);
        primary.set_layer_percent(40);
        primary.set_layer_cross_percent(35);
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
    }

    fn open_calendar_tasks_quick_menu(
        &mut self,
        task_ids: Vec<String>,
        time: Option<PrimitiveDateTime>,
        selection_active: bool,
        selection_invocation: Option<PersistenceSelectionInvocation>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        if selection_active {
            self.begin_calendar_bulk_action();
        }
        let state = self.context.store.borrow().state().clone();
        let tasks = tasks_for_ids(&state, &task_ids);
        let Some(task) = tasks.first() else {
            self.finish_transient_selection_without_persistence(selection_invocation);
            self.finish_bulk_action(ctx);
            return;
        };
        if !selection_active
            && tasks.len() == 1
            && let Some(time) = time
        {
            self.open_calendar_task_quick_menu(&task.id, time, ctx);
            return;
        }
        let task_states = tasks.iter().map(|task| task.state).collect::<Vec<_>>();
        let clipboard = TaskQuickClipboard {
            execute: task_agent_commands_for(&state, &tasks, "execute"),
            clarify: task_agent_commands_for(&state, &tasks, "clarify"),
            reference: Some(task_references_for(&state, &tasks)),
        };
        let mut menu = if let Some(time) = time {
            let ordered_at_time = task_ids_at_snooze_time(&state, time);
            let (can_move_to_top, can_move_to_bottom) =
                task_block_edge_availability(&ordered_at_time, &task_ids);
            TaskQuickMenu::new_calendar_multiple_at_time(
                task_ids,
                task_states,
                clipboard,
                time,
                can_move_to_top,
                can_move_to_bottom,
            )
        } else {
            TaskQuickMenu::new_calendar_multiple(task_ids, task_states, clipboard, false, false)
        };
        menu.set_selection_invocation(selection_invocation);
        let primary = self.primary_dialog();
        primary.replace_layer(AppDialog::TaskQuickMenu(Box::new(menu)), ctx);
        primary.set_layer_percent(40);
        primary.set_layer_cross_percent(35);
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
    }

    fn copy_task_clipboard(
        &mut self,
        payload: String,
        selection_invocation: Option<PersistenceSelectionInvocation>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.context
            .accept_transient_selection_without_persistence(selection_invocation);
        ctx.copy_to_clipboard(payload);
        self.close_dialog(ctx);
        ctx.request_layout();
        ctx.request_redraw();
    }

    fn dispatch_selection_action(
        &mut self,
        selection_invocation: PersistenceSelectionInvocation,
        action: AppMsg,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        if !self
            .context
            .has_transient_selection_invocation(selection_invocation)
            || !selection_action_matches_invocation(&action, &self.context, selection_invocation)
        {
            return;
        }
        match action {
            AppMsg::CopyTaskClipboard(payload) => {
                self.copy_task_clipboard(payload, Some(selection_invocation), ctx)
            }
            AppMsg::OpenTasksQuickMenu(task_ids) => {
                self.open_tasks_quick_menu(task_ids, Some(selection_invocation), ctx)
            }
            AppMsg::OpenCalendarTasksQuickMenu {
                task_ids,
                time,
                selection_active,
            } => self.open_calendar_tasks_quick_menu(
                task_ids,
                time,
                selection_active,
                Some(selection_invocation),
                ctx,
            ),
            AppMsg::OpenDeleteTasks(task_ids) => {
                self.open_delete_tasks_dialog(&task_ids, Some(selection_invocation), ctx)
            }
            AppMsg::DeleteTasksConfirmed(task_ids) => {
                self.delete_tasks(task_ids, Some(selection_invocation), ctx)
            }
            AppMsg::OpenCalendarDeleteTasks(task_ids) => {
                self.begin_calendar_bulk_action();
                self.open_delete_tasks_dialog(&task_ids, Some(selection_invocation), ctx);
            }
            AppMsg::OpenTasksSnooze(task_ids) => {
                self.open_tasks_snooze_dialog(task_ids, Some(selection_invocation), ctx)
            }
            AppMsg::OpenCalendarTasksSnooze(task_ids) => {
                self.begin_calendar_bulk_action();
                self.open_tasks_snooze_dialog(task_ids, Some(selection_invocation), ctx)
            }
            AppMsg::OpenCompleteTasks(task_ids) => {
                self.open_complete_tasks_dialog(&task_ids, Some(selection_invocation), ctx)
            }
            AppMsg::OpenCalendarCompleteTasks(task_ids) => {
                self.begin_calendar_bulk_action();
                self.open_complete_tasks_dialog(&task_ids, Some(selection_invocation), ctx)
            }
            AppMsg::MoveTasksToTop(task_ids) => {
                self.move_tasks_to_edge(task_ids, true, Some(selection_invocation), ctx)
            }
            AppMsg::MoveTasksToBottom(task_ids) => {
                self.move_tasks_to_edge(task_ids, false, Some(selection_invocation), ctx)
            }
            AppMsg::MoveTasksToTopAtTime { task_ids, time } => self.move_calendar_tasks_to_edge(
                task_ids,
                time,
                true,
                Some(selection_invocation),
                ctx,
            ),
            AppMsg::MoveTasksToBottomAtTime { task_ids, time } => self.move_calendar_tasks_to_edge(
                task_ids,
                time,
                false,
                Some(selection_invocation),
                ctx,
            ),
            AppMsg::CompleteTasks { task_ids, state }
            | AppMsg::CompleteCalendarTasks { task_ids, state } => {
                self.complete_tasks(task_ids, state, Some(selection_invocation), ctx)
            }
            AppMsg::SnoozeTasks {
                task_ids,
                until,
                remember_custom,
            } => self.snooze_tasks(
                task_ids,
                until,
                remember_custom,
                Some(selection_invocation),
                ctx,
            ),
            AppMsg::UnsnoozeTasks(task_ids) => {
                self.unsnooze_tasks(task_ids, Some(selection_invocation), ctx)
            }
            AppMsg::CloseDialog => {
                self.context
                    .cancel_transient_selection(selection_invocation);
                self.close_dialog(ctx);
            }
            AppMsg::CloseDeleteTaskDialog => {
                self.context
                    .cancel_transient_selection(selection_invocation);
                self.close_delete_task_dialog(ctx);
            }
            AppMsg::CloseCompleteTaskDialog => {
                self.context
                    .cancel_transient_selection(selection_invocation);
                self.close_complete_task_dialog(ctx);
            }
            AppMsg::CloseSnoozeDialog => {
                self.context
                    .cancel_transient_selection(selection_invocation);
                self.close_snooze_dialog(ctx);
            }
            _ => {}
        }
    }

    fn finish_transient_selection_without_persistence(
        &self,
        selection_invocation: Option<PersistenceSelectionInvocation>,
    ) {
        self.context
            .retire_transient_selection_without_persistence(selection_invocation);
    }

    fn move_task_to_edge(&mut self, task_id: &str, to_top: bool, ctx: &mut EventCtx<AppMsg>) {
        let state = self.context.store.borrow().state().clone();
        let mut ordered = state
            .tasks
            .iter()
            .map(|task| task.id.clone())
            .collect::<Vec<_>>();
        let Some(index) = ordered.iter().position(|id| id == task_id) else {
            self.close_dialog(ctx);
            focus_task_table(ctx);
            return;
        };
        let task_title = state.tasks[index].title.clone();
        let task_id = ordered.remove(index);
        if to_top {
            ordered.insert(0, task_id);
        } else {
            ordered.push(task_id);
        }
        if persist_task_order(&self.context, &state, &ordered, None) {
            let edge = if to_top { "top" } else { "bottom" };
            ctx.notify(tuicore::Notification::success(
                "Task moved",
                format!("“{task_title}” moved to the {edge}."),
            ));
        }
        self.close_dialog(ctx);
        focus_task_table(ctx);
    }

    fn move_tasks_to_edge(
        &mut self,
        task_ids: Vec<String>,
        to_top: bool,
        selection_invocation: Option<PersistenceSelectionInvocation>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let calendar_origin = self.take_calendar_bulk_origin();
        let state = self.context.store.borrow().state().clone();
        let selected = tasks_for_ids(&state, &task_ids)
            .into_iter()
            .map(|task| task.id)
            .collect::<Vec<_>>();
        if selected.is_empty() {
            self.finish_transient_selection_without_persistence(selection_invocation);
            self.close_dialog(ctx);
            self.focus_bulk_result(calendar_origin, ctx);
            return;
        }
        let ordered = ordered_task_ids(&state);
        let mut remaining = ordered
            .iter()
            .filter(|id| !selected.contains(id))
            .cloned()
            .collect::<Vec<_>>();
        let reordered = if to_top {
            selected
                .iter()
                .cloned()
                .chain(remaining)
                .collect::<Vec<_>>()
        } else {
            remaining.extend(selected.iter().cloned());
            remaining
        };
        if persist_task_order(&self.context, &state, &reordered, selection_invocation) {
            let edge = if to_top { "top" } else { "bottom" };
            let (title, body) = if selected.len() == 1 {
                let task = state
                    .tasks
                    .iter()
                    .find(|task| task.id == selected[0])
                    .unwrap();
                (
                    "Task moved",
                    format!("“{}” moved to the {edge}.", task.title),
                )
            } else {
                (
                    "Tasks moved",
                    format!("{} tasks moved to the {edge}.", selected.len()),
                )
            };
            ctx.notify(tuicore::Notification::success(title, body));
        } else {
            self.finish_transient_selection_without_persistence(selection_invocation);
        }
        self.close_dialog(ctx);
        self.focus_bulk_result(calendar_origin, ctx);
    }

    fn move_calendar_task_to_edge(
        &mut self,
        task_id: &str,
        time: PrimitiveDateTime,
        to_top: bool,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let state = self.context.store.borrow().state().clone();
        let mut ordered = task_ids_at_snooze_time(&state, time);
        let Some(index) = ordered.iter().position(|id| id == task_id) else {
            self.close_dialog(ctx);
            self.focus_calendar_bulk_result(ctx);
            return;
        };
        let task_title = state
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .map(|task| task.title.clone());
        let task_id = ordered.remove(index);
        if to_top {
            ordered.insert(0, task_id);
        } else {
            ordered.push(task_id);
        }
        if persist_task_order(&self.context, &state, &ordered, None)
            && let Some(task_title) = task_title
        {
            let edge = if to_top { "top" } else { "bottom" };
            ctx.notify(tuicore::Notification::success(
                "Task moved",
                format!("“{task_title}” moved to the {edge} of tasks at the same time."),
            ));
        }
        self.close_dialog(ctx);
        self.focus_calendar_bulk_result(ctx);
    }

    fn move_calendar_tasks_to_edge(
        &mut self,
        task_ids: Vec<String>,
        time: PrimitiveDateTime,
        to_top: bool,
        selection_invocation: Option<PersistenceSelectionInvocation>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let state = self.context.store.borrow().state().clone();
        let ordered = task_ids_at_snooze_time(&state, time);
        if task_ids.iter().any(|task_id| !ordered.contains(task_id)) {
            self.finish_transient_selection_without_persistence(selection_invocation);
            self.close_dialog(ctx);
            self.focus_calendar_bulk_result(ctx);
            return;
        }
        let selected = ordered
            .iter()
            .filter(|task_id| task_ids.contains(task_id))
            .cloned()
            .collect::<Vec<_>>();
        if selected.is_empty() {
            self.finish_transient_selection_without_persistence(selection_invocation);
            self.close_dialog(ctx);
            self.focus_calendar_bulk_result(ctx);
            return;
        }
        let mut remaining = ordered
            .iter()
            .filter(|task_id| !selected.contains(task_id))
            .cloned()
            .collect::<Vec<_>>();
        let reordered = if to_top {
            selected
                .iter()
                .cloned()
                .chain(remaining)
                .collect::<Vec<_>>()
        } else {
            remaining.extend(selected.iter().cloned());
            remaining
        };
        if persist_task_order(&self.context, &state, &reordered, selection_invocation) {
            let edge = if to_top { "top" } else { "bottom" };
            ctx.notify(tuicore::Notification::success(
                "Tasks moved",
                format!(
                    "{} tasks moved to the {edge} of tasks at the same time.",
                    selected.len()
                ),
            ));
        } else {
            self.finish_transient_selection_without_persistence(selection_invocation);
        }
        self.close_dialog(ctx);
        self.focus_calendar_bulk_result(ctx);
    }

    fn delete_task(&mut self, task_id: String, ctx: &mut EventCtx<AppMsg>) {
        self.delete_return_focus = None;
        let calendar_origin = self.take_calendar_bulk_origin();
        let task = {
            let store = self.context.store.borrow();
            let state = store.state();
            state.tasks.iter().find(|task| task.id == task_id).cloned()
        };
        let Some(task) = task else {
            self.finish_transient_selection_without_persistence(None);
            self.close_dialog(ctx);
            if calendar_origin {
                self.focus_calendar_bulk_result(ctx);
            }
            return;
        };
        let task_title = task.title.clone();
        self.context
            .store
            .borrow_mut()
            .dispatch(AppEvent::TaskDeleted(task_id.clone()));
        self.context
            .coordinator
            .borrow_mut()
            .submit(PersistenceCommand::DeleteTask(task));
        ctx.notify(tuicore::Notification::success(
            "Task deleted",
            format!("“{task_title}” was deleted."),
        ));
        self.close_dialog(ctx);
        if calendar_origin {
            self.focus_calendar_bulk_result(ctx);
        }
    }

    fn open_delete_tasks_dialog(
        &mut self,
        task_ids: &[String],
        selection_invocation: Option<PersistenceSelectionInvocation>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.delete_return_focus = None;
        let tasks = tasks_for_ids(self.context.store.borrow().state(), task_ids);
        if tasks.is_empty() {
            self.finish_transient_selection_without_persistence(selection_invocation);
            let calendar_origin = self.take_calendar_bulk_origin();
            self.close_dialog(ctx);
            self.focus_bulk_result(calendar_origin, ctx);
            return;
        }
        let primary = self.primary_dialog();
        primary.replace_layer(delete_tasks_dialog(&tasks, selection_invocation), ctx);
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
    }

    fn delete_tasks(
        &mut self,
        task_ids: Vec<String>,
        selection_invocation: Option<PersistenceSelectionInvocation>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.delete_return_focus = None;
        let calendar_origin = self.take_calendar_bulk_origin();
        let tasks = tasks_for_ids(self.context.store.borrow().state(), &task_ids);
        if tasks.is_empty() {
            self.finish_transient_selection_without_persistence(selection_invocation);
            self.close_dialog(ctx);
            self.focus_bulk_result(calendar_origin, ctx);
            return;
        }
        for task in &tasks {
            self.context
                .store
                .borrow_mut()
                .dispatch(AppEvent::TaskDeleted(task.id.clone()));
        }
        let selection_invocation = self.context.accept_transient_mutation(selection_invocation);
        self.context
            .coordinator
            .borrow_mut()
            .submit(PersistenceCommand::BulkDeleteTasks {
                before: tasks.clone(),
                expected_revisions: Default::default(),
                selection_invocation,
            });
        ctx.notify(tuicore::Notification::success(
            "Tasks deleted",
            format!("{} tasks were deleted.", tasks.len()),
        ));
        self.close_dialog(ctx);
        self.focus_bulk_result(calendar_origin, ctx);
    }

    fn open_task_snooze_dialog(
        &mut self,
        task_id: &str,
        return_focus: Option<SnoozeReturnFocus>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.snooze_return_focus = None;
        let Some(task) = self.task(task_id) else {
            self.close_dialog(ctx);
            focus_snooze_return_focus(return_focus, ctx);
            return;
        };
        let now = match local_now() {
            Ok(now) => now,
            Err(error) => {
                ctx.notify(tuicore::Notification::error(
                    "Local time unavailable",
                    format!("Cannot open snooze options: {error}"),
                ));
                self.close_dialog(ctx);
                focus_snooze_return_focus(return_focus, ctx);
                return;
            }
        };
        let last_custom = self.context.store.borrow().state().last_custom_snooze;
        let default_time = self
            .context
            .store
            .borrow()
            .state()
            .app_setting_values
            .get(DEFAULT_SNOOZE_TIME_SETTING)
            .and_then(|value| parse_default_snooze_time(Some(value)).ok())
            .unwrap_or(default_snooze_time());
        let primary = self.primary_dialog();
        primary.replace_layer(
            AppDialog::Snooze(Box::new(SnoozeDialog::new_with_default_time(
                task.id,
                now,
                default_time,
                last_custom,
                task.state == TaskState::Snoozed,
            ))),
            ctx,
        );
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
        self.snooze_return_focus = return_focus;
    }

    fn open_tasks_snooze_dialog(
        &mut self,
        task_ids: Vec<String>,
        selection_invocation: Option<PersistenceSelectionInvocation>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.snooze_return_focus = None;
        let tasks = tasks_for_ids(self.context.store.borrow().state(), &task_ids);
        if tasks.is_empty() {
            self.finish_transient_selection_without_persistence(selection_invocation);
            let calendar_origin = self.take_calendar_bulk_origin();
            self.close_dialog(ctx);
            self.focus_bulk_result(calendar_origin, ctx);
            return;
        }
        let now = match local_now() {
            Ok(now) => now,
            Err(error) => {
                self.finish_transient_selection_without_persistence(selection_invocation);
                let calendar_origin = self.take_calendar_bulk_origin();
                ctx.notify(tuicore::Notification::error(
                    "Local time unavailable",
                    format!("Cannot open snooze options: {error}"),
                ));
                self.close_dialog(ctx);
                self.focus_bulk_result(calendar_origin, ctx);
                return;
            }
        };
        let state = self.context.store.borrow();
        let last_custom = state.state().last_custom_snooze;
        let default_time = state
            .state()
            .app_setting_values
            .get(DEFAULT_SNOOZE_TIME_SETTING)
            .and_then(|value| parse_default_snooze_time(Some(value)).ok())
            .unwrap_or(default_snooze_time());
        drop(state);
        let primary = self.primary_dialog();
        primary.replace_layer(
            AppDialog::Snooze(Box::new(
                SnoozeDialog::new_multiple_with_default_time(tasks, now, default_time, last_custom)
                    .with_selection_invocation(selection_invocation),
            )),
            ctx,
        );
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
    }

    fn open_complete_task_dialog(
        &mut self,
        task_id: &str,
        return_focus: Option<TreePath>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.complete_return_focus = None;
        self.complete_return_to_calendar = false;
        let Some(task) = self.task(task_id) else {
            self.close_dialog(ctx);
            focus_task_table(ctx);
            return;
        };
        let primary = self.primary_dialog();
        primary.replace_layer(complete_task_dialog(&task), ctx);
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
        let task_selected_on_open = self
            .context
            .store
            .borrow()
            .state()
            .selected_task_id
            .as_deref()
            == Some(task.id.as_str());
        self.complete_return_focus = return_focus.map(|path| CompleteReturnFocus {
            task_id: task.id,
            task_state: task.state,
            task_selected_on_open,
            path,
        });
    }

    fn open_complete_tasks_dialog(
        &mut self,
        task_ids: &[String],
        selection_invocation: Option<PersistenceSelectionInvocation>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.complete_return_focus = None;
        self.complete_return_to_calendar = false;
        let tasks = tasks_for_ids(self.context.store.borrow().state(), task_ids);
        if tasks.is_empty() {
            self.finish_transient_selection_without_persistence(selection_invocation);
            let calendar_origin = self.take_calendar_bulk_origin();
            self.close_dialog(ctx);
            self.focus_bulk_result(calendar_origin, ctx);
            return;
        }
        let primary = self.primary_dialog();
        primary.replace_layer(complete_tasks_dialog(&tasks, selection_invocation), ctx);
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
    }

    fn open_calendar_complete_task_dialog(
        &mut self,
        task_id: &str,
        return_focus: Option<TreePath>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        if self.task(task_id).is_none() {
            self.close_dialog(ctx);
            self.focus_calendar_bulk_result(ctx);
            return;
        }
        self.open_complete_task_dialog(task_id, return_focus, ctx);
        self.complete_return_to_calendar = true;
    }

    fn complete_task(&mut self, task_id: String, state: TaskState, ctx: &mut EventCtx<AppMsg>) {
        let return_to_calendar = std::mem::take(&mut self.complete_return_to_calendar);
        let task_title = self.task(&task_id).map(|task| task.title);
        let patch = TaskPatch::State(state);
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
                .submit(PersistenceCommand::PatchTask(task_id.clone(), patch));
            if let Some(task_title) = task_title {
                let (title, body) = match state {
                    TaskState::Done => ("Task completed", format!("“{task_title}” moved to done.")),
                    TaskState::Rejected => (
                        "Task rejected",
                        format!("“{task_title}” moved to rejected."),
                    ),
                    _ => (
                        "Task updated",
                        format!("“{task_title}” moved to {}.", state.id()),
                    ),
                };
                ctx.notify(tuicore::Notification::success(title, body));
            }
        } else {
            self.finish_transient_selection_without_persistence(None);
        }
        self.complete_return_focus = None;
        self.close_dialog(ctx);
        if return_to_calendar {
            ctx.focus(initial_calendar_focus_request());
            ctx.stop_propagation();
            ctx.request_redraw();
        } else {
            focus_task_table(ctx);
        }
    }

    fn complete_tasks(
        &mut self,
        task_ids: Vec<String>,
        state: TaskState,
        selection_invocation: Option<PersistenceSelectionInvocation>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let calendar_origin = self.take_calendar_bulk_origin();
        let tasks = tasks_for_ids(self.context.store.borrow().state(), &task_ids);
        let patch = TaskPatch::State(state);
        let mut changed_tasks = Vec::new();
        for task in tasks {
            if self
                .context
                .store
                .borrow_mut()
                .dispatch(AppEvent::PatchTask {
                    task_id: task.id.clone(),
                    patch: patch.clone(),
                })
                .changed
            {
                changed_tasks.push(task);
            }
        }
        if !changed_tasks.is_empty() {
            let selection_invocation = self.context.accept_transient_mutation(selection_invocation);
            self.context
                .coordinator
                .borrow_mut()
                .submit(PersistenceCommand::BulkPatchTasks {
                    before: changed_tasks.clone(),
                    patch,
                    expected_revisions: Default::default(),
                    selection_invocation,
                });
            let (title, body) = match state {
                TaskState::Done => (
                    "Tasks completed",
                    format!("{} tasks moved to done.", changed_tasks.len()),
                ),
                TaskState::Rejected => (
                    "Tasks rejected",
                    format!("{} tasks moved to rejected.", changed_tasks.len()),
                ),
                _ => (
                    "Tasks updated",
                    format!("{} tasks moved to {}.", changed_tasks.len(), state.id()),
                ),
            };
            ctx.notify(tuicore::Notification::success(title, body));
        } else {
            self.finish_transient_selection_without_persistence(selection_invocation);
        }
        self.complete_return_focus = None;
        self.complete_return_to_calendar = false;
        self.close_dialog(ctx);
        self.focus_bulk_result(calendar_origin, ctx);
    }

    fn toggle_task_progress(&mut self, task_id: String, ctx: &mut EventCtx<AppMsg>) {
        let calendar_origin = self.take_calendar_bulk_origin();
        let Some(task) = self.task(&task_id) else {
            self.focus_bulk_result(calendar_origin, ctx);
            return;
        };
        let was_backlog = task.state == TaskState::Backlog;
        let was_snoozed = task.state == TaskState::Snoozed;
        let state = toggled_task_progress_state(task.state);
        let patch = TaskPatch::State(state);
        let outcome = self
            .context
            .store
            .borrow_mut()
            .dispatch(AppEvent::PatchTask {
                task_id: task_id.clone(),
                patch: patch.clone(),
            });
        if !outcome.changed {
            self.finish_transient_selection_without_persistence(None);
            return;
        }
        self.context
            .coordinator
            .borrow_mut()
            .submit(PersistenceCommand::PatchTask(task_id.clone(), patch));
        if was_backlog || was_snoozed {
            *self.pending_task_view.borrow_mut() = Some(TaskView::Active);
        }
        if was_snoozed && !calendar_origin {
            self.context
                .store
                .borrow_mut()
                .dispatch(AppEvent::SelectTask(task_id));
            self.active_tab.set(TASKS_TAB_INDEX);
            focus_task_table(ctx);
        }
        let state_label = match state {
            TaskState::Todo => "todo",
            TaskState::InProgress => "in-progress",
            _ => unreachable!("task progress shortcut only targets active states"),
        };
        ctx.notify(tuicore::Notification::success(
            "Task moved",
            format!("“{}” moved to {state_label}.", task.title),
        ));
        if calendar_origin {
            self.focus_calendar_bulk_result(ctx);
        }
        ctx.request_layout();
    }

    fn snooze_task(
        &mut self,
        task_id: String,
        until: PrimitiveDateTime,
        remember_custom: Option<PrimitiveDateTime>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let task_title = self.task(&task_id).map(|task| task.title);
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
                .submit(PersistenceCommand::PatchTask(task_id.clone(), patch));
            if let Some(task_title) = task_title {
                ctx.notify(tuicore::Notification::success(
                    "Task snoozed",
                    format!("“{task_title}” snoozed until {}.", format_datetime(until)),
                ));
            }
        } else {
            self.finish_transient_selection_without_persistence(None);
        }
        let return_focus = if self.active_tab.get() == CALENDAR_TAB_INDEX {
            self.snooze_return_focus.take()
        } else {
            self.snooze_return_focus = None;
            None
        };
        self.close_dialog(ctx);
        if let Some(SnoozeReturnFocus::CalendarDay {
            path,
            date,
            has_other_tasks,
        }) = return_focus
        {
            if until.date() != date && !has_other_tasks {
                ctx.focus(app_tabs_focus_request());
            } else {
                ctx.focus(FocusRequest::Path(path));
            }
            ctx.stop_propagation();
            ctx.request_redraw();
        } else if let Some(SnoozeReturnFocus::Path(path)) = return_focus {
            ctx.focus(FocusRequest::Path(path));
            ctx.stop_propagation();
            ctx.request_redraw();
        } else if self.active_tab.get() == CALENDAR_TAB_INDEX {
            ctx.focus(initial_calendar_focus_request());
            ctx.stop_propagation();
            ctx.request_redraw();
        } else {
            focus_task_table(ctx);
        }
    }

    fn unsnooze_task(&mut self, task_id: String, ctx: &mut EventCtx<AppMsg>) {
        let task_title = self.task(&task_id).map(|task| task.title);
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
                .submit(PersistenceCommand::PatchTask(task_id.clone(), patch));
            if let Some(task_title) = task_title {
                ctx.notify(tuicore::Notification::success(
                    "Task unsnoozed",
                    format!("“{task_title}” moved to todo."),
                ));
            }
        } else {
            self.finish_transient_selection_without_persistence(None);
        }
        let return_focus = if self.active_tab.get() == CALENDAR_TAB_INDEX {
            self.snooze_return_focus.take()
        } else {
            self.snooze_return_focus = None;
            None
        };
        self.close_dialog(ctx);
        if let Some(return_focus) = return_focus {
            ctx.focus(FocusRequest::Path(return_focus.path().clone()));
            ctx.stop_propagation();
            ctx.request_redraw();
        } else if self.active_tab.get() == CALENDAR_TAB_INDEX {
            ctx.focus(initial_calendar_focus_request());
            ctx.stop_propagation();
            ctx.request_redraw();
        } else {
            focus_task_table(ctx);
        }
    }

    fn snooze_tasks(
        &mut self,
        task_ids: Vec<String>,
        until: PrimitiveDateTime,
        remember_custom: Option<PrimitiveDateTime>,
        selection_invocation: Option<PersistenceSelectionInvocation>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let calendar_origin = self.take_calendar_bulk_origin();
        let tasks = tasks_for_ids(self.context.store.borrow().state(), &task_ids);
        let patch = TaskPatch::Snooze {
            until,
            remember_custom,
        };
        let mut changed_tasks = Vec::new();
        for task in tasks {
            if self
                .context
                .store
                .borrow_mut()
                .dispatch(AppEvent::PatchTask {
                    task_id: task.id.clone(),
                    patch: patch.clone(),
                })
                .changed
            {
                changed_tasks.push(task);
            }
        }
        if !changed_tasks.is_empty() {
            let selection_invocation = self.context.accept_transient_mutation(selection_invocation);
            self.context
                .coordinator
                .borrow_mut()
                .submit(PersistenceCommand::BulkPatchTasks {
                    before: changed_tasks.clone(),
                    patch,
                    expected_revisions: Default::default(),
                    selection_invocation,
                });
            ctx.notify(tuicore::Notification::success(
                "Tasks snoozed",
                format!(
                    "{} tasks snoozed until {}.",
                    changed_tasks.len(),
                    format_datetime(until)
                ),
            ));
        } else {
            self.finish_transient_selection_without_persistence(selection_invocation);
        }
        self.snooze_return_focus = None;
        self.close_dialog(ctx);
        self.focus_bulk_result(calendar_origin, ctx);
    }

    fn unsnooze_tasks(
        &mut self,
        task_ids: Vec<String>,
        selection_invocation: Option<PersistenceSelectionInvocation>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let calendar_origin = self.take_calendar_bulk_origin();
        let tasks = tasks_for_ids(self.context.store.borrow().state(), &task_ids);
        if tasks.is_empty() || tasks.iter().any(|task| task.state != TaskState::Snoozed) {
            self.finish_transient_selection_without_persistence(selection_invocation);
            self.close_dialog(ctx);
            self.focus_bulk_result(calendar_origin, ctx);
            return;
        }
        let patch = TaskPatch::Unsnooze;
        for task in &tasks {
            self.context
                .store
                .borrow_mut()
                .dispatch(AppEvent::PatchTask {
                    task_id: task.id.clone(),
                    patch: patch.clone(),
                });
        }
        let selection_invocation = self.context.accept_transient_mutation(selection_invocation);
        self.context
            .coordinator
            .borrow_mut()
            .submit(PersistenceCommand::BulkPatchTasks {
                before: tasks.clone(),
                patch,
                expected_revisions: Default::default(),
                selection_invocation,
            });
        ctx.notify(tuicore::Notification::success(
            "Tasks unsnoozed",
            format!("{} tasks moved to todo.", tasks.len()),
        ));
        self.snooze_return_focus = None;
        self.close_dialog(ctx);
        self.focus_bulk_result(calendar_origin, ctx);
    }

    fn begin_calendar_bulk_action(&mut self) {
        self.calendar_bulk_origin = true;
    }

    fn take_calendar_bulk_origin(&mut self) -> bool {
        std::mem::take(&mut self.calendar_bulk_origin)
    }

    fn finish_bulk_action(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let calendar_origin = self.take_calendar_bulk_origin();
        self.focus_bulk_result(calendar_origin, ctx);
    }

    fn focus_bulk_result(&self, calendar_origin: bool, ctx: &mut EventCtx<AppMsg>) {
        if calendar_origin {
            self.focus_calendar_bulk_result(ctx);
        } else {
            focus_task_table(ctx);
        }
    }

    fn focus_calendar_bulk_result(&self, ctx: &mut EventCtx<AppMsg>) {
        let selected_date = self.calendar_create_context.selected_date();
        let has_calendar_entry = self
            .context
            .store
            .borrow()
            .state()
            .tasks
            .iter()
            .any(|task| {
                task.state == TaskState::Snoozed
                    && task
                        .snoozed_until
                        .is_some_and(|snoozed_until| snoozed_until.date() == selected_date)
            });
        if has_calendar_entry {
            ctx.focus(initial_calendar_focus_request());
        } else {
            ctx.focus(app_tabs_focus_request());
        }
        ctx.stop_propagation();
        ctx.request_redraw();
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
        let task_creation_return_focus = std::mem::take(&mut self.task_creation_return_focus);
        let returning_from_task_creation = std::mem::take(&mut self.task_creation_active);
        self.pending_calendar_task = None;
        self.create_task_calendar_date = None;
        self.calendar_bulk_origin = false;
        self.root.set_active_with_context(false, ctx);
        self.primary_dialog().set_active_with_context(false, ctx);
        if returning_from_task_creation {
            if let Some(path) = task_creation_return_focus {
                ctx.focus(FocusRequest::Path(path));
                ctx.stop_propagation();
            }
            ctx.request_layout();
            ctx.request_redraw();
        }
    }

    fn task_creation_origin(&self, route: &EventRoute) -> TreePath {
        self.active_focus_path
            .clone()
            .unwrap_or_else(|| route.path.clone())
    }

    fn close_snooze_dialog(&mut self, ctx: &mut EventCtx<AppMsg>) {
        if self.pending_calendar_task.is_some() {
            self.snooze_return_focus = None;
            self.close_dialog(ctx);
            if self.active_tab.get() == CALENDAR_TAB_INDEX {
                ctx.focus(initial_calendar_focus_request());
            }
            return;
        }
        let return_focus = self.snooze_return_focus.take();
        self.close_dialog(ctx);
        if let Some(return_focus) = return_focus {
            ctx.focus(FocusRequest::Path(return_focus.path().clone()));
            ctx.stop_propagation();
            ctx.request_redraw();
        } else if self.active_tab.get() == CALENDAR_TAB_INDEX {
            ctx.focus(initial_calendar_focus_request());
            ctx.stop_propagation();
            ctx.request_redraw();
        } else {
            focus_task_table(ctx);
        }
    }

    fn close_delete_task_dialog(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let return_focus = self.delete_return_focus.take();
        self.close_dialog(ctx);
        if let Some(path) = return_focus {
            ctx.focus(FocusRequest::Path(path));
            ctx.stop_propagation();
            ctx.request_redraw();
        } else if self.active_tab.get() == CALENDAR_TAB_INDEX {
            ctx.focus(initial_calendar_focus_request());
            ctx.stop_propagation();
            ctx.request_redraw();
        } else {
            focus_task_table(ctx);
        }
    }

    fn close_complete_task_dialog(&mut self, ctx: &mut EventCtx<AppMsg>) {
        self.complete_return_to_calendar = false;
        let return_focus = self.complete_return_focus.take();
        self.close_dialog(ctx);
        let valid_return_path = return_focus.and_then(|origin| {
            let store = self.context.store.borrow();
            let state = store.state();
            let selection_is_valid = !origin.task_selected_on_open
                || state.selected_task_id.as_deref() == Some(&origin.task_id);
            state
                .tasks
                .iter()
                .any(|task| task.id == origin.task_id && task.state == origin.task_state)
                .then_some(origin.path)
                .filter(|_| selection_is_valid)
        });
        if let Some(path) = valid_return_path {
            ctx.focus(FocusRequest::Path(path));
            ctx.stop_propagation();
            ctx.request_redraw();
        } else if self.active_tab.get() == CALENDAR_TAB_INDEX {
            ctx.focus(initial_calendar_focus_request());
            ctx.stop_propagation();
            ctx.request_redraw();
        } else {
            focus_task_table(ctx);
        }
    }

    fn close_management_overlay(&mut self, ctx: &mut EventCtx<AppMsg>) {
        self.root.set_active_with_context(false, ctx);
    }
}

fn speed_reader_settings(state: &AppState) -> SpeedReaderSettings {
    let defaults = SpeedReaderSettings::default();
    SpeedReaderSettings {
        wpm: state
            .app_setting_values
            .get(SPEED_READER_WPM_SETTING)
            .and_then(|value| parse_speed_reader_wpm(Some(value)).ok())
            .unwrap_or(defaults.wpm),
        markdown_block_pause: state
            .app_setting_values
            .get(SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING)
            .and_then(|value| parse_markdown_block_pause(Some(value)).ok())
            .unwrap_or(defaults.markdown_block_pause),
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
        if route
            .path
            .keys()
            .last()
            .is_some_and(|key| matches!(key.as_str(), "new-task" | "new-note"))
            && detail_escape(event)
        {
            ctx.focus(app_tabs_focus_request());
            ctx.stop_propagation();
            ctx.request_redraw();
            return EventOutcome::Handled;
        }
        let outcome = self.root.dispatch_event(route, event, ctx);
        if ctx
            .messages()
            .iter()
            .any(|message| matches!(message, AppMsg::OpenCreateTask { .. }))
        {
            self.task_creation_return_focus = Some(self.task_creation_origin(route));
            return outcome;
        }
        if outcome.handled() {
            return outcome;
        }
        let task_create_hotkey = keys::TASK_QUICK_CREATE.matches(event)
            || matches!(
                event,
                TuiEvent::Hotkey(HotkeyEvent::Commit(sequence))
                    if sequence == &keys::TASK_QUICK_CREATE.hotkey()
            );
        if task_create_hotkey {
            self.task_creation_return_focus = Some(self.task_creation_origin(route));
            ctx.emit(AppMsg::OpenCreateTask {
                calendar_date: (self.active_tab.get() == CALENDAR_TAB_INDEX)
                    .then(|| self.calendar_create_context.selected_date()),
            });
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        if focused {
            self.active_focus_path = Some(target.path.clone());
        }
        self.root.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let mut result = self.root.tick(dt, settings);
        let persistence_changed = self.context.coordinator.borrow_mut().poll();
        let navigation_changed = self.route_external_navigation();
        if persistence_changed || navigation_changed {
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

    fn take_pending_focus_request(&mut self) -> Option<FocusRequest> {
        self.pending_focus_request.take()
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
type TaskTable = ListControl<TaskRow, String, AppMsg>;
type TaskDetail = TaskDetailForm;
const TASK_SEARCH_WIDTH: u16 = 28;

struct TaskMaster {
    toolbar: Flex<AppMsg>,
    table: TaskTable,
    toolbar_area: Rect,
    table_area: Rect,
}

impl TaskMaster {
    fn new(toolbar: Flex<AppMsg>, table: TaskTable) -> Self {
        Self {
            toolbar,
            table,
            toolbar_area: Rect::default(),
            table_area: Rect::default(),
        }
    }

    fn table(&self) -> &TaskTable {
        &self.table
    }

    fn table_mut(&mut self) -> &mut TaskTable {
        &mut self.table
    }

    #[cfg(test)]
    fn child_areas(&self) -> (Rect, Rect) {
        (self.toolbar_area, self.table_area)
    }
}

impl TuiNode<AppMsg> for TaskMaster {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.table.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        let toolbar_x = area
            .x
            .saturating_add(area.width.min(TASK_SEARCH_WIDTH))
            .saturating_add(1)
            .min(area.right());
        self.toolbar_area = Rect::new(
            toolbar_x,
            area.y,
            area.right().saturating_sub(toolbar_x),
            area.height.min(1),
        );
        self.table_area = area;
        ctx.push_slot(ChildKey::first(), self.toolbar_area, |ctx| {
            self.toolbar.layout(self.toolbar_area, ctx);
        });
        ctx.push_slot(ChildKey::second(), self.table_area, |ctx| {
            ctx.with_focus_fallback_hotkey_sequences_status(
                FocusId::new("task-table"),
                self.table_area,
                vec![keys::TASK_AGENT_YANK_CLARIFY.hotkey()],
                |ctx| self.table.layout(self.table_area, ctx),
            );
        });
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, _area: Rect, ctx: &mut RenderCtx<'a>) {
        self.table.render(frame, self.table_area, ctx);
        self.toolbar.render(frame, self.toolbar_area, ctx);
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
        if let Some(route) = route
            .path
            .without_first_if(&ChildKey::first())
            .map(EventRoute::new)
        {
            return self
                .toolbar
                .dispatch_event(&route, event, ctx)
                .bubble(ctx, |ctx| self.event(event, ctx));
        }
        if let Some(route) = route
            .path
            .without_first_if(&ChildKey::second())
            .map(EventRoute::new)
        {
            return self
                .table
                .dispatch_event(&route, event, ctx)
                .bubble(ctx, |ctx| self.event(event, ctx));
        }
        EventOutcome::Ignored
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        if let Some(target) = target.for_child(&ChildKey::first()) {
            self.toolbar.dispatch_focus(&target, focused, ctx);
        } else if let Some(target) = target.for_child(&ChildKey::second()) {
            self.table.dispatch_focus(&target, focused, ctx);
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.toolbar
            .tick(dt, settings)
            .merge(self.table.tick(dt, settings))
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.toolbar.init(ctx);
        self.table.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.toolbar.mount(ctx);
        self.table.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.table.unmount(ctx);
        self.toolbar.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.table.destroy(ctx);
        self.toolbar.destroy(ctx);
    }
}

type TaskWorkspaceLayout = ResponsiveSplit<TaskMaster, TaskDetail>;
type TaskViewChange = Rc<RefCell<Option<TaskView>>>;
type ActiveTaskView = Rc<RefCell<TaskView>>;
type PendingTaskNavigation = Rc<RefCell<Option<TaskNavigation>>>;
pub(crate) type ActiveWorkspaceFilter = Rc<RefCell<Option<String>>>;
pub(crate) type ActiveLabelFilter = Rc<RefCell<Vec<String>>>;
type VisibleTaskSelection = Rc<RefCell<Option<String>>>;

pub(crate) fn task_ids_at_snooze_time(state: &AppState, time: PrimitiveDateTime) -> Vec<String> {
    let mut tasks = state
        .tasks
        .iter()
        .filter(|task| task.state == TaskState::Snoozed && task.snoozed_until == Some(time))
        .collect::<Vec<_>>();
    tasks.sort_by_key(|task| task.rank);
    tasks.into_iter().map(|task| task.id.clone()).collect()
}

fn task_edge_availability(ordered_task_ids: &[String], task_id: &str) -> (bool, bool) {
    let Some(index) = ordered_task_ids.iter().position(|id| id == task_id) else {
        return (false, false);
    };
    (index > 0, index + 1 < ordered_task_ids.len())
}

fn task_block_edge_availability(ordered_task_ids: &[String], task_ids: &[String]) -> (bool, bool) {
    if task_ids.is_empty()
        || task_ids
            .iter()
            .any(|task_id| !ordered_task_ids.contains(task_id))
    {
        return (false, false);
    }
    (
        !task_ids.contains(&ordered_task_ids[0]),
        !task_ids.contains(&ordered_task_ids[ordered_task_ids.len() - 1]),
    )
}

fn ordered_task_ids(state: &AppState) -> Vec<String> {
    let mut tasks = state.tasks.iter().collect::<Vec<_>>();
    tasks.sort_by_key(|task| task.rank);
    tasks.into_iter().map(|task| task.id.clone()).collect()
}

pub(crate) fn tasks_for_ids(state: &AppState, task_ids: &[String]) -> Vec<Task> {
    let mut tasks = state
        .tasks
        .iter()
        .filter(|task| task_ids.contains(&task.id))
        .cloned()
        .collect::<Vec<_>>();
    tasks.sort_by_key(|task| task.rank);
    tasks
}

pub(crate) fn persist_task_order(
    context: &AppContext,
    state: &AppState,
    ordered_ids: &[String],
    selection_invocation: Option<PersistenceSelectionInvocation>,
) -> bool {
    let mut ranks = ordered_ids
        .iter()
        .filter_map(|id| state.tasks.iter().find(|task| task.id == *id))
        .map(|task| task.rank)
        .collect::<Vec<_>>();
    ranks.sort_unstable();
    if ranks.len() != ordered_ids.len() {
        return false;
    }

    let before = ordered_ids
        .iter()
        .filter_map(|id| {
            state
                .tasks
                .iter()
                .find(|task| task.id == *id)
                .map(|task| TaskRank {
                    id: id.clone(),
                    rank: task.rank,
                })
        })
        .collect::<Vec<_>>();
    let after = ordered_ids
        .iter()
        .cloned()
        .zip(ranks)
        .map(|(id, rank)| TaskRank { id, rank })
        .filter(|rank| {
            before
                .iter()
                .find(|previous| previous.id == rank.id)
                .is_some_and(|previous| previous.rank != rank.rank)
        })
        .collect::<Vec<_>>();
    if after.is_empty() {
        return false;
    }
    let before = before
        .into_iter()
        .filter(|rank| after.iter().any(|changed| changed.id == rank.id))
        .collect::<Vec<_>>();
    context
        .store
        .borrow_mut()
        .dispatch(AppEvent::TaskRanksChanged(after.clone()));
    let selection_invocation = context.accept_transient_mutation(selection_invocation);
    context
        .coordinator
        .borrow_mut()
        .submit(PersistenceCommand::ReorderTasks {
            before,
            after,
            expected_revisions: std::collections::HashMap::new(),
            selection_invocation,
        });
    true
}

fn initial_task_table_focus_request() -> FocusRequest {
    FocusRequest::TargetAt {
        path: TreePath::from_keys([
            ChildKey::first(),
            ChildKey::first(),
            ChildKey::new("content"),
            ChildKey::new("tabs"),
            ChildKey::new("tab-0"),
            ChildKey::first(),
            ChildKey::second(),
            ChildKey::new("data"),
        ]),
        id: FocusId::new("data-view"),
    }
}

fn issue_links_focus_request() -> FocusRequest {
    FocusRequest::TargetAt {
        path: TreePath::from_keys([
            ChildKey::first(),
            ChildKey::first(),
            ChildKey::new("content"),
            ChildKey::new("tabs"),
            ChildKey::new("tab-0"),
            ChildKey::second(),
            ChildKey::body(),
            ChildKey::new("form"),
            ChildKey::new("relations"),
            ChildKey::new("data"),
        ]),
        id: FocusId::new("data-view"),
    }
}

fn app_tabs_focus_request() -> FocusRequest {
    FocusRequest::TargetAt {
        path: TreePath::from_keys([
            ChildKey::first(),
            ChildKey::first(),
            ChildKey::new("content"),
            ChildKey::new("tabs"),
        ]),
        id: FocusId::new("tabs"),
    }
}

fn notes_first_child_focus_request() -> FocusRequest {
    let FocusRequest::TargetAt { path, id } = app_tabs_focus_request() else {
        unreachable!("app tabs focus request must target the tabs control");
    };
    FocusRequest::FirstChildOf { path, id }
}

fn notes_workspace_focus_path() -> TreePath {
    let FocusRequest::TargetAt { path, .. } = app_tabs_focus_request() else {
        unreachable!("app tabs focus request must target the tabs control");
    };
    path.child(ChildKey::new("tab-2"))
}

fn initial_calendar_focus_request() -> FocusRequest {
    FocusRequest::TargetAt {
        path: TreePath::from_keys([
            ChildKey::first(),
            ChildKey::first(),
            ChildKey::new("content"),
            ChildKey::new("tabs"),
            ChildKey::new("tab-1"),
            ChildKey::first(),
        ]),
        id: FocusId::new("calendar"),
    }
}

fn focus_return_path(return_focus: Option<TreePath>, ctx: &mut EventCtx<AppMsg>) {
    if let Some(path) = return_focus {
        ctx.focus(FocusRequest::Path(path));
        ctx.stop_propagation();
        ctx.request_redraw();
    } else {
        focus_task_table(ctx);
    }
}

fn focus_snooze_return_focus(return_focus: Option<SnoozeReturnFocus>, ctx: &mut EventCtx<AppMsg>) {
    focus_return_path(return_focus.map(|focus| focus.path().clone()), ctx);
}

pub(crate) fn selection_action(
    invocation: Option<PersistenceSelectionInvocation>,
    action: AppMsg,
) -> AppMsg {
    if let Some(invocation) = invocation {
        AppMsg::SelectionAction {
            invocation,
            action: Box::new(action),
        }
    } else {
        action
    }
}

fn selection_action_matches_invocation(
    action: &AppMsg,
    context: &AppContext,
    invocation: PersistenceSelectionInvocation,
) -> bool {
    let task_ids = match action {
        AppMsg::OpenTasksQuickMenu(task_ids)
        | AppMsg::OpenDeleteTasks(task_ids)
        | AppMsg::DeleteTasksConfirmed(task_ids)
        | AppMsg::OpenCalendarDeleteTasks(task_ids)
        | AppMsg::OpenCalendarTasksSnooze(task_ids)
        | AppMsg::OpenCalendarCompleteTasks(task_ids)
        | AppMsg::MoveTasksToTop(task_ids)
        | AppMsg::MoveTasksToBottom(task_ids)
        | AppMsg::OpenTasksSnooze(task_ids)
        | AppMsg::UnsnoozeTasks(task_ids) => Some(task_ids.as_slice()),
        AppMsg::OpenCalendarTasksQuickMenu { task_ids, .. }
        | AppMsg::MoveTasksToTopAtTime { task_ids, .. }
        | AppMsg::MoveTasksToBottomAtTime { task_ids, .. }
        | AppMsg::CompleteTasks { task_ids, .. }
        | AppMsg::CompleteCalendarTasks { task_ids, .. }
        | AppMsg::SnoozeTasks { task_ids, .. } => Some(task_ids.as_slice()),
        AppMsg::CopyTaskClipboard(_)
        | AppMsg::CloseDialog
        | AppMsg::CloseDeleteTaskDialog
        | AppMsg::CloseCompleteTaskDialog
        | AppMsg::CloseSnoozeDialog => None,
        _ => return false,
    };
    task_ids.is_none_or(|task_ids| context.selection_matches(invocation, task_ids))
}

#[derive(Clone)]
pub(crate) struct AppContext {
    pub(crate) store: AppStore,
    pub(crate) coordinator: Rc<RefCell<PersistenceCoordinator>>,
    selection_invocations: Rc<RefCell<Vec<SelectionInvocation>>>,
    next_selection_token: Rc<Cell<u64>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransientSelectionSource {
    TaskList,
    Calendar,
}

#[derive(Clone, Debug)]
struct SelectionInvocation {
    invocation: PersistenceSelectionInvocation,
    selected_task_ids: BTreeSet<String>,
    start_highlight: Option<String>,
    clear_requested: bool,
    clear_consumed: bool,
    persistence_invocation: Option<PersistenceSelectionInvocation>,
    rollback_pending: bool,
}

pub(crate) fn persistence_selection_invocation(
    source: TransientSelectionSource,
    token: u64,
) -> PersistenceSelectionInvocation {
    PersistenceSelectionInvocation {
        token,
        source: match source {
            TransientSelectionSource::TaskList => PersistenceSelectionSource::TaskList,
            TransientSelectionSource::Calendar => PersistenceSelectionSource::Calendar,
        },
    }
}

impl AppContext {
    fn new(store: AppStore, coordinator: Rc<RefCell<PersistenceCoordinator>>) -> Self {
        Self {
            store,
            coordinator,
            selection_invocations: Rc::new(RefCell::new(Vec::new())),
            next_selection_token: Rc::new(Cell::new(1)),
        }
    }

    pub(crate) fn begin_transient_selection(
        &self,
        source: TransientSelectionSource,
        task_ids: Vec<String>,
        highlighted_task_id: Option<String>,
    ) -> Option<PersistenceSelectionInvocation> {
        if task_ids.is_empty() {
            return None;
        }
        let token = self.next_selection_token.get();
        self.next_selection_token.set(token.wrapping_add(1));
        let invocation = persistence_selection_invocation(source, token);
        self.selection_invocations
            .borrow_mut()
            .push(SelectionInvocation {
                invocation,
                selected_task_ids: task_ids.into_iter().collect(),
                start_highlight: highlighted_task_id,
                clear_requested: false,
                clear_consumed: false,
                persistence_invocation: None,
                rollback_pending: false,
            });
        Some(invocation)
    }

    pub(crate) fn accept_transient_clipboard(
        &self,
        invocation: Option<PersistenceSelectionInvocation>,
    ) {
        self.resolve_transient_selection(invocation, false);
    }

    fn accept_transient_mutation(
        &self,
        invocation: Option<PersistenceSelectionInvocation>,
    ) -> Option<PersistenceSelectionInvocation> {
        self.resolve_transient_selection(invocation, true)
    }

    fn resolve_transient_selection(
        &self,
        persistence_invocation: Option<PersistenceSelectionInvocation>,
        persistence_guard: bool,
    ) -> Option<PersistenceSelectionInvocation> {
        let persistence_invocation = persistence_invocation?;
        let mut invocations = self.selection_invocations.borrow_mut();
        let Some(invocation) = invocations.iter_mut().find(|invocation| {
            !invocation.clear_requested && invocation.invocation == persistence_invocation
        }) else {
            return None;
        };
        invocation.clear_requested = true;
        if persistence_guard {
            invocation.persistence_invocation = Some(persistence_invocation);
            return Some(persistence_invocation);
        }
        None
    }

    pub(crate) fn take_selection_clear_request(
        &self,
        invocation: PersistenceSelectionInvocation,
    ) -> bool {
        let mut invocations = self.selection_invocations.borrow_mut();
        let Some(index) = invocations.iter().position(|current| {
            current.invocation == invocation && current.clear_requested && !current.clear_consumed
        }) else {
            return false;
        };
        invocations[index].clear_consumed = true;
        if invocations[index].persistence_invocation.is_none() {
            invocations.remove(index);
        }
        true
    }

    fn retire_transient_selection_without_persistence(
        &self,
        invocation: Option<PersistenceSelectionInvocation>,
    ) {
        self.resolve_transient_selection(invocation, false);
    }

    fn accept_transient_selection_without_persistence(
        &self,
        invocation: Option<PersistenceSelectionInvocation>,
    ) {
        self.resolve_transient_selection(invocation, false);
    }

    fn cancel_transient_selection(&self, invocation: PersistenceSelectionInvocation) {
        self.selection_invocations
            .borrow_mut()
            .retain(|current| current.clear_requested || current.invocation != invocation);
    }

    pub(crate) fn rollback_transient_selection_highlight(
        &self,
        invocation: PersistenceSelectionInvocation,
    ) -> Option<String> {
        let mut invocations = self.selection_invocations.borrow_mut();
        invocations
            .iter()
            .rposition(|current| current.invocation == invocation && current.rollback_pending)
            .and_then(|index| invocations.remove(index).start_highlight)
    }

    pub(crate) fn resolve_persistence_selection_outcomes(&self) {
        for outcome in self.coordinator.borrow_mut().take_selection_outcomes() {
            let mut invocations = self.selection_invocations.borrow_mut();
            let Some(index) = invocations
                .iter()
                .position(|current| current.persistence_invocation == Some(outcome.invocation))
            else {
                continue;
            };
            if outcome.failed {
                invocations[index].rollback_pending = true;
            } else {
                invocations.remove(index);
            }
        }
    }

    fn has_transient_selection_invocation(
        &self,
        persistence_invocation: PersistenceSelectionInvocation,
    ) -> bool {
        self.selection_invocations
            .borrow()
            .iter()
            .any(|invocation| {
                !invocation.clear_requested && invocation.invocation == persistence_invocation
            })
    }

    fn selection_matches(
        &self,
        persistence_invocation: PersistenceSelectionInvocation,
        task_ids: &[String],
    ) -> bool {
        self.selection_invocations
            .borrow()
            .iter()
            .any(|invocation| {
                !invocation.clear_requested
                    && invocation.invocation == persistence_invocation
                    && invocation.selected_task_ids == task_ids.iter().cloned().collect()
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TaskView {
    Active,
    Backlog,
    Snoozed,
    Archived,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskNavigation {
    target_task_id: String,
    source_task_id: Option<String>,
    view: TaskView,
}

enum ExternalNavigation {
    CreatedBacklogTask(String),
    StartedTask(String),
    Note(String),
}

impl TaskView {
    const OPTIONS: [Self; 5] = [
        Self::All,
        Self::Backlog,
        Self::Active,
        Self::Snoozed,
        Self::Archived,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Backlog => "Backlog",
            Self::Snoozed => "Snoozed",
            Self::Archived => "Archived",
            Self::All => "All",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::All => "",
            Self::Backlog => "",
            Self::Active => "",
            Self::Snoozed => "󰒲",
            Self::Archived => "",
        }
    }

    fn menu_label(self) -> String {
        format!("{} {}", self.icon(), self.label())
    }

    fn contains(self, task: &Task) -> bool {
        match self {
            Self::Active => matches!(task.state, TaskState::Todo | TaskState::InProgress),
            Self::Backlog => task.state == TaskState::Backlog,
            Self::Snoozed => task.state == TaskState::Snoozed,
            Self::Archived => matches!(task.state, TaskState::Done | TaskState::Rejected),
            Self::All => !matches!(task.state, TaskState::Done | TaskState::Rejected),
        }
    }

    fn empty_message(self) -> &'static str {
        match self {
            Self::Active => "No active tasks",
            Self::Backlog => "No tasks in backlog",
            Self::Snoozed => "No snoozed tasks",
            Self::Archived => "No archived tasks",
            Self::All => "No open tasks",
        }
    }
}

fn task_empty_state(tasks: &[Task], task_view: TaskView) -> SeasonalEmptyState {
    let message = if tasks.iter().any(|task| task_view.contains(task)) {
        "No tasks match your filters"
    } else {
        task_view.empty_message()
    };
    SeasonalEmptyState::new(message)
}

struct TaskViewMenu {
    menu_button: MenuButton<TaskView, AppMsg>,
    pending_view: TaskViewChange,
    active_view: ActiveTaskView,
}

impl TaskViewMenu {
    fn new(pending_view: TaskViewChange, active_view: ActiveTaskView) -> Self {
        let selected = *active_view.borrow();
        let hotkey = keys::TASK_VIEW_MENU.hotkey();
        let menu_button = MenuButton::new(
            selected.menu_label(),
            TaskView::OPTIONS.map(|view| MenuItem::new(view, view.menu_label())),
        )
        .visible_items(TaskView::OPTIONS.len() as u16)
        .min_popup_width(20)
        .hotkey(hotkey)
        .hotkey_label_mode(HotkeyLabelMode::Inline);
        Self {
            menu_button,
            pending_view,
            active_view,
        }
    }

    fn sync_activated(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let Some(view) = self.menu_button.take_activated().into_iter().last() else {
            return;
        };
        self.menu_button.set_label(view.menu_label());
        *self.active_view.borrow_mut() = view;
        *self.pending_view.borrow_mut() = Some(view);
        ctx.request_layout();
        ctx.request_redraw();
    }

    fn finish_event(
        &mut self,
        was_open: bool,
        event: &TuiEvent,
        outcome: EventOutcome,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        self.sync_activated(ctx);
        if was_open && !self.menu_button.is_open() && detail_escape(event) {
            focus_task_table(ctx);
            return EventOutcome::Handled;
        }
        outcome
    }
}

impl TuiNode<AppMsg> for TaskViewMenu {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.menu_button.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.menu_button
            .set_label(self.active_view.borrow().menu_label());
        self.menu_button.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.menu_button.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        let was_open = self.menu_button.is_open();
        let outcome = self.menu_button.event(event, ctx);
        self.finish_event(was_open, event, outcome, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let was_open = self.menu_button.is_open();
        let outcome = self.menu_button.dispatch_event(route, event, ctx);
        self.finish_event(was_open, event, outcome, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        self.menu_button.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.menu_button.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.menu_button.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.menu_button.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.menu_button.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.menu_button.destroy(ctx);
    }
}

struct TaskFilterControls {
    context: AppContext,
    controls: Flex<AppMsg>,
    active_workspace_filter: ActiveWorkspaceFilter,
    active_label_filter: ActiveLabelFilter,
    active_tab: Rc<Cell<usize>>,
    filter_submitted: Rc<Cell<bool>>,
    known_workspaces: Vec<(String, String)>,
    known_tags: Vec<(String, String)>,
}

impl TaskFilterControls {
    fn new(
        context: AppContext,
        active_workspace_filter: ActiveWorkspaceFilter,
        active_label_filter: ActiveLabelFilter,
        active_tab: Rc<Cell<usize>>,
    ) -> Self {
        let state = context.store.borrow();
        let workspaces = state.state().workspaces.clone();
        let tags = state.state().tags.clone();
        drop(state);
        let filter_submitted = Rc::new(Cell::new(false));
        let controls = Flex::row()
            .align(CrossAlign::Center)
            .gap(1)
            .child(
                "workspace",
                workspace_filter_dropdown(
                    &workspaces,
                    Rc::clone(&active_workspace_filter),
                    Rc::clone(&filter_submitted),
                ),
                FlexItem::content(),
            )
            .child(
                "labels",
                label_filter_dropdown(
                    &tags,
                    Rc::clone(&active_label_filter),
                    Rc::clone(&filter_submitted),
                ),
                FlexItem::content(),
            );
        Self {
            context,
            controls,
            active_workspace_filter,
            active_label_filter,
            active_tab,
            filter_submitted,
            known_workspaces: workspaces
                .iter()
                .map(|workspace| (workspace.id.clone(), workspace.name.clone()))
                .collect(),
            known_tags: tags
                .iter()
                .map(|tag| (tag.id.clone(), tag.label.clone()))
                .collect(),
        }
    }

    fn sync_options(&mut self) {
        let state = self.context.store.borrow();
        let workspaces = state.state().workspaces.clone();
        let tags = state.state().tags.clone();
        drop(state);
        let known_workspaces = workspaces
            .iter()
            .map(|workspace| (workspace.id.clone(), workspace.name.clone()))
            .collect::<Vec<_>>();
        let known_tags = tags
            .iter()
            .map(|tag| (tag.id.clone(), tag.label.clone()))
            .collect::<Vec<_>>();
        let mut ctx = EventCtx::default();

        if known_workspaces != self.known_workspaces {
            let workspace_ids = workspaces
                .iter()
                .map(|workspace| workspace.id.as_str())
                .collect::<Vec<_>>();
            if self
                .active_workspace_filter
                .borrow()
                .as_deref()
                .is_some_and(|id| !workspace_ids.contains(&id))
            {
                *self.active_workspace_filter.borrow_mut() = None;
            }
            self.controls
                .replace(
                    "workspace",
                    workspace_filter_dropdown(
                        &workspaces,
                        Rc::clone(&self.active_workspace_filter),
                        Rc::clone(&self.filter_submitted),
                    ),
                    FlexItem::content(),
                    &mut ctx,
                )
                .expect("task filters should contain workspace filter");
            self.known_workspaces = known_workspaces;
        }
        if known_tags != self.known_tags {
            let tag_ids = tags.iter().map(|tag| tag.id.as_str()).collect::<Vec<_>>();
            self.active_label_filter
                .borrow_mut()
                .retain(|id| tag_ids.contains(&id.as_str()));
            self.controls
                .replace(
                    "labels",
                    label_filter_dropdown(
                        &tags,
                        Rc::clone(&self.active_label_filter),
                        Rc::clone(&self.filter_submitted),
                    ),
                    FlexItem::content(),
                    &mut ctx,
                )
                .expect("task filters should contain label filter");
            self.known_tags = known_tags;
        }
    }

    fn is_visible(&self) -> bool {
        self.active_tab.get() != NOTES_TAB_INDEX
    }

    fn finish_event(
        &self,
        outcome: EventOutcome,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        if self.filter_submitted.replace(false) {
            let focus = if self.active_tab.get() == CALENDAR_TAB_INDEX {
                initial_calendar_focus_request()
            } else {
                initial_task_table_focus_request()
            };
            ctx.focus(focus);
            ctx.stop_propagation();
            ctx.request_redraw();
            return EventOutcome::Handled;
        }
        if !detail_escape(event) {
            return outcome;
        }
        let focus = if self.active_tab.get() == CALENDAR_TAB_INDEX {
            initial_calendar_focus_request()
        } else {
            initial_task_table_focus_request()
        };
        ctx.focus(focus);
        ctx.stop_propagation();
        ctx.request_redraw();
        EventOutcome::Handled
    }
}

impl TuiNode<AppMsg> for TaskFilterControls {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        if self.is_visible() {
            self.controls.measure(proposal)
        } else {
            LayoutSizeHint::fixed(0, 0)
        }
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        if !self.is_visible() {
            return LayoutResult::new(area);
        }
        self.sync_options();
        self.controls.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        if self.is_visible() {
            self.controls.render(frame, area, ctx);
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        if !self.is_visible() {
            return EventOutcome::Ignored;
        }
        let outcome = self.controls.event(event, ctx);
        self.finish_event(outcome, event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        if !self.is_visible() {
            return EventOutcome::Ignored;
        }
        let outcome = self.controls.dispatch_event(route, event, ctx);
        self.finish_event(outcome, event, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        if self.is_visible() {
            self.controls.dispatch_focus(target, focused, ctx);
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.controls.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.controls.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.controls.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.controls.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.controls.destroy(ctx);
    }
}

struct TaskWorkspace {
    context: AppContext,
    layout: TaskWorkspaceLayout,
    task_view: TaskView,
    pending_task_view: TaskViewChange,
    active_task_view: ActiveTaskView,
    workspace_filter: Option<String>,
    active_workspace_filter: ActiveWorkspaceFilter,
    label_filter: Vec<String>,
    active_label_filter: ActiveLabelFilter,
    known_task_ids: Vec<String>,
    known_tags: Vec<(String, String)>,
    visible_task_ids: Vec<String>,
    visible_selection: VisibleTaskSelection,
    table_focused: bool,
    detail_draft_protected: bool,
    observed_version: u64,
    observed_external_refresh_version: u64,
    pending_navigation: PendingTaskNavigation,
    active_selection_invocation: Option<PersistenceSelectionInvocation>,
}

#[derive(Debug, Default)]
struct TaskDetailSync {
    changed: bool,
    selected_task_changed: bool,
}

impl TaskWorkspace {
    #[cfg(test)]
    fn new(context: AppContext) -> Self {
        Self::new_with_filters(
            context,
            Rc::new(RefCell::new(None)),
            Rc::new(RefCell::new(Vec::new())),
            Rc::new(RefCell::new(None)),
            Rc::new(RefCell::new(None)),
        )
    }

    fn new_with_filters(
        context: AppContext,
        active_workspace_filter: ActiveWorkspaceFilter,
        active_label_filter: ActiveLabelFilter,
        pending_task_view: TaskViewChange,
        pending_navigation: PendingTaskNavigation,
    ) -> Self {
        let task_view = TaskView::Active;
        let state = context.store.borrow().state().clone();
        let workspace_filter = active_workspace_filter.borrow().clone();
        let label_filter = active_label_filter.borrow().clone();
        let rows = task_rows_for_view(
            &state.tasks,
            task_view,
            workspace_filter.as_deref(),
            &label_filter,
        );
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
        if let Some(task) = selected_task_id
            .as_deref()
            .and_then(|id| state.tasks.iter().find(|task| task.id == id))
        {
            for url in stale_link_urls(task) {
                context
                    .coordinator
                    .borrow_mut()
                    .fetch_task_link_title(task.id.clone(), url);
            }
        }

        let active_task_view = Rc::new(RefCell::new(task_view));
        let visible_selection = Rc::new(RefCell::new(selected_task_id.clone()));
        let toolbar = task_toolbar(Rc::clone(&pending_task_view), Rc::clone(&active_task_view));
        let layout = task_workspace_layout(
            toolbar,
            &context.store,
            task_view,
            workspace_filter.as_deref(),
            &label_filter,
        );
        let observed_version = context.store.borrow().state().version;
        let observed_external_refresh_version = state.external_refresh_version;
        Self {
            context,
            layout,
            task_view,
            pending_task_view,
            active_task_view,
            workspace_filter,
            active_workspace_filter,
            label_filter,
            active_label_filter,
            known_task_ids: state.tasks.iter().map(|task| task.id.clone()).collect(),
            known_tags: state
                .tags
                .iter()
                .map(|tag| (tag.id.clone(), tag.label.clone()))
                .collect(),
            visible_task_ids,
            visible_selection,
            table_focused: false,
            detail_draft_protected: false,
            observed_version,
            observed_external_refresh_version,
            pending_navigation,
            active_selection_invocation: None,
        }
    }

    fn task_list(&self) -> &TaskTable {
        self.layout.first().table()
    }

    fn task_list_mut(&mut self) -> &mut TaskTable {
        self.layout.first_mut().table_mut()
    }

    fn table(&self) -> &DataView<TaskRow, String> {
        self.task_list().data_view()
    }

    fn table_mut(&mut self) -> &mut DataView<TaskRow, String> {
        self.task_list_mut().data_view_mut()
    }

    fn detail(&self) -> &TaskDetail {
        self.layout.second()
    }

    fn detail_mut(&mut self) -> &mut TaskDetail {
        self.layout.second_mut()
    }

    fn sync_store_version(&mut self) {
        self.sync_store_version_with_ctx(&mut EventCtx::default());
    }

    fn sync_store_version_with_ctx(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let state = self.context.store.borrow().state().clone();
        let external_refresh =
            self.observed_external_refresh_version != state.external_refresh_version;
        self.context.resolve_persistence_selection_outcomes();
        if let Some(invocation) = self.active_selection_invocation
            && self.context.take_selection_clear_request(invocation)
        {
            self.task_list_mut().clear_transient_selection();
        }
        if self.observed_version != state.version || external_refresh {
            let rollback_highlight = self.active_selection_invocation.and_then(|invocation| {
                self.context
                    .rollback_transient_selection_highlight(invocation)
            });
            let selected_new_backlog = state
                .selected_task_id
                .as_deref()
                .filter(|id| !self.known_task_ids.iter().any(|known| known == *id))
                .and_then(|id| state.tasks.iter().find(|task| task.id == id))
                .is_some_and(|task| task.state == TaskState::Backlog);
            if selected_new_backlog && !matches!(self.task_view, TaskView::All | TaskView::Backlog)
            {
                self.table_mut().clear_search();
                self.task_view = TaskView::Backlog;
                *self.active_task_view.borrow_mut() = TaskView::Backlog;
            }
            let protect_detail = external_refresh
                && (self.detail_draft_protected || self.context.coordinator.borrow().has_pending());
            self.refresh_from_state(
                &state,
                false,
                !external_refresh,
                external_refresh && !protect_detail,
                rollback_highlight.as_deref(),
                Some(ctx),
            );
            if selected_new_backlog {
                self.table_mut().reveal_highlighted();
            }
        }
    }

    fn sync_navigation(&mut self) -> bool {
        let navigation = self.pending_navigation.borrow_mut().take();
        let Some(navigation) = navigation else {
            return false;
        };
        self.table_mut().clear_search();
        self.task_view = navigation.view;
        *self.active_task_view.borrow_mut() = navigation.view;
        self.workspace_filter = None;
        *self.active_workspace_filter.borrow_mut() = None;
        self.label_filter.clear();
        self.active_label_filter.borrow_mut().clear();
        if let Some(source_task_id) = navigation.source_task_id {
            self.detail_mut().queue_issue_link_highlight(source_task_id);
        }
        self.context
            .store
            .borrow_mut()
            .dispatch(AppEvent::SelectTask(navigation.target_task_id.clone()));
        let state = self.context.store.borrow().state().clone();
        self.refresh_from_state(
            &state,
            false,
            false,
            false,
            Some(&navigation.target_task_id),
            None,
        );
        true
    }

    fn refresh_from_state(
        &mut self,
        state: &AppState,
        select_first: bool,
        preserve_position: bool,
        refresh_detail: bool,
        preferred_task_id: Option<&str>,
        detail_ctx: Option<&mut EventCtx<AppMsg>>,
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
        self.sync_filter_options(&state.workspaces, &state.tags);
        let rows = task_rows_for_view(
            &state.tasks,
            self.task_view,
            self.workspace_filter.as_deref(),
            &self.label_filter,
        );
        let empty_state = task_empty_state(&state.tasks, self.task_view);
        let contains_id = |id: &str| rows.iter().any(|task| task.id == id);
        let selected_from_state = state
            .selected_task_id
            .as_deref()
            .filter(|id| contains_id(id))
            .map(str::to_string);
        let selected_from_table = previous_task_id.filter(|id| contains_id(id));
        let preferred_task_id = preferred_task_id
            .filter(|id| contains_id(id))
            .map(str::to_string);
        let selected_task_id = if let Some(task_id) = preferred_task_id {
            Some(task_id)
        } else if select_first {
            rows.first().map(|task| task.id.clone())
        } else {
            let selected = if external_refresh {
                selected_from_state.or(selected_from_table)
            } else {
                selected_from_table.or(selected_from_state)
            };
            selected
                .or_else(|| {
                    previous_index.flatten().and_then(|index| {
                        rows.get(index.min(rows.len().saturating_sub(1)))
                            .map(|task| task.id.clone())
                    })
                })
                .or_else(|| rows.first().map(|task| task.id.clone()))
        };
        self.visible_task_ids = rows.iter().map(|task| task.id.clone()).collect();
        self.task_list_mut().set_rows(rows);
        self.task_list_mut().set_empty_state(empty_state);
        if self.task_list().transient_selected_ids().is_empty()
            && let Some(task_id) = selected_task_id.as_ref()
        {
            self.table_mut().highlight_id(task_id);
            self.table_mut().select_id(task_id.clone());
        }
        self.task_list_mut().take_data_view_events();
        let selected_task_id = self.table().highlighted_id();
        let selected_task = selected_task_id
            .as_deref()
            .and_then(|id| state.tasks.iter().find(|task| task.id == id));
        let save_error = selected_task
            .and_then(|task| state.task_status_error(&task.id))
            .map(str::to_string);
        *self.visible_selection.borrow_mut() = selected_task_id.clone();
        self.layout.set_second_visible(selected_task_id.is_some());

        if let Some(task_id) = selected_task_id.as_ref()
            && state.selected_task_id.as_ref() != Some(task_id)
        {
            self.context
                .store
                .borrow_mut()
                .dispatch(AppEvent::SelectTask(task_id.clone()));
        }

        let detail_identity_changed = self.detail().task_id.as_deref()
            != selected_task_id.as_deref()
            || self.detail().task_state != selected_task.map(|task| task.state);
        let detail_options_changed = self.detail().people_snapshot != state.people
            || self.detail().workspaces_snapshot != state.workspaces
            || self.detail().tags_snapshot != state.tags
            || self.detail().tasks_snapshot != state.tasks;
        let detail_content_changed =
            self.detail().task_snapshot.as_ref() != selected_task || detail_options_changed;
        if detail_identity_changed
            || (!external_refresh && detail_options_changed)
            || (refresh_detail && detail_content_changed)
        {
            let mut default_ctx = EventCtx::default();
            self.detail_mut().set_task(
                selected_task,
                (&state.tasks, &state.people, &state.workspaces, &state.tags),
                save_error.as_deref(),
                detail_ctx.unwrap_or(&mut default_ctx),
            );
        } else {
            self.detail_mut().task_state = selected_task.map(|task| task.state);
            if !external_refresh {
                let detail = self.detail_mut();
                detail.task_snapshot = selected_task.cloned();
                detail.tasks_snapshot = state.tasks.clone();
                detail.people_snapshot = state.people.clone();
                detail.workspaces_snapshot = state.workspaces.clone();
                detail.tags_snapshot = state.tags.clone();
            }
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
        let reorderability_changed =
            (next_view == TaskView::Archived) != (self.task_view == TaskView::Archived);
        self.table_mut().clear_search();
        self.task_view = next_view;
        *self.active_task_view.borrow_mut() = next_view;
        let state = self.context.store.borrow().state().clone();
        let preserve_selected = state
            .selected_task_id
            .as_deref()
            .and_then(|id| state.tasks.iter().find(|task| task.id == id))
            .is_some_and(|task| next_view.contains(task));
        if reorderability_changed {
            let toolbar = task_toolbar(
                Rc::clone(&self.pending_task_view),
                Rc::clone(&self.active_task_view),
            );
            self.layout = task_workspace_layout(
                toolbar,
                &self.context.store,
                self.task_view,
                self.workspace_filter.as_deref(),
                &self.label_filter,
            );
        }
        self.refresh_from_state(&state, !preserve_selected, false, false, None, None);
        true
    }

    fn sync_label_filter_change(&mut self) -> bool {
        let next_filter = self.active_label_filter.borrow().clone();
        if next_filter == self.label_filter {
            return false;
        }
        self.table_mut().clear_search();
        self.label_filter = next_filter;
        let state = self.context.store.borrow().state().clone();
        self.refresh_from_state(&state, false, false, false, None, None);
        true
    }

    fn sync_workspace_filter_change(&mut self) -> bool {
        let next_filter = self.active_workspace_filter.borrow().clone();
        if next_filter == self.workspace_filter {
            return false;
        }
        self.table_mut().clear_search();
        self.workspace_filter = next_filter;
        let state = self.context.store.borrow().state().clone();
        self.refresh_from_state(&state, false, false, false, None, None);
        true
    }

    fn sync_filter_options(&mut self, workspaces: &[Workspace], tags: &[Tag]) {
        if self
            .workspace_filter
            .as_ref()
            .is_some_and(|id| !workspaces.iter().any(|workspace| workspace.id == *id))
        {
            self.workspace_filter = None;
            *self.active_workspace_filter.borrow_mut() = None;
        }
        let known_tags = tags
            .iter()
            .map(|tag| (tag.id.clone(), tag.label.clone()))
            .collect::<Vec<_>>();
        if known_tags == self.known_tags {
            return;
        }
        let tag_ids = tags.iter().map(|tag| tag.id.clone()).collect::<Vec<_>>();
        self.label_filter.retain(|id| tag_ids.contains(id));
        self.active_label_filter
            .borrow_mut()
            .retain(|id| tag_ids.contains(id));
        self.known_tags = known_tags;
    }

    fn sync_table_events(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let list_events = self.task_list_mut().take_events();
        for event in list_events {
            match event {
                ListControlEvent::Reordered { row_ids } => {
                    let state = self.context.store.borrow().state().clone();
                    if persist_task_order(&self.context, &state, &row_ids, None) {
                        ctx.notify(tuicore::Notification::success(
                            "Tasks reordered",
                            "Task order was updated.",
                        ));
                        ctx.request_layout();
                        ctx.request_redraw();
                    }
                }
                ListControlEvent::ReorderUnavailable { reason } => {
                    ctx.notify(tuicore::Notification::warning(
                        "Cannot move tasks",
                        format!("Task ordering is unavailable: {reason:?}"),
                    ));
                }
                ListControlEvent::Added { .. }
                | ListControlEvent::AddedChild { .. }
                | ListControlEvent::Removed { .. }
                | ListControlEvent::Edited { .. }
                | ListControlEvent::AddCancelled
                | ListControlEvent::EditCancelled { .. }
                | ListControlEvent::TreeMoved { .. }
                | ListControlEvent::TreeBlockMoved { .. }
                | ListControlEvent::TreeBlockMoveCancelled { .. }
                | ListControlEvent::CheckedChanged { .. }
                | ListControlEvent::ReorderCancelled { .. } => {}
            }
        }
        let events = self.task_list_mut().take_data_view_events();
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
                    selected_changed |= self.clear_task_detail(ctx);
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
        if let Some(task) = selected_task {
            for url in stale_link_urls(task) {
                self.context
                    .coordinator
                    .borrow_mut()
                    .fetch_task_link_title(task.id.clone(), url);
            }
        }
        let save_error = selected_task.and_then(|task| state.task_status_error(&task.id));
        if self.detail().task_id.as_deref() != Some(id) {
            self.detail_mut().set_task(
                selected_task,
                (&state.tasks, &state.people, &state.workspaces, &state.tags),
                save_error,
                ctx,
            );
        }
        let visibility_changed = self.layout.set_second_visible(true);
        outcome.changed || visibility_changed
    }

    fn clear_task_detail(&mut self, ctx: &mut EventCtx<AppMsg>) -> bool {
        *self.visible_selection.borrow_mut() = None;
        let state = self.context.store.borrow().state().clone();
        self.detail_mut().set_task(
            None,
            (&state.tasks, &state.people, &state.workspaces, &state.tags),
            None,
            ctx,
        );
        self.detail_draft_protected = false;
        self.layout.set_second_visible(false)
    }

    fn drain_detail_patches(&mut self) -> bool {
        let patches = self.detail_mut().take_patches();
        let mut changed = false;
        for (task_id, patch) in patches {
            changed |= self.apply_patch(task_id, patch);
        }
        changed
    }

    fn sync_detail_changes(&mut self, ctx: Option<&mut EventCtx<AppMsg>>) -> TaskDetailSync {
        if !self.drain_detail_patches() {
            return TaskDetailSync::default();
        }
        let previous_task_id = self.table().highlighted_id();
        let state = self.context.store.borrow().state().clone();
        let selected_task = self
            .visible_selection
            .borrow()
            .as_deref()
            .and_then(|id| state.tasks.iter().find(|task| task.id == id))
            .cloned();
        let detail = self.detail_mut();
        detail.task_snapshot = selected_task;
        detail.tasks_snapshot = state.tasks.clone();
        detail.people_snapshot = state.people.clone();
        detail.workspaces_snapshot = state.workspaces.clone();
        detail.tags_snapshot = state.tags.clone();
        self.refresh_from_state(&state, false, true, false, None, ctx);
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

    fn can_toggle_task_progress(&self, task_id: &str) -> bool {
        self.context
            .store
            .borrow()
            .state()
            .tasks
            .iter()
            .any(|task| task.id == task_id)
    }

    fn table_action_task_ids(&self) -> Vec<String> {
        let selected = self.task_list().transient_selected_ids();
        if selected.is_empty() {
            self.table().highlighted_id().into_iter().collect()
        } else {
            selected
        }
    }

    fn handle_workspace_event(
        &mut self,
        outcome: EventOutcome,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        if ctx.propagation() == Propagation::Stopped && ctx.focus_request().is_some() {
            return outcome;
        }
        if detail_escape(event) {
            focus_task_table(ctx);
            return EventOutcome::Handled;
        }
        if outcome.handled() {
            return outcome;
        }
        let visible_task_id = self.visible_selection.borrow().clone();
        let table_task_ids = self.table_focused.then(|| self.table_action_task_ids());
        let message = if self.table_focused
            && keys::TASK_QUICK_MENU.matches(event)
            && let Some(task_ids) = table_task_ids
                .as_ref()
                .filter(|task_ids| !task_ids.is_empty())
        {
            if task_ids.len() == 1 && self.task_list().transient_selected_ids().is_empty() {
                Some(AppMsg::OpenTaskQuickMenu(task_ids[0].clone()))
            } else {
                Some(AppMsg::OpenTasksQuickMenu(task_ids.clone()))
            }
        } else if !self.table_focused
            && visible_task_id.is_some()
            && keys::TASK_QUICK_MENU.matches(event)
        {
            visible_task_id.map(AppMsg::OpenTaskQuickMenu)
        } else if self.table_focused
            && keys::TASK_COMPLETE.matches(event)
            && let Some(task_ids) = table_task_ids
                .as_ref()
                .filter(|task_ids| !task_ids.is_empty())
        {
            if task_ids.len() == 1 && self.task_list().transient_selected_ids().is_empty() {
                Some(AppMsg::OpenCompleteTask {
                    task_id: task_ids[0].clone(),
                    return_focus: None,
                })
            } else {
                Some(AppMsg::OpenCompleteTasks(task_ids.clone()))
            }
        } else if self.table_focused
            && keys::TASK_TOGGLE_PROGRESS.matches(event)
            && let Some(task_ids) = table_task_ids
                .as_ref()
                .filter(|task_ids| !task_ids.is_empty())
        {
            let state = self.context.store.borrow().state().clone();
            let tasks = tasks_for_ids(&state, task_ids);
            if tasks.is_empty() {
                None
            } else if task_ids.len() == 1 && self.task_list().transient_selected_ids().is_empty() {
                Some(AppMsg::ToggleTaskProgress(task_ids[0].clone()))
            } else {
                Some(AppMsg::CompleteTasks {
                    task_ids: task_ids.clone(),
                    state: toggled_tasks_progress_state(&tasks),
                })
            }
        } else if self.table_focused
            && app_keymap::matches_any(
                event,
                &[
                    keys::TASK_DELETE_CTRL_X,
                    keys::TASK_DELETE,
                    keys::TASK_DELETE_BACKSPACE,
                ],
            )
            && let Some(task_ids) = table_task_ids
                .as_ref()
                .filter(|task_ids| !task_ids.is_empty())
        {
            if task_ids.len() == 1 && self.task_list().transient_selected_ids().is_empty() {
                Some(AppMsg::OpenDeleteTask {
                    task_id: task_ids[0].clone(),
                    return_focus: None,
                })
            } else {
                Some(AppMsg::OpenDeleteTasks(task_ids.clone()))
            }
        } else if self.table_focused
            && keys::TASK_SNOOZE.matches(event)
            && let Some(task_ids) = table_task_ids
                .as_ref()
                .filter(|task_ids| !task_ids.is_empty())
        {
            if task_ids.len() == 1 && self.task_list().transient_selected_ids().is_empty() {
                Some(AppMsg::OpenTaskSnooze {
                    task_id: task_ids[0].clone(),
                    return_focus: None,
                })
            } else {
                Some(AppMsg::OpenTasksSnooze(task_ids.clone()))
            }
        } else {
            None
        };
        if let Some(message) = message {
            let transient_task_ids = self.task_list().transient_selected_ids();
            let selection_invocation = if self.table_focused && !transient_task_ids.is_empty() {
                self.context.begin_transient_selection(
                    TransientSelectionSource::TaskList,
                    transient_task_ids,
                    self.table().highlighted_id(),
                )
            } else {
                None
            };
            self.active_selection_invocation = selection_invocation;
            ctx.emit(selection_action(selection_invocation, message));
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        outcome
    }

    fn handle_task_shortcut_outside_table(
        &mut self,
        event: &TuiEvent,
        snooze_return_focus: Option<TreePath>,
        ctx: &mut EventCtx<AppMsg>,
    ) -> Option<EventOutcome> {
        if self.table_focused {
            return None;
        }
        let task_id = self.visible_selection.borrow().clone()?;
        if keys::TASK_TOGGLE_PROGRESS.matches(event) && self.can_toggle_task_progress(&task_id) {
            ctx.emit(AppMsg::ToggleTaskProgress(task_id));
            ctx.stop_propagation();
            return Some(EventOutcome::Handled);
        }
        if keys::TASK_SNOOZE.matches(event) {
            ctx.emit(AppMsg::OpenTaskSnooze {
                task_id,
                return_focus: snooze_return_focus.map(SnoozeReturnFocus::Path),
            });
            return Some(EventOutcome::Handled);
        }
        if keys::TASK_MOVE_MODE.matches(event) {
            focus_task_table(ctx);
            let outcome = self.task_list_mut().event(event, ctx);
            self.sync_table_events(ctx);
            return Some(outcome);
        }
        None
    }

    fn handle_detail_delete_shortcut(
        &self,
        child_outcome: EventOutcome,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> Option<EventOutcome> {
        let route_keys = route.path.keys();
        let detail_route = route_keys.first() == Some(&ChildKey::second());
        let links_route = route_keys.iter().any(|key| key.as_str() == "links");
        let checklist_route = route_keys.iter().any(|key| key.as_str() == "checklist");
        if child_outcome.handled()
            || ctx.propagation() != Propagation::Continue
            || !detail_route
            || links_route
            || checklist_route
            || !keys::TASK_DELETE_CTRL_X.matches(event)
        {
            return None;
        }
        let task_id = self.visible_selection.borrow().clone()?;
        ctx.emit(AppMsg::OpenDeleteTask {
            task_id,
            return_focus: Some(ctx.current_path()),
        });
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }

    fn handle_detail_complete_shortcut(
        &self,
        child_outcome: EventOutcome,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> Option<EventOutcome> {
        let route_keys = route.path.keys();
        let detail_route = route_keys.first() == Some(&ChildKey::second());
        if child_outcome.handled()
            || ctx.propagation() != Propagation::Continue
            || !detail_route
            || !keys::TASK_COMPLETE.matches(event)
        {
            return None;
        }
        let task_id = self.visible_selection.borrow().clone()?;
        ctx.emit(AppMsg::OpenCompleteTask {
            task_id,
            return_focus: Some(ctx.current_path()),
        });
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }

    fn handle_task_agent_yank(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> Option<EventOutcome> {
        let TuiEvent::Hotkey(HotkeyEvent::Commit(sequence)) = event else {
            return None;
        };
        if sequence != &keys::TASK_AGENT_YANK.hotkey()
            && sequence != &keys::TASK_AGENT_YANK_CLARIFY.hotkey()
        {
            return None;
        }
        let (task_ids, selected_group) = self.task_yank_ids();
        let store = self.context.store.borrow();
        let tasks = tasks_for_ids(store.state(), &task_ids);
        let command = if sequence == &keys::TASK_AGENT_YANK.hotkey() {
            task_agent_commands_for(store.state(), &tasks, "execute")
        } else {
            task_agent_commands_for(store.state(), &tasks, "clarify")
        };
        drop(store);
        if let Some(command) = command {
            let selection_invocation = if selected_group {
                self.context.begin_transient_selection(
                    TransientSelectionSource::TaskList,
                    task_ids.clone(),
                    self.table().highlighted_id(),
                )
            } else {
                None
            };
            self.active_selection_invocation = selection_invocation;
            self.context
                .accept_transient_clipboard(selection_invocation);
            ctx.copy_to_clipboard(command);
            if selection_invocation
                .is_some_and(|invocation| self.context.take_selection_clear_request(invocation))
            {
                self.task_list_mut().clear_transient_selection();
            }
        }
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }

    fn handle_task_reference_yank(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> Option<EventOutcome> {
        if !matches!(event, TuiEvent::Yank) {
            return None;
        }
        let (task_ids, selected_group) = self.task_yank_ids();
        let store = self.context.store.borrow();
        let tasks = tasks_for_ids(store.state(), &task_ids);
        if !tasks.is_empty() {
            let selection_invocation = if selected_group {
                self.context.begin_transient_selection(
                    TransientSelectionSource::TaskList,
                    task_ids.clone(),
                    self.table().highlighted_id(),
                )
            } else {
                None
            };
            self.active_selection_invocation = selection_invocation;
            self.context
                .accept_transient_clipboard(selection_invocation);
            ctx.copy_to_clipboard(task_references_for(store.state(), &tasks));
            drop(store);
            if selection_invocation
                .is_some_and(|invocation| self.context.take_selection_clear_request(invocation))
            {
                self.task_list_mut().clear_transient_selection();
            }
        }
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }

    fn task_yank_ids(&self) -> (Vec<String>, bool) {
        let selected = self.task_list().transient_selected_ids();
        if !selected.is_empty() {
            return (selected, true);
        }
        if self.table_focused {
            return (self.table().highlighted_id().into_iter().collect(), false);
        }
        (
            self.visible_selection.borrow().iter().cloned().collect(),
            false,
        )
    }
}

impl TuiNode<AppMsg> for TaskWorkspace {
    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.sync_navigation();
        self.sync_task_view_change();
        self.sync_store_version();
        self.sync_workspace_filter_change();
        self.sync_label_filter_change();
        self.detail_mut().set_layout_limits(
            area.width < crate::ui::responsive_split::MASTER_DETAIL_NARROW_BREAKPOINT,
            area.height,
        );
        self.layout.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.layout.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        self.sync_store_version_with_ctx(ctx);
        if let Some(outcome) = self.handle_task_agent_yank(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_task_reference_yank(event, ctx) {
            return outcome;
        }
        let outcome = self.layout.event(event, ctx);
        let view_changed = self.sync_task_view_change();
        let workspace_filter_changed = self.sync_workspace_filter_change();
        let label_filter_changed = self.sync_label_filter_change();
        let detail_sync = self.sync_detail_changes(Some(ctx));
        if view_changed || workspace_filter_changed || label_filter_changed || detail_sync.changed {
            ctx.request_layout();
            ctx.request_redraw();
        }
        if view_changed
            || workspace_filter_changed
            || label_filter_changed
            || detail_sync.selected_task_changed
        {
            ctx.focus(initial_task_table_focus_request());
        }
        self.sync_table_events(ctx);
        if !outcome.handled()
            && let Some(outcome) = self.handle_task_shortcut_outside_table(event, None, ctx)
        {
            return outcome;
        }
        self.handle_workspace_event(outcome, event, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        self.sync_store_version_with_ctx(ctx);
        if let Some(outcome) = self.handle_task_agent_yank(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_task_reference_yank(event, ctx) {
            return outcome;
        }
        let outcome = self.layout.dispatch_event(route, event, ctx);
        let view_changed = self.sync_task_view_change();
        let workspace_filter_changed = self.sync_workspace_filter_change();
        let label_filter_changed = self.sync_label_filter_change();
        let detail_sync = self.sync_detail_changes(Some(ctx));
        if view_changed || workspace_filter_changed || label_filter_changed || detail_sync.changed {
            ctx.request_layout();
            ctx.request_redraw();
        }
        if view_changed
            || workspace_filter_changed
            || label_filter_changed
            || detail_sync.selected_task_changed
        {
            ctx.focus(initial_task_table_focus_request());
        }
        self.sync_table_events(ctx);
        if let Some(outcome) = self.handle_detail_delete_shortcut(outcome, route, event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.handle_detail_complete_shortcut(outcome, route, event, ctx) {
            return outcome;
        }
        if !outcome.handled()
            && let Some(outcome) =
                self.handle_task_shortcut_outside_table(event, Some(ctx.current_path()), ctx)
        {
            return outcome;
        }
        self.handle_workspace_event(outcome, event, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        let table_targeted = target
            .for_child(&ChildKey::first())
            .and_then(|target| target.for_child(&ChildKey::second()))
            .is_some();
        if table_targeted {
            self.table_focused = focused;
        } else if focused {
            self.table_focused = false;
        }
        let detail_targeted = target.for_child(&ChildKey::second()).is_some();
        if detail_targeted {
            self.detail_draft_protected = focused;
        } else if focused {
            self.detail_draft_protected = false;
        }
        self.layout.dispatch_focus(target, focused, ctx);
        let detail_sync = self.sync_detail_changes(None);
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

mod dialogs;
use dialogs::*;

pub(crate) mod task_detail;
use task_detail::*;

#[cfg(test)]
#[path = "app/tests.rs"]
pub(crate) mod tests;
