use std::{collections::HashSet, time::Duration};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, Checklist, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusTarget,
    LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, ListControl,
    ListControlEvent, RenderCtx, TickResult, TreeAdapter, TuiEvent, TuiNode,
};
use uuid::Uuid;

use super::{AppMsg, PatchSink};
use crate::{
    app_keymap::keys,
    domain::{ChecklistItem, Task, TaskPatch},
};

pub(super) struct TaskChecklistInput {
    input: Checklist<ChecklistItem, String, AppMsg>,
    committed: Vec<ChecklistItem>,
    patch_sink: PatchSink,
}

impl TaskChecklistInput {
    pub(super) fn new(task: &Task, patch_sink: PatchSink) -> Self {
        let rows = task.checklist.clone();
        let checked = rows
            .iter()
            .filter(|item| item.checked)
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        let expanded = rows
            .iter()
            .filter(|candidate| {
                rows.iter()
                    .any(|item| item.parent_id.as_ref() == Some(&candidate.id))
            })
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        let control = ListControl::list(
            rows.clone(),
            |item: &ChecklistItem| item.id.clone(),
            |item| item.text.clone(),
            |text, _| ChecklistItem {
                id: Uuid::new_v4().to_string(),
                parent_id: None,
                text,
                checked: false,
            },
        )
        .editable(
            |item| vec![item.text.clone()],
            |item, values| item.text = values.into_iter().next().unwrap_or_default(),
        )
        .tree(TreeAdapter::mutable_parent_id(
            |item: &ChecklistItem| item.parent_id.clone(),
            |item, parent_id| item.parent_id = parent_id,
        ))
        .expanded(expanded)
        .title("Checklist")
        .hotkey(keys::TASK_CHECKLIST_FIELD.hotkey())
        .empty_message("No checklist items")
        .max_rows(6);
        Self {
            input: Checklist::from_list_control(control).checked(checked),
            committed: rows,
            patch_sink,
        }
    }

    fn sync_events(&mut self, ctx: &mut EventCtx<AppMsg>) {
        let changed = self.input.take_events().into_iter().any(|event| {
            matches!(
                event,
                ListControlEvent::Added { .. }
                    | ListControlEvent::AddedChild { .. }
                    | ListControlEvent::Removed { .. }
                    | ListControlEvent::Edited { .. }
                    | ListControlEvent::Reordered { .. }
                    | ListControlEvent::TreeMoved { .. }
                    | ListControlEvent::CheckedChanged { .. }
            )
        });
        if !changed {
            return;
        }
        let checked = self.input.checked_ids().into_iter().collect::<HashSet<_>>();
        let mut items = self.input.items().to_vec();
        for item in &mut items {
            item.checked = checked.contains(&item.id);
        }
        if items != self.committed {
            self.committed = items.clone();
            self.patch_sink
                .borrow_mut()
                .push(TaskPatch::Checklist(items));
        }
        ctx.request_layout();
        ctx.request_redraw();
    }
}

impl TuiNode<AppMsg> for TaskChecklistInput {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        let mut hint = self.input.measure(proposal);
        hint.min.height = 4;
        hint.preferred.height = hint.preferred.height.clamp(4, 8);
        hint
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

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use tuicore::Key;

    use super::*;
    use crate::domain::TaskSize;

    fn task_with_items(count: usize) -> Task {
        let mut task =
            Task::quick_capture("task".into(), "Task".into(), String::new(), TaskSize::Small);
        task.checklist = (0..count)
            .map(|index| ChecklistItem {
                id: format!("item-{index}"),
                parent_id: None,
                text: format!("Item {index}"),
                checked: false,
            })
            .collect();
        task
    }

    #[test]
    fn checklist_shows_between_two_and_six_item_rows_plus_border() {
        let patches = Rc::new(RefCell::new(Vec::new()));
        let empty = TaskChecklistInput::new(&task_with_items(0), Rc::clone(&patches));
        let three = TaskChecklistInput::new(&task_with_items(3), Rc::clone(&patches));
        let many = TaskChecklistInput::new(&task_with_items(8), patches);

        let empty_height = empty.measure(LayoutProposal::unbounded()).preferred.height;
        let three_height = three.measure(LayoutProposal::unbounded()).preferred.height;
        let many_height = many.measure(LayoutProposal::unbounded()).preferred.height;

        assert_eq!(empty_height, 4);
        assert_eq!(three_height, 5);
        assert_eq!(many_height, 8);
    }

    #[test]
    fn checking_an_item_emits_complete_checklist_patch() {
        let patches = Rc::new(RefCell::new(Vec::new()));
        let mut input = TaskChecklistInput::new(&task_with_items(1), Rc::clone(&patches));
        input
            .input
            .list_control_mut()
            .data_view_mut()
            .highlight_id(&"item-0".to_string());
        input
            .input
            .list_control_mut()
            .data_view_mut()
            .on_key(Key::Enter, Rect::new(0, 0, 40, 4));

        input.sync_events(&mut EventCtx::default());

        assert!(matches!(
            patches.borrow().as_slice(),
            [TaskPatch::Checklist(items)] if items.len() == 1 && items[0].checked
        ));
    }
}
