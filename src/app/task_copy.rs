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
        let title = task.title.replace('\\', "\\\\").replace('"', "\\\"");
        format!("Tuido {} \"{title}\"", self.display_id(task))
    }

    pub(super) fn display_id(&self, task: &Task) -> String {
        let workspace = task
            .workspace_id
            .as_deref()
            .and_then(|workspace_id| self.workspaces.get(workspace_id));
        task_display_id(task, workspace)
    }
}
