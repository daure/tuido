use tuido::service::{
    PersonInput, ProjectInput, ServiceError, TagInput, TaskCreate, TaskUpdate, TuidoService,
};
use uuid::Uuid;

fn task_input(
    title: String,
    people_ids: Vec<String>,
    project_ids: Vec<String>,
    tag_ids: Vec<String>,
) -> TaskCreate {
    TaskCreate {
        title,
        detail: "postgres contract".into(),
        size: "small".into(),
        state: "todo".into(),
        priority: "high".into(),
        start_date: None,
        due_date: None,
        snoozed_until: None,
        people_ids,
        project_ids,
        tag_ids,
    }
}

#[tokio::test]
#[ignore = "requires TUIDO_TEST_POSTGRES_URL and explicit Postgres compatibility run"]
async fn postgres_migrations_crud_concurrency_and_transactions_hold() {
    let url = std::env::var("TUIDO_TEST_POSTGRES_URL")
        .expect("set TUIDO_TEST_POSTGRES_URL before running ignored Postgres compatibility test");
    let service = TuidoService::connect_url(&url).await.unwrap();
    let suffix = Uuid::new_v4().simple().to_string();

    let person = service
        .create_person(PersonInput {
            name: format!("Ada {suffix}"),
            email: format!("ada-{suffix}@example.com"),
            active: true,
        })
        .await
        .unwrap();
    let project = service
        .create_project(ProjectInput {
            key: format!("P{}", &suffix[..8]),
            name: format!("Project {suffix}"),
            description: String::new(),
            lead_person_id: Some(person.value.id.clone()),
        })
        .await
        .unwrap();
    let tag = service
        .create_tag(TagInput {
            label: format!("tag-{suffix}"),
        })
        .await
        .unwrap();
    let task = service
        .create_task(task_input(
            format!("Task {suffix}"),
            vec![person.value.id.clone()],
            vec![project.value.id.clone()],
            vec![tag.value.id.clone()],
        ))
        .await
        .unwrap();

    let updated = service
        .update_task(TaskUpdate {
            id: task.value.id.clone(),
            expected_revision: task.revision,
            title: format!("Updated {suffix}"),
            state: "in_progress".into(),
            size: "medium".into(),
            priority: "medium".into(),
            start_date: None,
            due_date: None,
            snoozed_until: None,
            people_ids: vec![person.value.id.clone()],
            project_ids: vec![project.value.id.clone()],
            tag_ids: vec![tag.value.id.clone()],
            detail: "updated".into(),
        })
        .await
        .unwrap();
    let stale = service
        .update_task(TaskUpdate {
            id: task.value.id.clone(),
            expected_revision: task.revision,
            title: "stale".into(),
            state: "todo".into(),
            size: "small".into(),
            priority: "low".into(),
            start_date: None,
            due_date: None,
            snoozed_until: None,
            people_ids: Vec::new(),
            project_ids: Vec::new(),
            tag_ids: Vec::new(),
            detail: String::new(),
        })
        .await
        .unwrap_err();
    assert!(matches!(stale, ServiceError::Conflict { .. }));

    let before_failed_relation = service.workspace().await.unwrap();
    let relation_error = service
        .update_task(TaskUpdate {
            id: task.value.id.clone(),
            expected_revision: updated.revision,
            title: updated.value.title.clone(),
            state: updated.value.state.clone(),
            size: updated.value.size.clone(),
            priority: updated.value.priority.clone(),
            start_date: None,
            due_date: None,
            snoozed_until: None,
            people_ids: vec![format!("missing-{suffix}")],
            project_ids: Vec::new(),
            tag_ids: Vec::new(),
            detail: updated.value.detail.clone(),
        })
        .await;
    assert!(relation_error.is_err());
    let after_failed_relation = service.workspace().await.unwrap();
    assert_eq!(
        after_failed_relation.revision,
        before_failed_relation.revision
    );
    assert_eq!(
        service.get_task(&task.value.id).await.unwrap().revision,
        updated.revision
    );

    service
        .delete_person(&person.value.id, person.revision)
        .await
        .unwrap();
    let cascaded = service.get_task(&task.value.id).await.unwrap();
    assert_eq!(cascaded.revision, updated.revision + 1);
    assert!(cascaded.value.people_ids.is_empty());
    let workspace = service.workspace().await.unwrap();
    let workspace_task = workspace
        .tasks
        .iter()
        .find(|candidate| candidate.value.id == task.value.id)
        .unwrap();
    assert_eq!(workspace_task.revision, cascaded.revision);
    assert_eq!(workspace_task.value.title, cascaded.value.title);

    service
        .delete_task(&task.value.id, cascaded.revision)
        .await
        .unwrap();
    service
        .delete_project(&project.value.id, project.revision + 1)
        .await
        .unwrap();
    service
        .delete_tag(&tag.value.id, tag.revision)
        .await
        .unwrap();
}
