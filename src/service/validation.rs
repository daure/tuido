use std::collections::HashSet;

use super::{
    ServiceError, ServiceResult, TaskPatch, TaskUpdate, TaskView, WorkspaceFilter, WorkspaceGraph,
};

pub(super) fn validate_workspace_filter(
    filter: &WorkspaceFilter,
    workspace: &WorkspaceGraph,
) -> ServiceResult<()> {
    for state in &filter.states {
        let state = crate::domain::TaskState::parse(state)
            .ok_or_else(|| ServiceError::Invalid(format!("unknown task state: {state}")))?;
        if !filter.include_resolved
            && matches!(
                state,
                crate::domain::TaskState::Done | crate::domain::TaskState::Rejected
            )
        {
            return Err(ServiceError::Invalid(
                "resolved state filters require include_resolved=true".into(),
            ));
        }
    }
    for priority in &filter.priorities {
        if crate::domain::TaskPriority::parse(priority).is_none() {
            return Err(ServiceError::Invalid(format!(
                "unknown task priority: {priority}"
            )));
        }
    }
    for size in &filter.sizes {
        if crate::domain::TaskSize::parse(size).is_none() {
            return Err(ServiceError::Invalid(format!("unknown task size: {size}")));
        }
    }
    ensure_known_filter_ids(
        "person_ids",
        &filter.person_ids,
        workspace
            .people
            .iter()
            .map(|person| person.value.id.as_str()),
    )?;
    ensure_known_filter_ids(
        "workspace_ids",
        &filter.workspace_ids,
        workspace
            .workspaces
            .iter()
            .map(|workspace| workspace.value.id.as_str()),
    )?;
    ensure_known_filter_ids(
        "tag_ids",
        &filter.tag_ids,
        workspace.tags.iter().map(|tag| tag.value.id.as_str()),
    )
}

fn ensure_known_filter_ids<'a>(
    field: &str,
    requested: &[String],
    known: impl IntoIterator<Item = &'a str>,
) -> ServiceResult<()> {
    let known = known.into_iter().collect::<HashSet<_>>();
    if let Some(unknown) = requested.iter().find(|id| !known.contains(id.as_str())) {
        return Err(ServiceError::Invalid(format!(
            "unknown {field} value: {unknown}"
        )));
    }
    Ok(())
}

pub(super) fn task_matches_workspace_filter(task: &TaskView, filter: &WorkspaceFilter) -> bool {
    let resolved = matches!(task.state.as_str(), "done" | "rejected");
    if resolved && !filter.include_resolved {
        return false;
    }
    if !filter.states.is_empty()
        && !filter.states.iter().any(|state| {
            crate::domain::TaskState::parse(state).is_some_and(|state| state.id() == task.state)
        })
    {
        return false;
    }
    if !filter.priorities.is_empty()
        && !filter.priorities.iter().any(|priority| {
            crate::domain::TaskPriority::parse(priority)
                .is_some_and(|priority| priority.id() == task.priority)
        })
    {
        return false;
    }
    if !filter.sizes.is_empty()
        && !filter.sizes.iter().any(|size| {
            crate::domain::TaskSize::parse(size).is_some_and(|size| size.id() == task.size)
        })
    {
        return false;
    }
    if !relation_matches(&filter.person_ids, &task.people_ids)
        || !optional_relation_matches(&filter.workspace_ids, task.workspace_id.as_deref())
        || !relation_matches(&filter.tag_ids, &task.tag_ids)
    {
        return false;
    }
    if let Some(query) = filter
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
    {
        let query = query.to_lowercase();
        if !task.title.to_lowercase().contains(&query)
            && !task.description.to_lowercase().contains(&query)
        {
            return false;
        }
    }
    true
}

fn relation_matches(filter_ids: &[String], task_ids: &[String]) -> bool {
    filter_ids.is_empty()
        || filter_ids
            .iter()
            .any(|filter_id| task_ids.contains(filter_id))
}

