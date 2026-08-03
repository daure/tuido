use tuido::service::{
    ChecklistItemInput, ServiceError, TaskCreate, TaskUpdate, TuidoService, WorkspaceFilter,
};

#[tokio::test]
async fn external_client_can_use_public_service_dtos() {
    let path =
        std::env::temp_dir().join(format!("tuido-public-api-{}.sqlite", uuid::Uuid::new_v4()));
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let service = TuidoService::connect_url(&url).await.unwrap();
    let created = service
        .create_task(TaskCreate {
            title: "External API".into(),
            description: String::new(),
            size: "small".into(),
            state: "todo".into(),
            priority: "medium".into(),
            snoozed_until: None,
            people_ids: Vec::new(),
            workspace_id: None,
            tag_ids: Vec::new(),
            links: vec!["https://example.com/task".into()],
        })
        .await
        .unwrap();
    let updated = service
        .update_task(TaskUpdate {
            id: created.value.id.clone(),
            expected_revision: created.revision,
            title: "Updated externally".into(),
            state: "in_progress".into(),
            size: "medium".into(),
            priority: "high".into(),
            snoozed_until: None,
            people_ids: Vec::new(),
            workspace_id: None,
            tag_ids: Vec::new(),
            links: vec!["file:///tmp/task.txt".into()],
            relations: Vec::new(),
            description: "public DTO mutation".into(),
        })
        .await
        .unwrap();
    let tagged = service
        .set_task_tags_by_label(
            created.value.id.clone(),
            updated.revision,
            vec![" public ".into(), "public".into()],
        )
        .await
        .unwrap();
    let checklist = service
        .set_task_checklist(
            created.value.id.clone(),
            tagged.revision,
            vec![ChecklistItemInput {
                id: None,
                text: "Ship it".into(),
                checked: false,
                children: vec![ChecklistItemInput {
                    id: None,
                    text: "Run tests".into(),
                    checked: true,
                    children: Vec::new(),
                }],
            }],
        )
        .await
        .unwrap();
    let workspace = service
        .filtered_workspace(WorkspaceFilter::default())
        .await
        .unwrap();

    assert_eq!(updated.value.state, "in_progress");
    assert_eq!(updated.value.links, vec!["file:///tmp/task.txt"]);
    assert_eq!(tagged.value.tag_ids.len(), 1);
    assert_eq!(checklist.value.checklist[0].text, "Ship it");
    assert!(checklist.value.checklist[0].children[0].checked);
    assert_eq!(workspace.tasks[0].value.id, created.value.id);

    drop(service);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn public_service_accepts_www_links_and_rejects_other_protocol_free_links() {
    let path = std::env::temp_dir().join(format!(
        "tuido-public-link-validation-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let service = TuidoService::connect_url(&url).await.unwrap();

    let result = service
        .create_task(TaskCreate {
            title: "Invalid link".into(),
            description: String::new(),
            size: "small".into(),
            state: "todo".into(),
            priority: "medium".into(),
            snoozed_until: None,
            people_ids: Vec::new(),
            workspace_id: None,
            tag_ids: Vec::new(),
            links: vec!["example.com/task".into()],
        })
        .await;

    assert!(matches!(result, Err(ServiceError::Invalid(_))));
    assert!(service.workspace().await.unwrap().tasks.is_empty());

    let created = service
        .create_task(TaskCreate {
            title: "WWW link".into(),
            description: String::new(),
            size: "small".into(),
            state: "todo".into(),
            priority: "medium".into(),
            snoozed_until: None,
            people_ids: Vec::new(),
            workspace_id: None,
            tag_ids: Vec::new(),
            links: vec!["www.google.com/search?q=tuido".into()],
        })
        .await
        .unwrap();
    assert_eq!(created.value.links, vec!["www.google.com/search?q=tuido"]);

    drop(service);
    let _ = std::fs::remove_file(path);
}
