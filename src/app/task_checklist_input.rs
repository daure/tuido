use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, CheckState, Checklist, EventCtx, EventOutcome, EventRoute, FocusCtx,
    FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    ListControl, ListControlEvent, RenderCtx, TickResult, TreeAdapter, TuiEvent, TuiNode,
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
    highlighted_id: Rc<RefCell<Option<String>>>,
}

impl TaskChecklistInput {
    pub(super) fn new(
        task: &Task,
        patch_sink: PatchSink,
        highlighted_id: Rc<RefCell<Option<String>>>,
    ) -> Self {
        let rows = task.checklist.clone();
        let checked = rows
            .iter()
            .filter(|candidate| {
                candidate.checked
                    && !rows
                        .iter()
                        .any(|item| item.parent_id.as_ref() == Some(&candidate.id))
            })
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
        let mut input = Checklist::from_list_control(control)
            .cascade_descendants(true)
            .checked(checked);
        if let Some(id) = highlighted_id.borrow().as_ref() {
            input.list_control_mut().data_view_mut().highlight_id(id);
        }
        *highlighted_id.borrow_mut() = input.list_control().data_view().highlighted_id();
        Self {
            input,
            committed: rows,
            patch_sink,
            highlighted_id,
        }
    }

    fn sync_events(&mut self, ctx: &mut EventCtx<AppMsg>) {
        *self.highlighted_id.borrow_mut() = self.input.list_control().data_view().highlighted_id();
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
        let mut items = self.input.items().to_vec();
        for item in &mut items {
            item.checked = self.input.check_state(&item.id) == CheckState::Checked;
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
        hint.min.height = 3;
        hint.preferred.height = hint.preferred.height.clamp(3, 8);
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
    fn checklist_shows_between_one_and_six_item_rows_plus_border() {
        let patches = Rc::new(RefCell::new(Vec::new()));
        let empty = TaskChecklistInput::new(
            &task_with_items(0),
            Rc::clone(&patches),
            Rc::new(RefCell::new(None)),
        );
        let three = TaskChecklistInput::new(
            &task_with_items(3),
            Rc::clone(&patches),
            Rc::new(RefCell::new(None)),
        );
        let many =
            TaskChecklistInput::new(&task_with_items(8), patches, Rc::new(RefCell::new(None)));

        let empty_height = empty.measure(LayoutProposal::unbounded()).preferred.height;
        let three_height = three.measure(LayoutProposal::unbounded()).preferred.height;
        let many_height = many.measure(LayoutProposal::unbounded()).preferred.height;

        assert_eq!(empty_height, 3);
        assert_eq!(three_height, 5);
        assert_eq!(many_height, 8);
    }

    #[test]
    fn checking_an_item_emits_complete_checklist_patch() {
        let patches = Rc::new(RefCell::new(Vec::new()));
        let mut input = TaskChecklistInput::new(
            &task_with_items(1),
            Rc::clone(&patches),
            Rc::new(RefCell::new(None)),
        );
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

    #[test]
    fn selection_toggle_rebuild_preserves_highlighted_item() {
        let mut task = task_with_items(3);
        let highlighted_id = Rc::new(RefCell::new(None));
        let patches = Rc::new(RefCell::new(Vec::new()));
        let mut input =
            TaskChecklistInput::new(&task, Rc::clone(&patches), Rc::clone(&highlighted_id));
        input
            .input
            .list_control_mut()
            .data_view_mut()
            .highlight_id(&"item-1".into());
        input
            .input
            .list_control_mut()
            .data_view_mut()
            .on_key(Key::Enter, Rect::new(0, 0, 40, 5));
        input.sync_events(&mut EventCtx::default());
        let TaskPatch::Checklist(checklist) = patches.borrow()[0].clone() else {
            panic!("selection toggle should emit a checklist patch");
        };
        task.checklist = checklist;

        let rebuilt =
            TaskChecklistInput::new(&task, Rc::new(RefCell::new(Vec::new())), highlighted_id);

        assert_eq!(
            rebuilt.input.list_control().data_view().highlighted_id(),
            Some("item-1".into())
        );
    }

    #[test]
    fn mixed_child_checks_make_parent_indeterminate() {
        let mut task = task_with_items(3);
        task.checklist[1].parent_id = Some("item-0".into());
        task.checklist[1].checked = true;
        task.checklist[2].parent_id = Some("item-0".into());

        let input = TaskChecklistInput::new(
            &task,
            Rc::new(RefCell::new(Vec::new())),
            Rc::new(RefCell::new(None)),
        );

        assert_eq!(
            input.input.check_state(&"item-0".into()),
            CheckState::Indeterminate
        );
    }

    #[test]
    fn dedent_and_indent_recompute_parent_check_state() {
        let mut task = task_with_items(3);
        task.checklist[1].parent_id = Some("item-0".into());
        task.checklist[1].checked = true;
        task.checklist[2].parent_id = Some("item-0".into());
        let patches = Rc::new(RefCell::new(Vec::new()));
        let mut input =
            TaskChecklistInput::new(&task, Rc::clone(&patches), Rc::new(RefCell::new(None)));
        input
            .input
            .list_control_mut()
            .data_view_mut()
            .highlight_id(&"item-2".into());

        input.event(
            &TuiEvent::Key(Key::Char('<').into()),
            &mut EventCtx::default(),
        );

        assert_eq!(
            input.input.check_state(&"item-0".into()),
            CheckState::Checked
        );
        assert_eq!(
            input.input.list_control().data_view().highlighted_id(),
            Some("item-2".into())
        );
        assert!(matches!(
            patches.borrow().last(),
            Some(TaskPatch::Checklist(items))
                if items[0].checked && items[1].checked && !items[2].checked
        ));

        input.event(
            &TuiEvent::Key(Key::Char('>').into()),
            &mut EventCtx::default(),
        );

        assert_eq!(
            input.input.check_state(&"item-0".into()),
            CheckState::Indeterminate
        );
        assert_eq!(
            input.input.list_control().data_view().highlighted_id(),
            Some("item-2".into())
        );
    }
}
