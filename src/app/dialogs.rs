use super::*;

pub(super) enum AppDialog {
    People(Box<people::PeopleDialog>),
    Workspaces(Box<workspaces::WorkspacesDialog>),
    Tags(Box<tags::TagsDialog>),
    CreateManagement(DialogHost<CreateManagementDialog, AppMsg>),
    DeleteManagement(ConfirmationDialog<AppMsg>),
    CreateTask(DialogHost<CreateTaskDialog, AppMsg>),
    DeleteTask(ConfirmationDialog<AppMsg>),
    Generic(Dialog<AppMsg>),
    TaskQuickMenu(Box<TaskQuickMenu>),
    Settings(DialogHost<SettingsDialog, AppMsg>),
    Empty(Dialog<AppMsg>),
    Snooze(Box<SnoozeDialog>),
}

pub(super) fn empty_app_dialog() -> AppDialog {
    AppDialog::Empty(Dialog::new())
}

pub(super) fn management_dialog(context: AppContext, kind: ManagementDialogKind) -> AppDialog {
    match kind {
        ManagementDialogKind::People => AppDialog::People(Box::new(people::dialog(context))),
        ManagementDialogKind::Workspaces => {
            AppDialog::Workspaces(Box::new(workspaces::dialog(context)))
        }
        ManagementDialogKind::Tags => AppDialog::Tags(Box::new(tags::dialog(context))),
    }
}

