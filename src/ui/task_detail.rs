use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, ChildKey, ChildSlot, EventCtx, EventOutcome, EventRoute, Flex, FlexItem,
    FocusCtx, FocusId, FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint,
    LifecycleCtx, RenderCtx, ScrollContainer, TickResult, TuiEvent, TuiNode,
};

use crate::{
    app::{
        AppMsg,
        task_detail::{
            TASK_DESCRIPTION_DESKTOP_MAX_PERCENT, TASK_DESCRIPTION_DESKTOP_MIN_CONTENT_ROWS,
            TASK_DESCRIPTION_NARROW_MAX_CONTENT_ROWS, detail_form,
        },
    },
    domain::{Person, Tag, Task, TaskPatch, TaskState, Workspace},
    ui::save_status::SaveStatusLine,
};

pub(crate) type PatchSink = Rc<RefCell<Vec<TaskPatch>>>;
pub(crate) type TaskDetailCatalogs<'a> = (&'a [Task], &'a [Person], &'a [Workspace], &'a [Tag]);

pub(crate) struct TaskDetailForm {
    root: ScrollContainer<TaskDetailFormBody, AppMsg>,
    pub(crate) task_id: Option<String>,
    pub(crate) task_state: Option<TaskState>,
    pub(crate) task_snapshot: Option<Task>,
    pub(crate) tasks_snapshot: Vec<Task>,
    pub(crate) people_snapshot: Vec<Person>,
    pub(crate) workspaces_snapshot: Vec<Workspace>,
    pub(crate) tags_snapshot: Vec<Tag>,
    pub(crate) patches: PatchSink,
    checklist_highlighted_id: Rc<RefCell<Option<String>>>,
    pending_issue_link_highlight: Option<String>,
    save_status: SaveStatusLine,
    description_max_rows: Rc<Cell<Option<usize>>>,
}

impl TaskDetailForm {
    pub(crate) fn new(
        task: Option<&Task>,
        tasks: &[Task],
        people: &[Person],
        workspaces: &[Workspace],
        tags: &[Tag],
        save_error: Option<&str>,
    ) -> Self {
        let patches = Rc::new(RefCell::new(Vec::new()));
        let checklist_highlighted_id = Rc::new(RefCell::new(None));
        let save_status = SaveStatusLine::new(save_error);
        let description_max_rows = Rc::new(Cell::new(None));
        Self {
            root: ScrollContainer::vertical(TaskDetailFormBody::new(detail_form(
                task,
                (tasks, people, workspaces, tags),
                Rc::clone(&patches),
                Rc::clone(&checklist_highlighted_id),
                None,
                save_status.clone(),
                Rc::clone(&description_max_rows),
            )))
            .focus_reveal(true),
            task_id: task.map(|task| task.id.clone()),
            task_state: task.map(|task| task.state),
            task_snapshot: task.cloned(),
            tasks_snapshot: tasks.to_vec(),
            people_snapshot: people.to_vec(),
            workspaces_snapshot: workspaces.to_vec(),
            tags_snapshot: tags.to_vec(),
            patches,
            checklist_highlighted_id,
            pending_issue_link_highlight: None,
            save_status,
            description_max_rows,
        }
    }

    pub(crate) fn take_patches(&mut self) -> Vec<(String, TaskPatch)> {
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

    pub(crate) fn set_task(
        &mut self,
        task: Option<&Task>,
        catalogs: TaskDetailCatalogs<'_>,
        save_error: Option<&str>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        let (tasks, people, workspaces, tags) = catalogs;
        if self.task_id.as_deref() != task.map(|task| task.id.as_str()) {
            *self.checklist_highlighted_id.borrow_mut() = None;
        }
        self.patches = Rc::new(RefCell::new(Vec::new()));
        self.task_id = task.map(|task| task.id.clone());
        self.task_state = task.map(|task| task.state);
        self.task_snapshot = task.cloned();
        self.tasks_snapshot = tasks.to_vec();
        self.people_snapshot = people.to_vec();
        self.workspaces_snapshot = workspaces.to_vec();
        self.tags_snapshot = tags.to_vec();
        self.save_status = SaveStatusLine::new(save_error);
        let highlighted_issue_link_task_id = self.pending_issue_link_highlight.take();
        self.root.child_mut().replace(
            detail_form(
                task,
                catalogs,
                Rc::clone(&self.patches),
                Rc::clone(&self.checklist_highlighted_id),
                highlighted_issue_link_task_id.as_deref(),
                self.save_status.clone(),
                Rc::clone(&self.description_max_rows),
            ),
            ctx,
        );
    }

    pub(crate) fn set_save_error(&self, save_error: Option<&str>) {
        self.save_status.set_error(save_error);
    }

    pub(crate) fn queue_issue_link_highlight(&mut self, task_id: String) {
        self.pending_issue_link_highlight = Some(task_id);
    }

    pub(crate) fn set_layout_limits(&mut self, narrow: bool, height: u16) {
        let max_rows = if narrow {
            usize::from(TASK_DESCRIPTION_NARROW_MAX_CONTENT_ROWS)
        } else {
            usize::from(height.saturating_mul(TASK_DESCRIPTION_DESKTOP_MAX_PERCENT) / 100)
                .saturating_sub(2)
                .max(usize::from(TASK_DESCRIPTION_DESKTOP_MIN_CONTENT_ROWS))
        };
        self.description_max_rows.set(Some(max_rows));
        self.root.child_mut().set_description_item(if narrow {
            FlexItem::fill_min(
                1,
                TASK_DESCRIPTION_DESKTOP_MIN_CONTENT_ROWS.saturating_add(2),
            )
        } else {
            FlexItem::content().shrink(1)
        });
    }
}

struct TaskDetailFormBody {
    form: ChildSlot<Flex<AppMsg>, AppMsg>,
}

impl TaskDetailFormBody {
    fn new(form: Flex<AppMsg>) -> Self {
        Self {
            form: ChildSlot::new(ChildKey::new("form"), form),
        }
    }

    fn replace(&mut self, form: Flex<AppMsg>, ctx: &mut EventCtx<AppMsg>) {
        self.form.replace(form, ctx);
    }

    fn set_description_item(&mut self, item: FlexItem) {
        self.form.child_mut().set_item("description", item);
    }
}

impl TuiNode<AppMsg> for TaskDetailFormBody {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.form.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.form.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.form.render(frame, area, ctx);
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        self.form.dispatch_event(route, event, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        self.form.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.form.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.form.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.form.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.form.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.form.destroy(ctx);
    }
}

impl TuiNode<AppMsg> for TaskDetailForm {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.root.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        if self.description_max_rows.get().is_none() {
            self.set_layout_limits(
                area.width < crate::ui::responsive_split::MASTER_DETAIL_NARROW_BREAKPOINT,
                area.height,
            );
        }
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

    fn focus(&mut self, target: Option<&FocusId>, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        self.root.focus(target, focused, ctx);
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
