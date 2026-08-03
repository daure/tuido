use std::{collections::HashMap, time::Duration};

use super::{AppMsg, PatchSink, task_state_icon};
use crate::domain::{Task, TaskPatch, TaskRelation, TaskRelationKind, Workspace, task_display_id};
use ratatui::{Frame, layout::Constraint, layout::Rect};
use tuicore::{
    AnimationSettings, Column, DataViewTypedEvent, EventCtx, EventOutcome, EventRoute, FocusCtx,
    FocusTarget, LayoutCtx, LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx,
    ListControl, ListControlEvent, ListControlField, Notification, RenderCtx, TickResult, TuiEvent,
    TuiNode,
};

#[derive(Clone)]
struct TaskRelationRow {
    id: String,
    relation: TaskRelation,
    target_reference: String,
    target_label: String,
}

pub(super) struct TaskRelationsInput {
    input: ListControl<TaskRelationRow, String, AppMsg>,
    committed: Vec<TaskRelationRow>,
    patch_sink: PatchSink,
    source_task_id: String,
}

impl TaskRelationsInput {
    pub(super) fn new(
        task: &Task,
        tasks: &[Task],
        workspaces: &[Workspace],
        patch_sink: PatchSink,
        highlighted_task_id: Option<&str>,
    ) -> Self {
        let task_labels = tasks
            .iter()
            .map(|task| (task.id.clone(), task_label(task, workspaces)))
            .collect::<HashMap<_, _>>();
        let task_references = tasks
            .iter()
            .map(|task| {
                let workspace = task.workspace_id.as_deref().and_then(|workspace_id| {
                    workspaces
                        .iter()
                        .find(|workspace| workspace.id == workspace_id)
                });
                (task.id.clone(), task_display_id(task, workspace))
            })
            .collect::<HashMap<_, _>>();
        let mut rows = task
            .relations
            .iter()
            .filter_map(|relation| {
                tasks
                    .iter()
                    .find(|candidate| candidate.id == relation.task_id)
                    .map(|target| TaskRelationRow {
                        id: relation.task_id.clone(),
                        relation: relation.clone(),
                        target_reference: task_references[&target.id].clone(),
                        target_label: task_labels[&target.id].clone(),
                    })
            })
            .collect::<Vec<_>>();
        sort_issue_links(&mut rows);
        let targets = task_options_by_updated(task, tasks)
            .into_iter()
            .map(|candidate| (candidate.id.clone(), task_labels[&candidate.id].clone()))
            .collect::<Vec<_>>();
        let creator_labels = task_labels.clone();
        let editor_labels = task_labels;
        let creator_references = task_references.clone();
        let editor_references = task_references;
        let committed = rows.clone();
        let mut input = ListControl::new_fields(
            rows,
            |row: &TaskRelationRow| row.id.clone(),
            [
                ListControlField::dropdown_options(
                    "Relation",
                    [
                        (
                            TaskRelationKind::Blocks.id(),
                            TaskRelationKind::Blocks.label(),
                        ),
                        (
                            TaskRelationKind::IsBlockedBy.id(),
                            TaskRelationKind::IsBlockedBy.label(),
                        ),
                        (
                            TaskRelationKind::RelatesTo.id(),
                            TaskRelationKind::RelatesTo.label(),
                        ),
                        (
                            TaskRelationKind::Duplicates.id(),
                            TaskRelationKind::Duplicates.label(),
                        ),
                        (
                            TaskRelationKind::IsDuplicatedBy.id(),
                            TaskRelationKind::IsDuplicatedBy.label(),
                        ),
                    ],
                ),
                ListControlField::dropdown_options("Task", targets).max_filtered_items(10),
            ],
            move |values, _| {
                let kind = TaskRelationKind::parse(&values[0])
                    .expect("relation dropdown only contains valid relation types");
                let task_id = values[1].clone();
                let target_label = creator_labels
                    .get(&task_id)
                    .cloned()
                    .unwrap_or_else(|| task_id.clone());
                TaskRelationRow {
                    id: task_id.clone(),
                    relation: TaskRelation { task_id, kind },
                    target_reference: creator_references
                        .get(&values[1])
                        .cloned()
                        .unwrap_or_else(|| values[1].clone()),
                    target_label,
                }
            },
        )
        .editable(
            |row| {
                vec![
                    row.relation.kind.id().to_string(),
                    row.relation.task_id.clone(),
                ]
            },
            move |row, values| {
                apply_issue_link_edit(row, &values, &editor_references, &editor_labels);
            },
        )
        .column(Column::text(
            "issue-link",
            "",
            Constraint::Fill(1),
            issue_link_label,
        ))
        .headers(false)
        .title("Issue links")
        .hotkey(crate::app_keymap::keys::TASK_ISSUE_LINKS_FIELD.hotkey())
        .empty_message("No issue links")
        .max_rows(usize::MAX);
        if let Some(row_id) = highlighted_task_id.and_then(|task_id| {
            input
                .items()
                .iter()
                .find(|row| row.relation.task_id == task_id)
                .map(|row| row.id.clone())
        }) {
            input.data_view_mut().highlight_id(&row_id);
        }
        Self {
            input,
            committed,
            patch_sink,
            source_task_id: task.id.clone(),
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
            if let Some(row) = self.input.items().iter().find(|row| row.id == row_id) {
                ctx.emit(AppMsg::NavigateToTask {
                    source_task_id: self.source_task_id.clone(),
                    target_task_id: row.relation.task_id.clone(),
                });
            }
        }

        if !self.input.take_events().iter().any(|event| {
            matches!(
                event,
                ListControlEvent::Added { .. }
                    | ListControlEvent::Edited { .. }
                    | ListControlEvent::Removed { .. }
            )
        }) {
            return;
        }
        if let Some((existing, duplicate)) = duplicate_target(self.input.items()) {
            ctx.notify(Notification::error(
                "Issue link already exists",
                format!(
                    "{} is already linked as {}; cannot also link it as {}.",
                    existing.target_label,
                    existing.relation.kind.label(),
                    duplicate.relation.kind.label()
                ),
            ));
            self.input.data_view_mut().set_rows(self.committed.clone());
            ctx.request_layout();
            ctx.request_redraw();
            return;
        }
        self.committed = self.input.items().to_vec();
        sort_issue_links(&mut self.committed);
        self.input.data_view_mut().set_rows(self.committed.clone());
        let mut relations = Vec::new();
        for row in &self.committed {
            if !relations.contains(&row.relation) {
                relations.push(row.relation.clone());
            }
        }
        self.patch_sink
            .borrow_mut()
            .push(TaskPatch::Relations(relations));
        ctx.request_layout();
        ctx.request_redraw();
    }
}