impl TuiNode<AppMsg> for AppDialog {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        match self {
            Self::People(dialog) => dialog.measure(proposal),
            Self::Workspaces(dialog) => dialog.measure(proposal),
            Self::Tags(dialog) => dialog.measure(proposal),
            Self::CreateManagement(dialog) => measure_dialog_host(dialog, proposal),
            Self::DeleteManagement(dialog) => dialog.measure(proposal),
            Self::CreateTask(dialog) => measure_dialog_host(dialog, proposal),
            Self::DeleteTask(dialog) => dialog.measure(proposal),
            Self::Generic(dialog) => dialog.measure(proposal),
            Self::TaskQuickMenu(menu) => menu.measure(proposal),
            Self::Settings(dialog) => measure_dialog_host(dialog, proposal),
            Self::Empty(dialog) => dialog.measure(proposal),
            Self::Snooze(dialog) => dialog.measure(proposal),
        }
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        match self {
            Self::People(dialog) => dialog.layout(area, ctx),
            Self::Workspaces(dialog) => dialog.layout(area, ctx),
            Self::Tags(dialog) => dialog.layout(area, ctx),
            Self::CreateManagement(dialog) => dialog.layout(area, ctx),
            Self::DeleteManagement(dialog) => dialog.layout(area, ctx),
            Self::CreateTask(dialog) => dialog.layout(area, ctx),
            Self::DeleteTask(dialog) => dialog.layout(area, ctx),
            Self::Generic(dialog) => dialog.layout(area, ctx),
            Self::TaskQuickMenu(dialog) => dialog.layout(area, ctx),
            Self::Settings(dialog) => dialog.layout(area, ctx),
            Self::Empty(dialog) => dialog.layout(area, ctx),
            Self::Snooze(dialog) => dialog.layout(area, ctx),
        }
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        match self {
            Self::People(dialog) => dialog.render(frame, area, ctx),
            Self::Workspaces(dialog) => dialog.render(frame, area, ctx),
            Self::Tags(dialog) => dialog.render(frame, area, ctx),
            Self::CreateManagement(dialog) => dialog.render(frame, area, ctx),
            Self::DeleteManagement(dialog) => dialog.render(frame, area),
            Self::CreateTask(dialog) => dialog.render(frame, area, ctx),
            Self::DeleteTask(dialog) => dialog.render(frame, area),
            Self::Generic(dialog) => dialog.render(frame, area),
            Self::TaskQuickMenu(dialog) => dialog.render(frame, area, ctx),
            Self::Settings(dialog) => dialog.render(frame, area, ctx),
            Self::Empty(dialog) => dialog.render(frame, area),
            Self::Snooze(dialog) => dialog.render(frame, area, ctx),
        }
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        match self {
            Self::People(dialog) => dialog.event(event, ctx),
            Self::Workspaces(dialog) => dialog.event(event, ctx),
            Self::Tags(dialog) => dialog.event(event, ctx),
            Self::CreateManagement(dialog) => dialog.event(event, ctx),
            Self::DeleteManagement(dialog) => dialog.event(event, ctx),
            Self::CreateTask(dialog) => dialog.event(event, ctx),
            Self::DeleteTask(dialog) => dialog.event(event, ctx),
            Self::Generic(dialog) => dialog.event(event, ctx),
            Self::TaskQuickMenu(dialog) => dialog.event(event, ctx),
            Self::Settings(dialog) => dialog.event(event, ctx),
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
            Self::Workspaces(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::Tags(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::CreateManagement(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::DeleteManagement(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::CreateTask(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::DeleteTask(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::Generic(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::TaskQuickMenu(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::Settings(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::Empty(dialog) => dialog.dispatch_event(route, event, ctx),
            Self::Snooze(dialog) => dialog.dispatch_event(route, event, ctx),
        }
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        match self {
            Self::People(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::Workspaces(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::Tags(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::CreateManagement(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::DeleteManagement(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::CreateTask(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::DeleteTask(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::Generic(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::TaskQuickMenu(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::Settings(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::Empty(dialog) => dialog.dispatch_focus(target, focused, ctx),
            Self::Snooze(dialog) => dialog.dispatch_focus(target, focused, ctx),
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        match self {
            Self::People(dialog) => dialog.tick(dt, settings),
            Self::Workspaces(dialog) => dialog.tick(dt, settings),
            Self::Tags(dialog) => dialog.tick(dt, settings),
            Self::CreateManagement(dialog) => dialog.tick(dt, settings),
            Self::DeleteManagement(dialog) => dialog.tick(dt, settings),
            Self::CreateTask(dialog) => dialog.tick(dt, settings),
            Self::DeleteTask(dialog) => dialog.tick(dt, settings),
            Self::Generic(dialog) => dialog.tick(dt, settings),
            Self::TaskQuickMenu(dialog) => dialog.tick(dt, settings),
            Self::Settings(dialog) => dialog.tick(dt, settings),
            Self::Empty(dialog) => dialog.tick(dt, settings),
            Self::Snooze(dialog) => dialog.tick(dt, settings),
        }
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::People(dialog) => dialog.init(ctx),
            Self::Workspaces(dialog) => dialog.init(ctx),
            Self::Tags(dialog) => dialog.init(ctx),
            Self::CreateManagement(dialog) => dialog.init(ctx),
            Self::DeleteManagement(dialog) => dialog.init(ctx),
            Self::CreateTask(dialog) => dialog.init(ctx),
            Self::DeleteTask(dialog) => dialog.init(ctx),
            Self::Generic(dialog) => dialog.init(ctx),
            Self::TaskQuickMenu(dialog) => dialog.init(ctx),
            Self::Settings(dialog) => dialog.init(ctx),
            Self::Empty(dialog) => dialog.init(ctx),
            Self::Snooze(dialog) => dialog.init(ctx),
        }
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::People(dialog) => dialog.mount(ctx),
            Self::Workspaces(dialog) => dialog.mount(ctx),
            Self::Tags(dialog) => dialog.mount(ctx),
            Self::CreateManagement(dialog) => dialog.mount(ctx),
            Self::DeleteManagement(dialog) => dialog.mount(ctx),
            Self::CreateTask(dialog) => dialog.mount(ctx),
            Self::DeleteTask(dialog) => dialog.mount(ctx),
            Self::Generic(dialog) => dialog.mount(ctx),
            Self::TaskQuickMenu(dialog) => dialog.mount(ctx),
            Self::Settings(dialog) => dialog.mount(ctx),
            Self::Empty(dialog) => dialog.mount(ctx),
            Self::Snooze(dialog) => dialog.mount(ctx),
        }
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::People(dialog) => dialog.unmount(ctx),
            Self::Workspaces(dialog) => dialog.unmount(ctx),
            Self::Tags(dialog) => dialog.unmount(ctx),
            Self::CreateManagement(dialog) => dialog.unmount(ctx),
            Self::DeleteManagement(dialog) => dialog.unmount(ctx),
            Self::CreateTask(dialog) => dialog.unmount(ctx),
            Self::DeleteTask(dialog) => dialog.unmount(ctx),
            Self::Generic(dialog) => dialog.unmount(ctx),
            Self::TaskQuickMenu(dialog) => dialog.unmount(ctx),
            Self::Settings(dialog) => dialog.unmount(ctx),
            Self::Empty(dialog) => dialog.unmount(ctx),
            Self::Snooze(dialog) => dialog.unmount(ctx),
        }
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        match self {
            Self::People(dialog) => dialog.destroy(ctx),
            Self::Workspaces(dialog) => dialog.destroy(ctx),
            Self::Tags(dialog) => dialog.destroy(ctx),
            Self::CreateManagement(dialog) => dialog.destroy(ctx),
            Self::DeleteManagement(dialog) => dialog.destroy(ctx),
            Self::CreateTask(dialog) => dialog.destroy(ctx),
            Self::DeleteTask(dialog) => dialog.destroy(ctx),
            Self::Generic(dialog) => dialog.destroy(ctx),
            Self::TaskQuickMenu(dialog) => dialog.destroy(ctx),
            Self::Settings(dialog) => dialog.destroy(ctx),
            Self::Empty(dialog) => dialog.destroy(ctx),
            Self::Snooze(dialog) => dialog.destroy(ctx),
        }
    }
}

pub(super) fn measure_dialog_host<C: TuiNode<AppMsg>>(
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

pub(super) fn create_management_dialog_host(kind: ManagementDialogKind) -> AppDialog {
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

pub(super) fn delete_management_dialog(
    kind: ManagementDialogKind,
    entity_id: String,
    label: &str,
) -> AppDialog {
    let description = format!("Delete “{label}”? This cannot be undone.");
    let dialog = ConfirmationDialog::new(format!("Delete {}?", kind.singular()), &description)
        .yes_text("Delete")
        .yes_hotkey(keys::DELETE_CONFIRM.key_spec())
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

pub(super) fn notify_required(ctx: &mut EventCtx<AppMsg>, title: &str, body: &str) {
    ctx.notify(tuicore::Notification::warning(title, body));
}

pub(super) fn create_task_dialog_host() -> AppDialog {
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

pub(super) fn delete_task_dialog(task: &Task) -> AppDialog {
    let task_id = task.id.clone();
    let description = format!("Delete “{}”? This cannot be undone.", task.title);
    let dialog = ConfirmationDialog::new("Delete task?", &description)
        .yes_text("Delete")
        .yes_hotkey(keys::DELETE_CONFIRM.key_spec())
        .on_outcome(move |outcome| match outcome {
            ConfirmationDialogOutcome::Confirmed => AppMsg::DeleteTaskConfirmed(task_id.clone()),
            ConfirmationDialogOutcome::Cancelled | ConfirmationDialogOutcome::Closed(_) => {
                AppMsg::CloseDeleteTaskDialog
            }
        });
    AppDialog::DeleteTask(dialog)
}

pub(super) fn complete_task_dialog(task: &Task) -> AppDialog {
    let done_task_id = task.id.clone();
    let rejected_task_id = task.id.clone();
    AppDialog::Generic(
        Dialog::new()
            .top_left("Complete task?")
            .content([format!("Choose an outcome for “{}”.", task.title)])
            .keybindings(tuicore::DialogKeyBindings {
                close: vec![keys::DIALOG_CLOSE.key_spec()],
            })
            .actions([
                tuicore::DialogAction::new("Done")
                    .hotkey(keys::COMPLETE_DONE.key_spec())
                    .on_trigger(move || AppMsg::CompleteTask {
                        task_id: done_task_id.clone(),
                        state: TaskState::Done,
                    }),
                tuicore::DialogAction::new("Reject")
                    .hotkey(keys::COMPLETE_REJECT.key_spec())
                    .on_trigger(move || AppMsg::CompleteTask {
                        task_id: rejected_task_id.clone(),
                        state: TaskState::Rejected,
                    }),
                tuicore::DialogAction::new("Cancel")
                    .hotkey(keys::DIALOG_CANCEL.key_spec())
                    .on_trigger(|| AppMsg::CloseCompleteTaskDialog),
            ])
            .on_close(|_| AppMsg::CloseCompleteTaskDialog),
    )
}
