use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, EventCtx, EventOutcome, EventRoute, Flex, FlexItem, FocusCtx, FocusTarget,
    LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult,
    TuiEvent, TuiNode,
};

use crate::{
    app::{AppMsg, task_detail::detail_form},
    domain::{Person, Project, Tag, Task, TaskPatch, TaskState},
    ui::save_status::SaveStatusLine,
};

pub(crate) type PatchSink = Rc<RefCell<Vec<TaskPatch>>>;

pub(crate) struct TaskDetailForm {
    root: Flex<AppMsg>,
    pub(crate) task_id: Option<String>,
    pub(crate) task_state: Option<TaskState>,
    pub(crate) task_snapshot: Option<Task>,
    pub(crate) people_snapshot: Vec<Person>,
    pub(crate) projects_snapshot: Vec<Project>,
    pub(crate) tags_snapshot: Vec<Tag>,
    pub(crate) patches: PatchSink,
    save_status: SaveStatusLine,
}

impl TaskDetailForm {
    pub(crate) fn new(
        task: Option<&Task>,
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
            task_snapshot: task.cloned(),
            people_snapshot: people.to_vec(),
            projects_snapshot: projects.to_vec(),
            tags_snapshot: tags.to_vec(),
            patches,
            save_status,
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
        people: &[Person],
        projects: &[Project],
        tags: &[Tag],
        save_error: Option<&str>,
        ctx: &mut EventCtx<AppMsg>,
    ) {
        self.patches = Rc::new(RefCell::new(Vec::new()));
        self.task_id = task.map(|task| task.id.clone());
        self.task_state = task.map(|task| task.state);
        self.task_snapshot = task.cloned();
        self.people_snapshot = people.to_vec();
        self.projects_snapshot = projects.to_vec();
        self.tags_snapshot = tags.to_vec();
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

    pub(crate) fn set_save_error(&self, save_error: Option<&str>) {
        self.save_status.set_error(save_error);
    }
}

impl TuiNode<AppMsg> for TaskDetailForm {
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