fn apply_issue_link_edit(
    row: &mut TaskRelationRow,
    values: &[String],
    task_references: &HashMap<String, String>,
    task_labels: &HashMap<String, String>,
) {
    row.relation.kind = TaskRelationKind::parse(&values[0])
        .expect("relation dropdown only contains valid relation types");
    row.relation.task_id.clone_from(&values[1]);
    row.target_reference = task_references
        .get(&row.relation.task_id)
        .cloned()
        .unwrap_or_else(|| row.relation.task_id.clone());
    row.target_label = task_labels
        .get(&row.relation.task_id)
        .cloned()
        .unwrap_or_else(|| row.relation.task_id.clone());
}

fn duplicate_target(rows: &[TaskRelationRow]) -> Option<(&TaskRelationRow, &TaskRelationRow)> {
    rows.iter().enumerate().find_map(|(index, row)| {
        rows[index + 1..]
            .iter()
            .find(|candidate| candidate.relation.task_id == row.relation.task_id)
            .map(|duplicate| (row, duplicate))
    })
}

fn issue_link_label(row: &TaskRelationRow) -> String {
    format!("{} {}", row.relation.kind.label(), row.target_label)
}

fn task_options_by_updated<'a>(task: &Task, tasks: &'a [Task]) -> Vec<&'a Task> {
    let mut options = tasks
        .iter()
        .filter(|candidate| candidate.id != task.id)
        .collect::<Vec<_>>();
    options.sort_by(|left, right| {
        let left_timestamp = left.updated_at.parse::<u128>();
        let right_timestamp = right.updated_at.parse::<u128>();
        match (left_timestamp, right_timestamp) {
            (Ok(left), Ok(right)) => right.cmp(&left),
            _ => right.updated_at.cmp(&left.updated_at),
        }
        .then_with(|| left.rank.cmp(&right.rank))
        .then_with(|| left.id.cmp(&right.id))
    });
    options
}

