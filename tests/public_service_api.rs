use tuido::service::{TaskCreate, TaskUpdate, TuidoService, WorkspaceFilter};

#[tokio::test]
async fn external_client_can_use_public_service_dtos() {
    let path =
        std::env::temp_dir().join(format!("tuido-public-api-{}.sqlite", uuid::Uuid::new_v4()));
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let service = TuidoService::connect_url(&url).await.unwrap();
    let created = service
        .create_task(TaskCreate {
            title: "External API".into(),
            detail: String::new(),
            size: "small".into(),
            state: "todo".into(),
            priority: "medium".into(),
            start_date: None,
            due_date: None,
            snoozed_until: None,
            people_ids: Vec::new(),
            project_ids: Vec::new(),
            tag_ids: Vec::new(),
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
            start_date: None,
            due_date: None,
            snoozed_until: None,
            people_ids: Vec::new(),
            project_ids: Vec::new(),
            tag_ids: Vec::new(),
            detail: "public DTO mutation".into(),
        })
        .await
        .unwrap();
    let workspace = service
        .filtered_workspace(WorkspaceFilter::default())
        .await
        .unwrap();

    assert_eq!(updated.value.state, "in_progress");
    assert_eq!(workspace.tasks[0].value.id, created.value.id);

    drop(service);
    let _ = std::fs::remove_file(path);
}
