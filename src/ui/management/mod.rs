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
