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

type OpenLink = Rc<dyn Fn(&str, LinkOpenMode) -> Result<(), String>>;
const TITLE_REVEAL_DURATION: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LinkOpenMode {
    Foreground,
    Background,
}

#[derive(Clone)]
struct TaskLinkRow {
    id: String,
    url: String,
    title: Option<String>,
    fetching: bool,
    show_title: bool,
}

pub(super) struct TaskLinksInput {
    input: ListControl<TaskLinkRow, String, AppMsg>,
    committed: Vec<TaskLinkRow>,
    patch_sink: PatchSink,
    open_link: Option<OpenLink>,
    task_id: String,
    title_reveal: Option<(String, Duration)>,
}

impl TaskLinksInput {
    pub(super) fn new(task: &Task, patch_sink: PatchSink) -> Self {
        Self::with_optional_opener(task, patch_sink, None)
    }

    #[cfg(test)]
    pub(super) fn with_opener(
        task: &Task,
        patch_sink: PatchSink,
        open_link: impl Fn(&str, LinkOpenMode) -> Result<(), String> + 'static,
    ) -> Self {
        Self::with_optional_opener(task, patch_sink, Some(Rc::new(open_link)))
    }

    fn with_optional_opener(
        task: &Task,
        patch_sink: PatchSink,
        open_link: Option<OpenLink>,
    ) -> Self {
        let mut rows = task
            .links
            .iter()
            .map(|link| TaskLinkRow {
                id: Uuid::new_v4().to_string(),
                url: link.url.clone(),
                title: link.title.clone(),
                fetching: false,
                show_title: false,
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
                title: None,
                fetching: true,
                show_title: false,
            },
        )
        .editable(
            |row| vec![row.url.clone()],
            |row, values| {
                let url = values.into_iter().next().unwrap_or_default();
                if row.url != url {
                    row.url = url;
                    row.title = None;
                    row.fetching = true;
                    row.show_title = false;
                }
            },
        )
        .columns([
            Column::text("icon", "", Constraint::Length(1), |row: &TaskLinkRow| {
                task_link::icon(&row.url).to_string()
            }),
            Column::text("url", "", Constraint::Fill(1), |row: &TaskLinkRow| {
                if row.show_title {
                    row.title.clone().unwrap_or_else(|| row.url.clone())
                } else {
                    row.url.clone()
                }
            }),
        ])
        .copy_with(|row| row.url.clone())
        .headers(false)
        .title("URL links")
        .hotkey(keys::TASK_URL_LINKS_FIELD.hotkey())
        .empty_message("No URL links added")
        .keybindings(ListControlKeyBindings::default().remove([keys::TASK_LINK_DELETE.key_spec()]))
        .max_rows(usize::MAX);
        Self {
            input,
            committed: rows,
            patch_sink,
            open_link,
            task_id: task.id.clone(),
            title_reveal: None,
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
            self.open_row(&row_id, LinkOpenMode::Foreground, ctx);
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
        self.title_reveal = None;
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
            let fetches = self
                .input
                .items()
                .iter()
                .filter(|row| {
                    row.fetching
                        && !self
                            .committed
                            .iter()
                            .any(|committed| committed.url == row.url && committed.fetching)
                })
                .map(|row| row.url.clone())
                .collect::<Vec<_>>();
            self.committed = self.input.items().to_vec();
            self.committed
                .sort_by(|left, right| left.url.cmp(&right.url));
            self.committed.dedup_by(|left, right| left.url == right.url);
            self.input.data_view_mut().set_rows(self.committed.clone());
            self.patch_sink.borrow_mut().push(TaskPatch::Links(
                self.committed.iter().map(|row| row.url.clone()).collect(),
            ));
            for url in fetches {
                ctx.emit(AppMsg::FetchTaskLinkTitle {
                    task_id: self.task_id.clone(),
                    url,
                });
            }
        }
        ctx.request_layout();
        ctx.request_redraw();
    }

    fn open_highlighted(&self, mode: LinkOpenMode, ctx: &mut EventCtx<AppMsg>) -> bool {
        if self.input.is_editing() || self.input.is_adding() {
            return false;
        }
        let Some(row_id) = self.input.data_view().highlighted_id() else {
            return false;
        };
        self.open_row(&row_id, mode, ctx);
        true
    }

    fn reveal_highlighted_title(&mut self, ctx: &mut EventCtx<AppMsg>) -> bool {
        if self.input.is_editing() || self.input.is_adding() {
            return false;
        }
        let Some(row_id) = self.input.data_view().highlighted_id() else {
            return false;
        };
        if !self
            .input
            .items()
            .iter()
            .any(|row| row.id == row_id && row.title.is_some())
        {
            return false;
        }
        if let Some((previous_row_id, _)) = self.title_reveal.take()
            && previous_row_id != row_id
        {
            self.input
                .data_view_mut()
                .update_row(&previous_row_id, |row| {
                    row.show_title = false;
                });
        }
        self.input.data_view_mut().update_row(&row_id, |row| {
            row.show_title = true;
        });
        self.title_reveal = Some((row_id, Duration::ZERO));
        ctx.request_tick();
        ctx.request_redraw();
        true
    }

    fn open_row(&self, row_id: &str, mode: LinkOpenMode, ctx: &mut EventCtx<AppMsg>) {
        let Some(row) = self.input.items().iter().find(|row| row.id == row_id) else {
            return;
        };
        let target = task_link::browser_target(&row.url);
        if let Some(open_link) = &self.open_link
            && let Err(error) = open_link(&target, mode)
        {
            ctx.notify(Notification::error(
                "Could not open link",
                format!("{}: {error}", row.url),
            ));
        } else if self.open_link.is_none() {
            ctx.emit(AppMsg::OpenTaskLink {
                url: target,
                background: mode == LinkOpenMode::Background,
            });
        }
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
        if keys::TASK_LINK_TOGGLE_TITLE.matches(event) && self.reveal_highlighted_title(ctx) {
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        if keys::TASK_LINK_OPEN_BACKGROUND.matches(event)
            && self.open_highlighted(LinkOpenMode::Background, ctx)
        {
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
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
        if keys::TASK_LINK_TOGGLE_TITLE.matches(event) && self.reveal_highlighted_title(ctx) {
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        if keys::TASK_LINK_OPEN_BACKGROUND.matches(event)
            && self.open_highlighted(LinkOpenMode::Background, ctx)
        {
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        let outcome = self.input.dispatch_event(route, event, ctx);
        self.sync_events(ctx);
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        self.input.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let input_tick = self.input.tick(dt, settings);
        let Some((row_id, elapsed)) = &mut self.title_reveal else {
            return input_tick;
        };
        *elapsed += dt;
        if *elapsed < TITLE_REVEAL_DURATION {
            return input_tick.merge(TickResult {
                changed: false,
                layout: false,
                active: true,
                next_tick: Some(TITLE_REVEAL_DURATION - *elapsed),
            });
        }
        let row_id = row_id.clone();
        self.title_reveal = None;
        self.input.data_view_mut().update_row(&row_id, |row| {
            row.show_title = false;
        });
        input_tick.merge(TickResult {
            changed: true,
            layout: false,
            active: false,
            next_tick: None,
        })
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