fn sort_issue_links(rows: &mut [TaskRelationRow]) {
    rows.sort_by(|left, right| {
        left.relation
            .kind
            .label()
            .cmp(right.relation.kind.label())
            .then_with(|| left.target_reference.cmp(&right.target_reference))
    });
}

fn task_label(task: &Task, workspaces: &[Workspace]) -> String {
    let workspace = task.workspace_id.as_deref().and_then(|workspace_id| {
        workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
    });
    let reference = task_display_id(task, workspace);
    format!(
        "{} {reference} - {}",
        task_state_icon(task.state),
        task.title
    )
}

impl TuiNode<AppMsg> for TaskRelationsInput {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{TaskSize, TaskState};
    use tuicore::{Key, KeyEvent};

    #[test]
    fn pressing_e_on_an_issue_link_starts_editing() {
        let mut first = Task::quick_capture(
            "first".into(),
            "First".into(),
            String::new(),
            TaskSize::Small,
        );
        let second = Task::quick_capture(
            "second".into(),
            "Second".into(),
            String::new(),
            TaskSize::Small,
        );
        first.relations.push(TaskRelation {
            task_id: second.id.clone(),
            kind: TaskRelationKind::Blocks,
        });
        let mut input = TaskRelationsInput::new(
            &first,
            &[first.clone(), second],
            &[],
            std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
            None,
        );
        let row_id = input.input.items()[0].id.clone();
        input.input.data_view_mut().highlight_id(&row_id);
        input.input.data_view_mut().set_focused(true);

        input.event(
            &TuiEvent::Key(KeyEvent::from(Key::Char('e'))),
            &mut EventCtx::default(),
        );

        assert!(input.input.is_editing());
    }

    #[test]
    fn activating_an_issue_link_requests_navigation_to_its_task() {
        let mut first = Task::quick_capture(
            "first".into(),
            "First".into(),
            String::new(),
            TaskSize::Small,
        );
        let second = Task::quick_capture(
            "second".into(),
            "Second".into(),
            String::new(),
            TaskSize::Small,
        );
        first.relations.push(TaskRelation {
            task_id: second.id.clone(),
            kind: TaskRelationKind::Blocks,
        });
        let mut input = TaskRelationsInput::new(
            &first,
            &[first.clone(), second],
            &[],
            std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
            None,
        );
        let row_id = input.input.items()[0].id.clone();
        input.input.data_view_mut().highlight_id(&row_id);
        input.input.data_view_mut().set_focused(true);
        let mut ctx = EventCtx::default();
        let outcome = input
            .input
            .data_view_mut()
            .event(&TuiEvent::Key(KeyEvent::from(Key::Enter)), &mut ctx);
        input.sync_events(&mut ctx);

        let effects = tuicore::DispatchEffects::from_event_ctx(outcome, ctx);
        assert!(matches!(
            effects.messages.as_slice(),
            [AppMsg::NavigateToTask { source_task_id, target_task_id }]
                if source_task_id == "first" && target_task_id == "second"
        ));
    }

    #[test]
    fn requested_issue_link_task_is_initially_highlighted() {
        let mut first = Task::quick_capture(
            "first".into(),
            "First".into(),
            String::new(),
            TaskSize::Small,
        );
        let second = Task::quick_capture(
            "second".into(),
            "Second".into(),
            String::new(),
            TaskSize::Small,
        );
        first.relations.push(TaskRelation {
            task_id: second.id.clone(),
            kind: TaskRelationKind::Blocks,
        });

        let input = TaskRelationsInput::new(
            &first,
            &[first.clone(), second],
            &[],
            std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
            Some("second"),
        );

        let highlighted_id = input.input.data_view().highlighted_id();
        let highlighted = input
            .input
            .items()
            .iter()
            .find(|row| Some(&row.id) == highlighted_id.as_ref())
            .expect("requested issue link should be highlighted");
        assert_eq!(highlighted.relation.task_id, "second");
        assert_eq!(highlighted.id, "second");
    }

    #[test]
    fn multiple_issue_links_to_the_same_task_are_invalid() {
        let rows = [
            TaskRelationRow {
                id: "first".into(),
                relation: TaskRelation {
                    task_id: "target".into(),
                    kind: TaskRelationKind::Blocks,
                },
                target_reference: "LIF-2".into(),
                target_label: "Target".into(),
            },
            TaskRelationRow {
                id: "second".into(),
                relation: TaskRelation {
                    task_id: "target".into(),
                    kind: TaskRelationKind::IsBlockedBy,
                },
                target_reference: "LIF-2".into(),
                target_label: "Target".into(),
            },
        ];

        assert!(duplicate_target(&rows).is_some());
    }

