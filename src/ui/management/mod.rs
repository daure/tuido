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
