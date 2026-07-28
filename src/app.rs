use std::{cell::RefCell, error::Error, rc::Rc, time::Duration};

use crate::app_keymap::{self, keys};
use crate::calendar::{CalendarWorkspace, SHOW_WEEKENDS_SETTING, parse_show_weekends_setting};
use crate::create_management_dialog::{CreateManagementDialog, ManagementEntityDraft};
use crate::create_task_dialog::{CreateTaskDialog, CreateTaskDraft};
use crate::domain::{
    AppEvent, AppState, Person, Project, Tag, Task, TaskPatch, TaskPriority, TaskRank, TaskSize,
    TaskState, reduce_app_state,
};
use crate::persistence_coordinator::{AppStore, PersistenceCommand, PersistenceCoordinator};
use crate::service::TuidoService;
use crate::settings_dialog::SettingsDialog;
use crate::snooze::{
    DEFAULT_SNOOZE_TIME_SETTING, SnoozeDialog, format_default_snooze_time, local_now,
    parse_default_snooze_time,
};
use crate::storage::Storage;
use crate::task_quick_menu::TaskQuickMenu;
use crate::task_title::format_title;
use crate::ui::management::{ManagementDialogKind, people, projects, tags};
use crate::ui::responsive_split::ResponsiveSplit;
use crate::ui::save_status::SaveStatusLine;
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
    DataViewTypedEvent, DatePickerDropdown, DateTimePickerDropdown, Dialog, DialogBackdrop,
    DialogHost, DialogLayer, Dropdown, DropdownCommitMode, DropdownSearchMode, EventCtx,
    EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusId, FocusRequest, FocusTarget,
    HotkeyEvent, HotkeyLabelMode, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint,
    LifecycleCtx, ListControl, ListControlEvent, ListControlField, ListControlKeyBindings, Menu,
    MenuItem, MenuSearchMode, Paragraph, RenderCtx, SelectedTag, SelectionMode, SelectionTrigger,
    Split, StatusBar, StatusBarMenuItem, Store, Tab, Tabs, TabsVariant, TagInput, TagInputEvent,
    TextInput, TextareaInput, TickResult, TreeApp, TreePath, TuiEvent, TuiNode,
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
const SETTINGS_MENU_ID: &str = "settings";
const STATUS_BAR_MENU_ITEMS: [StatusBarMenuItem; 6] = [
    StatusBarMenuItem::Custom {
        id: SETTINGS_MENU_ID,
        label: "Settings",
    },
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

fn default_snooze_time() -> Time {
    parse_default_snooze_time(None).expect("default snooze time should be valid")
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

#[derive(Debug)]
pub(crate) enum AppMsg {
    Noop,
    OpenSettings,
    SetShowCalendarWeekends(bool),
    SetDefaultSnoozeTime(Time),
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
    OpenTaskQuickMenu(String),
    MoveTaskToTop(String),
    MoveTaskToBottom(String),
    OpenTaskSnooze(String),
    SnoozeTask {
        task_id: String,
        until: PrimitiveDateTime,
        remember_custom: Option<PrimitiveDateTime>,
    },
    UnsnoozeTask(String),
    CloseManagementOverlay,
    CloseSnoozeDialog,
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
    let default_snooze_time = parse_default_snooze_time(
        runtime
            .block_on(service.app_setting(DEFAULT_SNOOZE_TIME_SETTING))?
            .as_deref(),
    )
    .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidData, message))?;
    let mut app_state = AppState::from_snapshot(workspace.snapshot);
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
        AppMsg::OpenSettings => app.open_settings_dialog(ctx),
        AppMsg::SetShowCalendarWeekends(show) => app.set_show_calendar_weekends(show),
        AppMsg::SetDefaultSnoozeTime(time) => app.set_default_snooze_time(time),
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
        AppMsg::OpenTaskQuickMenu(task_id) => app.open_task_quick_menu(&task_id, ctx),
        AppMsg::MoveTaskToTop(task_id) => app.move_task_to_edge(&task_id, true, ctx),
        AppMsg::MoveTaskToBottom(task_id) => app.move_task_to_edge(&task_id, false, ctx),
        AppMsg::OpenTaskSnooze(task_id) => app.open_task_snooze_dialog(&task_id, ctx),
        AppMsg::SnoozeTask {
            task_id,
            until,
            remember_custom,
        } => app.snooze_task(task_id, until, remember_custom, ctx),
        AppMsg::UnsnoozeTask(task_id) => app.unsnooze_task(task_id, ctx),
        AppMsg::CloseManagementOverlay => app.close_management_overlay(ctx),
        AppMsg::CloseSnoozeDialog => app.close_snooze_dialog(ctx),
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
                    SETTINGS_MENU_ID => AppMsg::OpenSettings,
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
        drop(state);
        let settings = SettingsDialog::new(show_weekends, default_time);
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

    fn set_show_calendar_weekends(&mut self, show: bool) {
        self.persist_app_setting(SHOW_WEEKENDS_SETTING, show.to_string());
    }

    fn set_default_snooze_time(&mut self, time: Time) {
        self.persist_app_setting(
            DEFAULT_SNOOZE_TIME_SETTING,
            format_default_snooze_time(time),
        );
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

        let mut task = Task::quick_capture(
            Uuid::new_v4().to_string(),
            title,
            String::new(),
            TaskSize::Small,
        );
        task.rank = self
            .context
            .store
            .borrow()
            .state()
            .tasks
            .iter()
            .map(|task| task.rank)
            .max()
            .unwrap_or(0)
            + 1;
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

    fn open_task_quick_menu(&mut self, task_id: &str, ctx: &mut EventCtx<AppMsg>) {
        if self.task(task_id).is_none() {
            return;
        }
        let primary = self.primary_dialog();
        primary.replace_layer(
            AppDialog::TaskQuickMenu(Box::new(TaskQuickMenu::new(task_id.to_string()))),
            ctx,
        );
        primary.set_layer_percent(40);
        primary.set_layer_cross_percent(35);
        primary.set_fit_content(true);
        primary.set_active_with_context(true, ctx);
    }

    fn move_task_to_edge(&mut self, task_id: &str, to_top: bool, ctx: &mut EventCtx<AppMsg>) {
        let state = self.context.store.borrow().state().clone();
        let mut ordered = state
            .tasks
            .iter()
            .map(|task| task.id.clone())
            .collect::<Vec<_>>();
        let Some(index) = ordered.iter().position(|id| id == task_id) else {
            return;
        };
        let task_id = ordered.remove(index);
        if to_top {
            ordered.insert(0, task_id);
        } else {
            ordered.push(task_id);
        }
        persist_task_order(&self.context, &state, &ordered);
        self.close_dialog(ctx);
        focus_task_table(ctx);
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

    fn close_snooze_dialog(&mut self, ctx: &mut EventCtx<AppMsg>) {
        self.close_dialog(ctx);
        focus_task_table(ctx);
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
type TaskTable = ListControl<TaskRow, String, AppMsg>;
type TaskDetail = TaskDetailForm;
type TaskPane = ResponsiveSplit<TaskTable, TaskDetail>;
type TaskWorkspaceLayout = Split<Flex<AppMsg>, TaskPane>;
type TaskViewChange = Rc<RefCell<Option<TaskView>>>;
type ActiveTaskView = Rc<RefCell<TaskView>>;
type VisibleTaskSelection = Rc<RefCell<Option<String>>>;
type PatchSink = Rc<RefCell<Vec<TaskPatch>>>;

fn persist_task_order(context: &AppContext, state: &AppState, ordered_ids: &[String]) -> bool {
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
    context
        .coordinator
        .borrow_mut()
        .submit(PersistenceCommand::ReorderTasks {
            before,
            after,
            expected_revisions: std::collections::HashMap::new(),
        });
    true
}

fn initial_task_table_focus_request() -> FocusRequest {
    FocusRequest::TargetAt {
        path: TreePath::from_keys([
            ChildKey::first(),
            ChildKey::first(),
            ChildKey::new("tabs"),
            ChildKey::new("tab-0"),
            ChildKey::second(),
            ChildKey::first(),
            ChildKey::new("data"),
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
    Active,
    Backlog,
    Snoozed,
    Archived,
    All,
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
        let task_view = TaskView::Active;
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

    fn task_list(&self) -> &TaskTable {
        self.layout.second().first()
    }

    fn task_list_mut(&mut self) -> &mut TaskTable {
        self.layout.second_mut().first_mut()
    }

    fn table(&self) -> &DataView<TaskRow, String> {
        self.task_list().data_view()
    }

    fn table_mut(&mut self) -> &mut DataView<TaskRow, String> {
        self.task_list_mut().data_view_mut()
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
            let selected_new_active = state
                .selected_task_id
                .as_deref()
                .filter(|id| !self.known_task_ids.iter().any(|known| known == *id))
                .and_then(|id| state.tasks.iter().find(|task| task.id == id))
                .is_some_and(|task| task.state == TaskState::Todo);
            if selected_new_active && !matches!(self.task_view, TaskView::All | TaskView::Active) {
                self.table_mut().clear_search();
                self.task_view = TaskView::Active;
                *self.active_task_view.borrow_mut() = TaskView::Active;
            }
            let protect_detail = external_refresh
                && (self.detail_draft_protected || self.context.coordinator.borrow().has_pending());
            self.refresh_from_state(
                &state,
                false,
                !external_refresh,
                external_refresh && !protect_detail,
            );
            if selected_new_active {
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
        let list_events = self.task_list_mut().take_events();
        for event in list_events {
            match event {
                ListControlEvent::Reordered { row_ids } => {
                    let state = self.context.store.borrow().state().clone();
                    if persist_task_order(&self.context, &state, &row_ids) {
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
                | ListControlEvent::Removed { .. }
                | ListControlEvent::Edited { .. }
                | ListControlEvent::AddCancelled
                | ListControlEvent::EditCancelled { .. }
                | ListControlEvent::ReorderCancelled { .. } => {}
            }
        }
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
        let message = if visible_task_id.is_some() && keys::TASK_QUICK_MENU.matches(event) {
            visible_task_id.map(AppMsg::OpenTaskQuickMenu)
        } else if self.table_focused
            && visible_task_id.is_some()
            && app_keymap::matches_any(
                event,
                &[
                    keys::TASK_DELETE_CTRL_X,
                    keys::TASK_DELETE,
                    keys::TASK_DELETE_BACKSPACE,
                ],
            )
        {
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

    fn handle_task_shortcut_outside_table(
        &mut self,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> Option<EventOutcome> {
        if self.table_focused {
            return None;
        }
        let task_id = self.visible_selection.borrow().clone()?;
        if keys::TASK_SNOOZE.matches(event) {
            focus_task_table(ctx);
            ctx.emit(AppMsg::OpenTaskSnooze(task_id));
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

    fn handle_task_agent_yank(
        &self,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> Option<EventOutcome> {
        let TuiEvent::Hotkey(HotkeyEvent::Commit(sequence)) = event else {
            return None;
        };
        if sequence != &keys::TASK_AGENT_YANK.hotkey() {
            return None;
        }
        let task_title = self
            .visible_selection
            .borrow()
            .as_ref()
            .and_then(|task_id| {
                self.context
                    .store
                    .borrow()
                    .state()
                    .tasks
                    .iter()
                    .find(|task| task.id == *task_id)
                    .map(|task| task.title.clone())
            });
        if let Some(task_title) = task_title {
            ctx.copy_to_clipboard(format!("Tuido execute \"{task_title}\""));
        }
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
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
        if let Some(outcome) = self.handle_task_agent_yank(event, ctx) {
            return outcome;
        }
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
        if !outcome.handled()
            && let Some(outcome) = self.handle_task_shortcut_outside_table(event, ctx)
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
        self.sync_store_version();
        if let Some(outcome) = self.handle_task_agent_yank(event, ctx) {
            return outcome;
        }
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
        if !outcome.handled()
            && let Some(outcome) = self.handle_task_shortcut_outside_table(event, ctx)
        {
            return outcome;
        }
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

mod dialogs;
use dialogs::*;

mod task_detail;
use task_detail::*;

#[cfg(test)]
#[path = "app/tests.rs"]
pub(crate) mod tests;