    #[test]
    fn task_options_show_state_workspace_task_number_and_title() {
        let workspace = Workspace::new(
            "workspace".into(),
            "core".into(),
            "Core".into(),
            String::new(),
        );
        let mut task = Task::quick_capture(
            "OLD-42".into(),
            "Ship it".into(),
            String::new(),
            TaskSize::Small,
        );
        task.state = TaskState::Todo;
        task.workspace_id = Some(workspace.id.clone());

        assert_eq!(task_label(&task, &[workspace]), " CORE-42 - Ship it");
    }

    #[test]
    fn editing_issue_link_target_preserves_data_view_row_id() {
        let mut row = TaskRelationRow {
            id: "workspaceed-task".into(),
            relation: TaskRelation {
                task_id: "workspaceed-task".into(),
                kind: TaskRelationKind::Blocks,
            },
            target_reference: "APP-2".into(),
            target_label: "APP-2 - Workspaceed".into(),
        };
        let references = HashMap::from([("3".into(), "3".into())]);
        let labels = HashMap::from([("3".into(), "3 - No workspace".into())]);

        apply_issue_link_edit(
            &mut row,
            &["relates_to".into(), "3".into()],
            &references,
            &labels,
        );

        assert_eq!(row.id, "workspaceed-task");
        assert_eq!(row.relation.task_id, "3");
        assert_eq!(row.relation.kind, TaskRelationKind::RelatesTo);
        assert_eq!(row.target_reference, "3");
        assert_eq!(row.target_label, "3 - No workspace");
    }

    #[test]
    fn issue_link_rows_combine_relation_and_task_into_one_column() {
        let row = TaskRelationRow {
            id: "link".into(),
            relation: TaskRelation {
                task_id: "LIF-30".into(),
                kind: TaskRelationKind::IsBlockedBy,
            },
            target_reference: "LIF-30".into(),
            target_label: " LIF-30 - From calendar".into(),
        };

        assert_eq!(
            issue_link_label(&row),
            "is blocked by  LIF-30 - From calendar"
        );
    }

    #[test]
    fn issue_links_sort_by_relation_then_ticket_id() {
        let mut rows = [
            TaskRelationRow {
                id: "third".into(),
                relation: TaskRelation {
                    task_id: "3".into(),
                    kind: TaskRelationKind::RelatesTo,
                },
                target_reference: "LIF-3".into(),
                target_label: "Third".into(),
            },
            TaskRelationRow {
                id: "second".into(),
                relation: TaskRelation {
                    task_id: "2".into(),
                    kind: TaskRelationKind::Blocks,
                },
                target_reference: "LIF-2".into(),
                target_label: "Second".into(),
            },
            TaskRelationRow {
                id: "first".into(),
                relation: TaskRelation {
                    task_id: "1".into(),
                    kind: TaskRelationKind::Blocks,
                },
                target_reference: "LIF-1".into(),
                target_label: "First".into(),
            },
        ];

        sort_issue_links(&mut rows);

        assert_eq!(
            rows.map(|row| row.id),
            [
                "first".to_string(),
                "second".to_string(),
                "third".to_string()
            ]
        );
    }

    #[test]
    fn task_options_show_all_other_tasks_most_recently_updated_first() {
        let current = Task::quick_capture(
            "current".into(),
            "Current".into(),
            String::new(),
            TaskSize::Small,
        );
        let task = |id: &str, state, updated_at: &str| {
            let mut task =
                Task::quick_capture(id.into(), id.into(), String::new(), TaskSize::Small);
            task.state = state;
            task.updated_at = updated_at.into();
            task
        };
        let tasks = [
            current.clone(),
            task("backlog", TaskState::Backlog, "100"),
            task("todo", TaskState::Todo, "600"),
            task("active", TaskState::InProgress, "500"),
            task("done", TaskState::Done, "400"),
            task("snoozed", TaskState::Snoozed, "300"),
            task("rejected", TaskState::Rejected, "200"),
        ];

        assert_eq!(
            task_options_by_updated(&current, &tasks)
                .into_iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            ["todo", "active", "done", "snoozed", "rejected", "backlog"]
        );
    }
}
