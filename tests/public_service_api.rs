use tuido::service::{
    ChecklistItemInput, ServiceError, TaskCreate, TaskRelationInput, TaskUpdate, TuidoService,
    WorkspaceFilter,
};

#[tokio::test]
async fn external_client_can_use_public_service_dtos() {
    let path =
        std::env::temp_dir().join(format!("tuido-public-api-{}.sqlite", uuid::Uuid::new_v4()));
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let service = TuidoService::connect_url(&url).await.unwrap();
    let related = service
        .create_task(TaskCreate {
            title: "Related task".into(),
            description: String::new(),
            size: "small".into(),
            state: "todo".into(),
            priority: "medium".into(),
            snoozed_until: None,
            people_ids: Vec::new(),
            workspace_id: None,
            tag_ids: Vec::new(),
            links: Vec::new(),
            checklist: Vec::new(),
            relations: Vec::new(),
            tags: Vec::new(),
        })
        .await
        .unwrap();
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
            checklist: vec![ChecklistItemInput {
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
            relations: vec![TaskRelationInput {
                task_id: related.value.id.clone(),
                relation_type: "blocks".into(),
            }],
            tags: vec![" public ".into(), "public".into()],
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
    assert_eq!(created.value.tag_ids.len(), 1);
    assert_eq!(created.value.checklist[0].text, "Ship it");
    assert_eq!(created.value.relations[0].task.id, related.value.id);
    assert_eq!(
        updated
            .value
            .links
            .iter()
            .map(|link| link.url.as_str())
            .collect::<Vec<_>>(),
        vec!["file:///tmp/task.txt"]
    );
    assert_eq!(tagged.value.tag_ids.len(), 1);
    assert_eq!(checklist.value.checklist[0].text, "Ship it");
    assert!(checklist.value.checklist[0].children[0].checked);
    assert_eq!(workspace.tasks[0].value.id, created.value.id);

    drop(service);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn create_task_rolls_back_collections_when_an_issue_link_is_invalid() {
    let path = std::env::temp_dir().join(format!(
        "tuido-public-create-rollback-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let service = TuidoService::connect_url(&url).await.unwrap();

    let result = service
        .create_task(TaskCreate {
            title: "Invalid relation".into(),
            description: String::new(),
            size: "small".into(),
            state: "todo".into(),
            priority: "medium".into(),
            snoozed_until: None,
            people_ids: Vec::new(),
            workspace_id: None,
            tag_ids: Vec::new(),
            links: vec!["https://example.com/task".into()],
            checklist: vec![ChecklistItemInput {
                id: None,
                text: "Prepare release".into(),
                checked: false,
                children: Vec::new(),
            }],
            relations: vec![TaskRelationInput {
                task_id: "missing".into(),
                relation_type: "blocks".into(),
            }],
            tags: vec!["temporary".into()],
        })
        .await;

    assert!(matches!(result, Err(ServiceError::Invalid(_))));
    let workspace = service.workspace().await.unwrap();
    assert!(workspace.tasks.is_empty());
    assert!(workspace.tags.is_empty());

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
            checklist: Vec::new(),
            relations: Vec::new(),
            tags: Vec::new(),
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
            checklist: Vec::new(),
            relations: Vec::new(),
            tags: Vec::new(),
        })
        .await
        .unwrap();
    assert_eq!(
        created
            .value
            .links
            .iter()
            .map(|link| link.url.as_str())
            .collect::<Vec<_>>(),
        vec!["www.google.com/search?q=tuido"]
    );

    drop(service);
    let _ = std::fs::remove_file(path);
}
