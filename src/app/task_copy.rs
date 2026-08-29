use std::collections::HashMap;

use crate::domain::{Task, Workspace, task_display_id};

#[derive(Clone, Default)]
pub(super) struct TaskCopyContext {
    workspaces: HashMap<String, Workspace>,
}

impl TaskCopyContext {
    pub(super) fn new(workspaces: &[Workspace]) -> Self {
        Self {
            workspaces: workspaces
                .iter()
                .cloned()
                .map(|workspace| (workspace.id.clone(), workspace))
                .collect(),
        }
    }

    pub(super) fn reference(&self, task: &Task) -> String {
        format!("Tuido {}", self.entry(task))
    }

    pub(super) fn references(&self, tasks: &[Task]) -> String {
        format!(
            "Tuido {}",
            tasks
                .iter()
                .map(|task| self.entry(task))
                .collect::<Vec<_>>()
                .join("; ")
        )
    }

    pub(super) fn display_id(&self, task: &Task) -> String {
        let workspace = task
            .workspace_id
            .as_deref()
            .and_then(|workspace_id| self.workspaces.get(workspace_id));
        task_display_id(task, workspace)
    }

    fn entry(&self, task: &Task) -> String {
        format!("{} {}", self.display_id(task), Self::quoted_title(task))
    }

    pub(super) fn quoted_title(task: &Task) -> String {
        let title = task.title.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{title}\"")
    }
}
