mod common;
pub(crate) mod people;
pub(crate) mod projects;
pub(crate) mod tags;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagementDialogKind {
    People,
    Projects,
    Tags,
}

impl ManagementDialogKind {
    pub(crate) fn singular(self) -> &'static str {
        match self {
            Self::People => "person",
            Self::Projects => "project",
            Self::Tags => "tag",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{AppMsg, tests::test_context},
        domain::{Person, Project, Tag, WorkspaceSnapshot},
    };
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use tuicore::{LayoutCtx, RenderCtx, TuiNode};

    #[test]
    fn narrow_management_workspaces_render_table_detail_and_controls() {
        let person = Person::new("person-1".into(), "Ada".into(), "ada@example.com".into());
        let mut project = Project::new(
            "project-1".into(),
            "CORE".into(),
            "Core".into(),
            "Platform".into(),
        );
        project.lead_person_id = Some(person.id.clone());
        let (_runtime, context, _store) = test_context(WorkspaceSnapshot {
            tasks: vec![],
            people: vec![person],
            projects: vec![project],
            tags: vec![Tag::new("tag-1".into(), "api".into())],
        });
        let cases: Vec<(Box<dyn TuiNode<AppMsg>>, &[&str])> = vec![
            (
                Box::new(people::dialog(context.clone())),
                &["Ada", "Email", "Active", "New"],
            ),
            (
                Box::new(projects::dialog(context.clone())),
                &["CORE", "Description", "Lead", "New"],
            ),
            (Box::new(tags::dialog(context)), &["api", "Label", "New"]),
        ];
        let area = Rect::new(0, 0, 80, 30);

        for (mut workspace, expected) in cases {
            workspace.layout(area, &mut LayoutCtx::new());
            let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
            terminal
                .draw(|frame| workspace.render(frame, area, &mut RenderCtx::new()))
                .unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();

            for label in expected {
                assert!(rendered.contains(label), "missing {label}: {rendered}");
            }
        }
    }
}
