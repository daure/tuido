use tuicore::{Dropdown, DropdownCommitMode, DropdownSearchMode};

use crate::domain::Person;

#[derive(Debug, Clone)]
pub(super) struct Choice {
    id: String,
    label: String,
}

pub(super) fn dropdown_single(
    label: &'static str,
    rows: Vec<Choice>,
    selected: &str,
    on_select: impl Fn(String) + 'static,
) -> Dropdown<Choice, String> {
    Dropdown::single(rows, |row| row.id.clone(), |row| row.label.clone())
        .label(label)
        .selected_one(selected.to_string())
        .search_mode(DropdownSearchMode::Contains)
        .commit_mode(DropdownCommitMode::Explicit)
        .on_select(move |ids| {
            if let Some(id) = ids.into_iter().next() {
                on_select(id);
            }
        })
}

pub(super) fn dropdown_single_optional(
    label: &'static str,
    mut rows: Vec<Choice>,
    selected: Option<&str>,
    on_select: impl Fn(Option<String>) + 'static,
) -> Dropdown<Choice, String> {
    rows.insert(
        0,
        Choice {
            id: String::new(),
            label: "None".to_string(),
        },
    );
    Dropdown::single(rows, |row| row.id.clone(), |row| row.label.clone())
        .label(label)
        .selected_one(selected.unwrap_or_default().to_string())
        .search_mode(DropdownSearchMode::Contains)
        .commit_mode(DropdownCommitMode::Explicit)
        .on_select(move |ids| {
            if let Some(id) = ids.into_iter().next() {
                on_select((!id.is_empty()).then_some(id));
            }
        })
}

pub(super) fn active_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "true".to_string(),
            label: "Active".to_string(),
        },
        Choice {
            id: "false".to_string(),
            label: "Inactive".to_string(),
        },
    ]
}

pub(super) fn person_choices(people: &[Person]) -> Vec<Choice> {
    people
        .iter()
        .map(|person| Choice {
            id: person.id.clone(),
            label: person.name.clone(),
        })
        .collect()
}
