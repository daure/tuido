use std::collections::HashSet;

use time::{Date, format_description::well_known::Iso8601};

use super::{
    ServiceError, ServiceResult, TaskPatch, TaskUpdate, TaskView, WorkspaceFilter, WorkspaceView,
};

pub(super) fn validate_workspace_filter(
    filter: &WorkspaceFilter,
    workspace: &WorkspaceView,
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
    for (field, value) in [
        ("due_before", filter.due_before.as_deref()),
        ("due_after", filter.due_after.as_deref()),
    ] {
        if let Some(value) = value
            && Date::parse(value, &Iso8601::DATE).is_err()
        {
            return Err(ServiceError::Invalid(format!(
                "{field} must be an ISO date formatted YYYY-MM-DD"
            )));
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
        "project_ids",
        &filter.project_ids,
        workspace
            .projects
            .iter()
            .map(|project| project.value.id.as_str()),
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
        || !relation_matches(&filter.project_ids, &task.project_ids)
        || !relation_matches(&filter.tag_ids, &task.tag_ids)
    {
        return false;
    }
    if filter
        .due_before
        .as_ref()
        .is_some_and(|bound| task.due_date.as_ref().is_none_or(|due| due >= bound))
        || filter
            .due_after
            .as_ref()
            .is_some_and(|bound| task.due_date.as_ref().is_none_or(|due| due <= bound))
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
            && !task.detail.to_lowercase().contains(&query)
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
    validate_task_temporal_fields(
        state,
        value.start_date.as_deref(),
        value.due_date.as_deref(),
        value.snoozed_until.is_some(),
    )
}

pub(super) fn validate_task_temporal_fields(
    state: crate::domain::TaskState,
    start_date: Option<&str>,
    due_date: Option<&str>,
    has_snoozed_until: bool,
) -> ServiceResult<()> {
    for (field, value) in [("start_date", start_date), ("due_date", due_date)] {
        if let Some(value) = value
            && Date::parse(value, &Iso8601::DATE).is_err()
        {
            return Err(ServiceError::Invalid(format!(
                "{field} must be an ISO date formatted YYYY-MM-DD"
            )));
        }
    }
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
        TaskPatch::StartDate(value) => validate_task_temporal_fields(
            crate::domain::TaskState::Todo,
            value.as_deref(),
            None,
            false,
        ),
        TaskPatch::EndDate(value) => validate_task_temporal_fields(
            crate::domain::TaskState::Todo,
            None,
            value.as_deref(),
            false,
        ),
        TaskPatch::State(crate::domain::TaskState::Snoozed) => Err(ServiceError::Invalid(
            "use snooze action to set snoozed state and snoozed_until together".into(),
        )),
        _ => Ok(()),
    }
}

pub(super) fn validate_required(field: &str, value: &str) -> ServiceResult<()> {
    if value.trim().is_empty() {
        Err(ServiceError::Invalid(format!("{field} is required")))
    } else {
        Ok(())
    }
}