fn optional_relation_matches(filter_ids: &[String], task_id: Option<&str>) -> bool {
    filter_ids.is_empty()
        || task_id.is_some_and(|task_id| filter_ids.iter().any(|id| id == task_id))
}

pub(super) fn validate_task_update(value: &TaskUpdate) -> ServiceResult<()> {
    if value.title.trim().is_empty() {
        return Err(ServiceError::Invalid("task title is required".into()));
    }
    if crate::domain::TaskState::parse(&value.state).is_none()
        || crate::domain::TaskSize::parse(&value.size).is_none()
        || crate::domain::TaskPriority::parse(&value.priority).is_none()
    {
        return Err(ServiceError::Invalid(
            "invalid state, size, or priority".into(),
        ));
    }
    let state = crate::domain::TaskState::parse(&value.state).expect("validated task state");
    validate_task_links(&value.links)?;
    validate_task_temporal_fields(state, value.snoozed_until.is_some())
}

pub(super) fn validate_task_temporal_fields(
    state: crate::domain::TaskState,
    has_snoozed_until: bool,
) -> ServiceResult<()> {
    match (state, has_snoozed_until) {
        (crate::domain::TaskState::Snoozed, false) => Err(ServiceError::Invalid(
            "snoozed tasks require snoozed_until".into(),
        )),
        (crate::domain::TaskState::Snoozed, true) => Ok(()),
        (_, true) => Err(ServiceError::Invalid(
            "non-snoozed tasks must not retain snoozed_until".into(),
        )),
        (_, false) => Ok(()),
    }
}

pub(super) fn validate_task_patch(patch: &TaskPatch) -> ServiceResult<()> {
    match patch {
        TaskPatch::State(crate::domain::TaskState::Snoozed) => Err(ServiceError::Invalid(
            "use snooze action to set snoozed state and snoozed_until together".into(),
        )),
        TaskPatch::Links(links) => validate_task_links(links),
        TaskPatch::Checklist(items) => validate_task_checklist(items),
        _ => Ok(()),
    }
}

pub(super) fn validate_task_checklist(items: &[crate::domain::ChecklistItem]) -> ServiceResult<()> {
    let mut ids = std::collections::HashSet::new();
    for item in items {
        validate_required("checklist item text", &item.text)?;
        if item.id.trim().is_empty() || !ids.insert(item.id.as_str()) {
            return Err(ServiceError::Invalid(
                "checklist item IDs must be non-empty and unique".into(),
            ));
        }
    }
    for item in items {
        if item
            .parent_id
            .as_ref()
            .is_some_and(|parent_id| parent_id == &item.id || !ids.contains(parent_id.as_str()))
        {
            return Err(ServiceError::Invalid(format!(
                "checklist item {} has an invalid parent",
                item.id
            )));
        }
        let mut parent_id = item.parent_id.as_deref();
        let mut visited = std::collections::HashSet::new();
        while let Some(parent) = parent_id {
            if !visited.insert(parent) {
                return Err(ServiceError::Invalid(
                    "checklist must not contain cycles".into(),
                ));
            }
            parent_id = items
                .iter()
                .find(|candidate| candidate.id == parent)
                .and_then(|candidate| candidate.parent_id.as_deref());
        }
    }
    Ok(())
}

pub(super) fn validate_task_links(links: &[String]) -> ServiceResult<()> {
    if let Some(link) = links.iter().find(|link| !crate::task_link::is_valid(link)) {
        Err(ServiceError::Invalid(format!(
            "task link `{link}` must match link pattern {}",
            crate::task_link::LINK_PATTERN
        )))
    } else {
        Ok(())
    }
}

pub(super) fn validate_required(field: &str, value: &str) -> ServiceResult<()> {
    if value.trim().is_empty() {
        Err(ServiceError::Invalid(format!("{field} is required")))
    } else {
        Ok(())
    }
}

pub(super) fn validate_workspace_key(value: &str) -> ServiceResult<()> {
    if crate::domain::Workspace::is_valid_key(value) {
        Ok(())
    } else {
        Err(ServiceError::Invalid(
            "workspace key must be 2-5 characters without spaces".into(),
        ))
    }
}
