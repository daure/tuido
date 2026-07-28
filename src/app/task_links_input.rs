use std::{rc::Rc, time::Duration};

use ratatui::{Frame, layout::Constraint, layout::Rect};
use tuicore::{
    AnimationSettings, Column, DataViewTypedEvent, EventCtx, EventOutcome, EventRoute, FocusCtx,
    FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    ListControl, ListControlEvent, ListControlField, ListControlKeyBindings, Notification,
    RenderCtx, TickResult, TuiEvent, TuiNode,
};
use uuid::Uuid;

use super::{AppMsg, PatchSink};
use crate::{app_keymap::keys, domain::Task, domain::TaskPatch, task_link};

#[derive(Clone)]
struct TaskLinkRow {
    id: String,
    url: String,
}

pub(super) struct TaskLinksInput {
    input: ListControl<TaskLinkRow, String, AppMsg>,
    committed: Vec<TaskLinkRow>,
    patch_sink: PatchSink,
    open_link: Rc<dyn Fn(&str) -> Result<(), String>>,
}

impl TaskLinksInput {
    pub(super) fn new(task: &Task, patch_sink: PatchSink) -> Self {
        Self::with_opener(task, patch_sink, |url| {
            webbrowser::open(url).map_err(|error| error.to_string())
        })
    }

    pub(super) fn with_opener(
        task: &Task,
        patch_sink: PatchSink,
        open_link: impl Fn(&str) -> Result<(), String> + 'static,
    ) -> Self {
        let mut rows = task
            .links
            .iter()
            .map(|url| TaskLinkRow {
                id: Uuid::new_v4().to_string(),
                url: url.clone(),
            })
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| left.url.cmp(&right.url));
        let input = ListControl::new_fields(
            rows.clone(),
            |row: &TaskLinkRow| row.id.clone(),
            [ListControlField::text("URL")],
            |values, _| TaskLinkRow {
                id: Uuid::new_v4().to_string(),
                url: values.into_iter().next().unwrap_or_default(),
            },
        )
        .editable(
            |row| vec![row.url.clone()],
            |row, values| row.url = values.into_iter().next().unwrap_or_default(),
        )
        .columns([
            Column::text("icon", "", Constraint::Length(1), |row: &TaskLinkRow| {
                task_link::icon(&row.url).to_string()
            }),
            Column::text("url", "", Constraint::Fill(1), |row: &TaskLinkRow| {
                row.url.clone()
            }),
        ])
        .copy_with(|row| row.url.clone())
        .headers(false)
        .title("Links")
        .hotkey(keys::TASK_LINKS_FIELD.hotkey())
        .keybindings(ListControlKeyBindings::default().remove([keys::TASK_LINK_DELETE.key_spec()]))
        .max_rows(usize::MAX);
        Self {
            input,
            committed: rows,
            patch_sink,
            open_link: Rc::new(open_link),
        }
    }

    fn sync_events(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let activated = self
            .input
            .data_view_mut()
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                DataViewTypedEvent::Activated { row_id } => Some(row_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        for row_id in activated {
            let Some(row) = self.input.items().iter().find(|row| row.id == row_id) else {
                continue;
            };
            let target = task_link::browser_target(&row.url);
            if let Err(error) = (self.open_link)(&target) {
                ctx.notify(Notification::error(
                    "Could not open link",
                    format!("{}: {error}", row.url),
                ));
            }
        }

        let events = self.input.take_events();
        if !events.iter().any(|event| {
            matches!(
                event,
                ListControlEvent::Added { .. }
                    | ListControlEvent::Edited { .. }
                    | ListControlEvent::Removed { .. }
            )
        }) {
            return;
        }
        if let Some(invalid) = self
            .input
            .items()
            .iter()
            .find(|row| !task_link::is_valid(&row.url))
        {
            ctx.notify(Notification::error(
                "Invalid link",
                format!("{} must match {}", invalid.url, task_link::LINK_PATTERN),
            ));
            self.input.data_view_mut().set_rows(self.committed.clone());
        } else {
            self.committed = self.input.items().to_vec();
            self.committed
                .sort_by(|left, right| left.url.cmp(&right.url));
            self.committed.dedup_by(|left, right| left.url == right.url);
            self.input.data_view_mut().set_rows(self.committed.clone());
            self.patch_sink.borrow_mut().push(TaskPatch::Links(
                self.committed.iter().map(|row| row.url.clone()).collect(),
            ));
        }
        ctx.request_layout();
        ctx.request_redraw();
    }
}

impl TuiNode<AppMsg> for TaskLinksInput {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.input.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.input.layout(area, ctx)
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut RenderCtx<'a>) {
        self.input.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        let outcome = self.input.event(event, ctx);
        self.sync_events(ctx);
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let outcome = self.input.dispatch_event(route, event, ctx);
        self.sync_events(ctx);
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        self.input.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.input.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.input.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.input.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.input.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.input.destroy(ctx);
    }
}
