use super::*;
use crate::domain::{TaskSize, TaskState};
use sqlx::any::AnyPoolOptions;

async fn test_service() -> TuidoService {
    sqlx::any::install_default_drivers();
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    TuidoService::from_parts(pool, SqlDialect::Sqlite)
}

#[test]
fn app_settings_are_persisted_and_replace_previous_values() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let service = test_service().await;

        assert_eq!(
            service.app_setting("calendar.show_weekends").await.unwrap(),
            None
        );

        service
            .set_app_setting("calendar.show_weekends", "false")
            .await
            .unwrap();
        let restarted_service = TuidoService::from_parts(service.pool.clone(), service.dialect);
        assert_eq!(
            restarted_service
                .app_setting("calendar.show_weekends")
                .await
                .unwrap(),
            Some("false".to_string())
        );

        service
            .set_app_setting("calendar.show_weekends", "true")
            .await
            .unwrap();
        assert_eq!(
            service.app_setting("calendar.show_weekends").await.unwrap(),
            Some("true".to_string())
        );
    });
}

#[test]
fn explicit_expiry_processing_unsnoozes_due_tasks() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let service = test_service().await;
        let create = |title: &str, snoozed_until: &str| TaskCreate {
            title: title.into(),
            description: String::new(),
            size: "small".into(),
            state: "snoozed".into(),
            priority: "medium".into(),
            start_date: None,
            due_date: None,
            snoozed_until: Some(snoozed_until.into()),
            people_ids: Vec::new(),
            project_ids: Vec::new(),
            tag_ids: Vec::new(),
            links: Vec::new(),
        };
        let expired = service
            .create_task(create("Expired", "2000-01-01T00:00:00"))
            .await
            .unwrap();
        let future = service
            .create_task(create("Future", "2099-01-01T00:00:00"))
            .await
            .unwrap();

        let before = service.workspace().await.unwrap();
        assert_eq!(
            before
                .tasks
                .iter()
                .find(|task| task.value.id == expired.value.id)
                .unwrap()
                .value
                .state,
            "snoozed"
        );
        service
            .process_snooze_expirations_at(
                crate::snooze::parse_datetime("2001-01-01T00:00:00").unwrap(),
            )
            .await
            .unwrap();
        let workspace = service.workspace().await.unwrap();
        let expired = workspace
            .tasks
            .iter()
            .find(|task| task.value.id == expired.value.id)
            .unwrap();
        let future = workspace
            .tasks
            .iter()
            .find(|task| task.value.id == future.value.id)
            .unwrap();

        assert_eq!(expired.value.state, "todo");
        assert_eq!(expired.value.snoozed_until, None);
        assert_eq!(expired.revision, 2);
        assert_eq!(future.value.state, "snoozed");
        assert_eq!(future.revision, 1);
        assert_eq!(workspace.revision, 4);
    });
}

#[test]
fn task_tags_by_label_reuse_create_replace_clear_and_rollback_atomically() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let service = test_service().await;
        let existing = service
            .create_tag(TagInput {
                label: "api".into(),
            })
            .await
            .unwrap();
        let task = service
            .create_task(TaskCreate {
                title: "Tagged".into(),
                description: String::new(),
                size: "small".into(),
                state: "todo".into(),
                priority: "medium".into(),
                start_date: None,
                due_date: None,
                snoozed_until: None,
                people_ids: Vec::new(),
                project_ids: Vec::new(),
                tag_ids: Vec::new(),
                links: Vec::new(),
            })
            .await
            .unwrap();

        let tagged = service
            .set_task_tags_by_label(
                task.value.id.clone(),
                task.revision,
                vec![" api ".into(), "new".into(), "new".into()],
            )
            .await
            .unwrap();
        assert_eq!(tagged.value.tag_ids.len(), 2);
        assert!(tagged.value.tag_ids.contains(&existing.value.id));
        let workspace = service.workspace().await.unwrap();
        assert_eq!(workspace.tags.len(), 2);
        assert_eq!(
            workspace
                .tags
                .iter()
                .filter(|tag| tag.value.label == "new")
                .count(),
            1
        );

        let replaced = service
            .set_task_tags_by_label(
                task.value.id.clone(),
                tagged.revision,
                vec!["replacement".into()],
            )
            .await
            .unwrap();
        assert_eq!(replaced.value.tag_ids.len(), 1);
        assert!(!replaced.value.tag_ids.contains(&existing.value.id));

        let cleared = service
            .set_task_tags_by_label(task.value.id.clone(), replaced.revision, Vec::new())
            .await
            .unwrap();
        assert!(cleared.value.tag_ids.is_empty());
        let before_stale = service.workspace_revision().await.unwrap();
        let error = service
            .set_task_tags_by_label(
                task.value.id.clone(),
                replaced.revision,
                vec!["ghost".into()],
            )
            .await
            .unwrap_err();
        assert!(matches!(error, ServiceError::Conflict { .. }));
        let workspace = service.workspace().await.unwrap();
        assert_eq!(workspace.revision, before_stale);
        assert!(!workspace.tags.iter().any(|tag| tag.value.label == "ghost"));
        assert!(
            service
                .get_task(&task.value.id)
                .await
                .unwrap()
                .value
                .tag_ids
                .is_empty()
        );
    });
}

#[test]
fn filtered_workspace_filters_tasks_and_returns_complete_entity_catalogs() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let service = test_service().await;
        let person = service
            .create_person(PersonInput {
                name: "Marlo".into(),
                email: "marlo@example.com".into(),
                active: true,
            })
            .await
            .unwrap();
        service
            .create_person(PersonInput {
                name: "Unrelated".into(),
                email: String::new(),
                active: true,
            })
            .await
            .unwrap();
        let project = service
            .create_project(ProjectInput {
                key: "LAUNCH".into(),
                name: "Launch".into(),
                description: String::new(),
                lead_person_id: Some(person.value.id.clone()),
            })
            .await
            .unwrap();
        service
            .create_project(ProjectInput {
                key: "OTHER".into(),
                name: "Unrelated".into(),
                description: String::new(),
                lead_person_id: None,
            })
            .await
            .unwrap();
        let tag = service
            .create_tag(TagInput { label: "UI".into() })
            .await
            .unwrap();
        service
            .create_tag(TagInput {
                label: "Unrelated".into(),
            })
            .await
            .unwrap();
        service
            .create_task(TaskCreate {
                title: "Keep selection stable".into(),
                description: "Select task after refresh".into(),
                size: "small".into(),
                state: "todo".into(),
                priority: "high".into(),
                start_date: None,
                due_date: Some("2026-07-30".into()),
                snoozed_until: None,
                people_ids: Vec::new(),
                project_ids: vec![project.value.id.clone()],
                tag_ids: vec![tag.value.id.clone()],
                links: Vec::new(),
            })
            .await
            .unwrap();
        service
            .create_task(TaskCreate {
                title: "Resolved task".into(),
                description: String::new(),
                size: "medium".into(),
                state: "done".into(),
                priority: "medium".into(),
                start_date: None,
                due_date: None,
                snoozed_until: None,
                people_ids: Vec::new(),
                project_ids: Vec::new(),
                tag_ids: Vec::new(),
                links: Vec::new(),
            })
            .await
            .unwrap();

        let workspace = service
            .filtered_workspace(WorkspaceFilter {
                states: vec!["todo".into()],
                priorities: vec!["high".into()],
                sizes: vec!["small".into()],
                project_ids: vec![project.value.id.clone()],
                tag_ids: vec![tag.value.id.clone()],
                due_before: Some("2026-08-01".into()),
                due_after: Some("2026-07-01".into()),
                query: Some("selection".into()),
                ..WorkspaceFilter::default()
            })
            .await
            .unwrap();

        assert_eq!(workspace.tasks.len(), 1);
        assert_eq!(workspace.people.len(), 2);
        assert!(
            workspace
                .people
                .iter()
                .any(|candidate| candidate.value.id == person.value.id)
        );
        assert_eq!(workspace.projects.len(), 2);
        assert_eq!(workspace.tags.len(), 2);

        let with_resolved = service
            .filtered_workspace(WorkspaceFilter {
                include_resolved: true,
                ..WorkspaceFilter::default()
            })
            .await
            .unwrap();
        assert_eq!(with_resolved.tasks.len(), 2);
    });
}

#[test]
fn filtered_workspace_rejects_resolved_states_and_unknown_relations_by_default() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let service = test_service().await;

        let resolved = service
            .filtered_workspace(WorkspaceFilter {
                states: vec!["done".into()],
                ..WorkspaceFilter::default()
            })
            .await;
        assert!(matches!(resolved, Err(ServiceError::Invalid(_))));

        let unknown = service
            .filtered_workspace(WorkspaceFilter {
                tag_ids: vec!["missing".into()],
                ..WorkspaceFilter::default()
            })
            .await;
        assert!(matches!(unknown, Err(ServiceError::Invalid(_))));
    });
}

#[test]
fn public_task_inputs_reject_legacy_state_aliases() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let service = test_service().await;

        for alias in ["clarify", "next", "waiting", "doing"] {
            let create = service
                .create_task(TaskCreate {
                    title: format!("Legacy {alias}"),
                    description: String::new(),
                    size: "small".into(),
                    state: alias.into(),
                    priority: "medium".into(),
                    start_date: None,
                    due_date: None,
                    snoozed_until: None,
                    people_ids: Vec::new(),
                    project_ids: Vec::new(),
                    tag_ids: Vec::new(),
                    links: Vec::new(),
                })
                .await;
            assert!(matches!(create, Err(ServiceError::Invalid(_))));

            let filter = service
                .filtered_workspace(WorkspaceFilter {
                    states: vec![alias.into()],
                    ..WorkspaceFilter::default()
                })
                .await;
            assert!(matches!(filter, Err(ServiceError::Invalid(_))));
        }

        let task = service
            .create_task(TaskCreate {
                title: "Canonical".into(),
                description: String::new(),
                size: "small".into(),
                state: "todo".into(),
                priority: "medium".into(),
                start_date: None,
                due_date: None,
                snoozed_until: None,
                people_ids: Vec::new(),
                project_ids: Vec::new(),
                tag_ids: Vec::new(),
                links: Vec::new(),
            })
            .await
            .unwrap();
        let update = service
            .update_task(TaskUpdate {
                id: task.value.id,
                expected_revision: task.revision,
                title: "Legacy update".into(),
                state: "doing".into(),
                size: "small".into(),
                priority: "medium".into(),
                start_date: None,
                due_date: None,
                snoozed_until: None,
                people_ids: Vec::new(),
                project_ids: Vec::new(),
                tag_ids: Vec::new(),
                links: Vec::new(),
                description: String::new(),
            })
            .await;
        assert!(matches!(update, Err(ServiceError::Invalid(_))));
    });
}

#[test]
fn stale_task_write_returns_typed_conflict_and_preserves_winner() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        let service = TuidoService::from_parts(pool, SqlDialect::Sqlite);
        let task = Task::quick_capture(
            "task".into(),
            "Original".into(),
            String::new(),
            TaskSize::Small,
        );
        service.create_task_entity(task).await.unwrap();

        service
            .patch_task("task".into(), 1, TaskPatch::State(TaskState::Done))
            .await
            .unwrap();
        let error = service
            .patch_task("task".into(), 1, TaskPatch::State(TaskState::Rejected))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            ServiceError::Conflict {
                expected: 1,
                actual: Some(2),
                ..
            }
        ));
        assert_eq!(service.get_task("task").await.unwrap().value.state, "done");
        assert_eq!(service.workspace_revision().await.unwrap(), 3);
    });
}

#[test]
fn failed_relation_replacement_rolls_back_entity_and_workspace_revisions() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        let service = TuidoService::from_parts(pool, SqlDialect::Sqlite);
        service
            .create_task_entity(Task::quick_capture(
                "task".into(),
                "Task".into(),
                String::new(),
                TaskSize::Small,
            ))
            .await
            .unwrap();

        assert!(
            service
                .patch_task("task".into(), 1, TaskPatch::People(vec!["missing".into()]))
                .await
                .is_err()
        );

        assert_eq!(service.get_task("task").await.unwrap().revision, 1);
        assert_eq!(service.workspace_revision().await.unwrap(), 2);
    });
}

#[test]
fn malformed_task_snooze_update_is_rejected_without_poisoning_workspace() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let service = test_service().await;
        service
            .create_task_entity(Task::quick_capture(
                "task".into(),
                "Original".into(),
                String::new(),
                TaskSize::Small,
            ))
            .await
            .unwrap();

        let error = service
            .update_task(TaskUpdate {
                id: "task".into(),
                expected_revision: 1,
                title: "Changed".into(),
                state: "todo".into(),
                size: "small".into(),
                priority: "medium".into(),
                start_date: None,
                due_date: None,
                snoozed_until: Some("not-a-date".into()),
                people_ids: Vec::new(),
                project_ids: Vec::new(),
                tag_ids: Vec::new(),
                links: Vec::new(),
                description: String::new(),
            })
            .await
            .unwrap_err();

        assert!(matches!(error, ServiceError::Invalid(_)));
        let task = service.get_task("task").await.unwrap();
        assert_eq!(task.revision, 1);
        assert_eq!(task.value.title, "Original");
        assert!(service.workspace().await.is_ok());
    });
}

#[test]
fn task_temporal_fields_and_snooze_invariants_are_enforced() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let service = test_service().await;
        let mut invalid_date = Task::quick_capture(
            "invalid-date".into(),
            "Invalid date".into(),
            String::new(),
            TaskSize::Small,
        );
        invalid_date.start_date = Some("2026-02-30".into());
        assert!(matches!(
            service.create_task_entity(invalid_date).await,
            Err(ServiceError::Invalid(_))
        ));

        let mut missing_until = Task::quick_capture(
            "missing-until".into(),
            "Missing until".into(),
            String::new(),
            TaskSize::Small,
        );
        missing_until.state = TaskState::Snoozed;
        assert!(matches!(
            service.create_task_entity(missing_until).await,
            Err(ServiceError::Invalid(_))
        ));

        let mut stale_until = Task::quick_capture(
            "stale-until".into(),
            "Stale until".into(),
            String::new(),
            TaskSize::Small,
        );
        stale_until.snoozed_until = Some(time::macros::datetime!(2026-07-25 08:00));
        assert!(matches!(
            service.create_task_entity(stale_until).await,
            Err(ServiceError::Invalid(_))
        ));
    });
}

#[test]
fn state_patch_clears_snooze_timestamp_and_snoozed_state_requires_snooze_action() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let service = test_service().await;
        let mut task =
            Task::quick_capture("task".into(), "Task".into(), String::new(), TaskSize::Small);
        task.state = TaskState::Snoozed;
        task.snoozed_until = Some(time::macros::datetime!(2026-07-25 08:00));
        service.create_task_entity(task).await.unwrap();

        service
            .patch_task("task".into(), 1, TaskPatch::State(TaskState::Done))
            .await
            .unwrap();
        let completed = service.get_task("task").await.unwrap();
        assert_eq!(completed.value.state, "done");
        assert_eq!(completed.value.snoozed_until, None);

        let error = service
            .patch_task("task".into(), 2, TaskPatch::State(TaskState::Snoozed))
            .await
            .unwrap_err();
        assert!(matches!(error, ServiceError::Invalid(_)));
        assert_eq!(service.get_task("task").await.unwrap().revision, 2);
    });
}

#[test]
fn cascade_deletes_increment_every_affected_revision() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let service = test_service().await;
        let person = Person::new("person".into(), "Ada".into(), String::new());
        service.create_person_entity(person).await.unwrap();
        let mut project = Project::new(
            "project".into(),
            "CORE".into(),
            "Core".into(),
            String::new(),
        );
        project.lead_person_id = Some("person".into());
        service.create_project_entity(project).await.unwrap();
        service
            .create_tag_entity(Tag::new("tag".into(), "api".into()))
            .await
            .unwrap();
        let mut task =
            Task::quick_capture("task".into(), "Task".into(), String::new(), TaskSize::Small);
        task.people_ids = vec!["person".into()];
        task.project_ids = vec!["project".into()];
        task.tag_ids = vec!["tag".into()];
        service.create_task_entity(task).await.unwrap();

        service.delete_person("person", 1).await.unwrap();
        assert_eq!(service.get_task("task").await.unwrap().revision, 2);
        let project = service
            .workspace()
            .await
            .unwrap()
            .projects
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(project.revision, 2);
        assert_eq!(project.value.lead_person_id, None);
        assert!(matches!(
            service
                .patch_project("project".into(), 1, ProjectPatch::Name("Stale".into()))
                .await,
            Err(ServiceError::Conflict {
                actual: Some(2),
                ..
            })
        ));

        service.delete_project("project", 2).await.unwrap();
        assert_eq!(service.get_task("task").await.unwrap().revision, 3);
        service.delete_tag("tag", 1).await.unwrap();
        assert_eq!(service.get_task("task").await.unwrap().revision, 4);
        assert!(matches!(
            service
                .patch_task("task".into(), 1, TaskPatch::Title("Stale".into()))
                .await,
            Err(ServiceError::Conflict {
                actual: Some(4),
                ..
            })
        ));
    });
}

#[test]
fn empty_management_updates_are_rejected() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let service = test_service().await;
        service
            .create_person_entity(Person::new("person".into(), "Ada".into(), String::new()))
            .await
            .unwrap();
        service
            .create_project_entity(Project::new(
                "project".into(),
                "CORE".into(),
                "Core".into(),
                String::new(),
            ))
            .await
            .unwrap();
        service
            .create_tag_entity(Tag::new("tag".into(), "api".into()))
            .await
            .unwrap();

        assert!(matches!(
            service
                .patch_person("person".into(), 1, PersonPatch::Name("  ".into()))
                .await,
            Err(ServiceError::Invalid(_))
        ));
        assert!(matches!(
            service
                .patch_project("project".into(), 1, ProjectPatch::Key(String::new()))
                .await,
            Err(ServiceError::Invalid(_))
        ));
        assert!(matches!(
            service
                .patch_tag("tag".into(), 1, TagPatch::Label("\t".into()))
                .await,
            Err(ServiceError::Invalid(_))
        ));
    });
}

#[test]
fn concurrent_workspace_reads_return_matching_values_and_revisions() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        sqlx::any::install_default_drivers();
        let path = std::env::temp_dir().join(format!("tuido-{}.sqlite", Uuid::new_v4()));
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let pool = AnyPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        sqlx::query("PRAGMA busy_timeout = 5000")
            .execute(&pool)
            .await
            .unwrap();
        let service = TuidoService::from_parts(pool.clone(), SqlDialect::Sqlite);
        service
            .create_task_entity(Task::quick_capture(
                "task".into(),
                "v0".into(),
                String::new(),
                TaskSize::Small,
            ))
            .await
            .unwrap();
        let writer = {
            let service = service.clone();
            tokio::spawn(async move {
                for revision in 1..=20 {
                    service
                        .patch_task(
                            "task".into(),
                            revision,
                            TaskPatch::Title(format!("v{revision}")),
                        )
                        .await
                        .unwrap();
                    tokio::task::yield_now().await;
                }
            })
        };

        for _ in 0..40 {
            let workspace = service.workspace().await.unwrap();
            let task = &workspace.tasks[0];
            assert_eq!(workspace.revision, task.revision + 1);
            let value_revision = task
                .value
                .title
                .strip_prefix('v')
                .unwrap()
                .parse::<u64>()
                .unwrap();
            assert_eq!(value_revision + 1, task.revision);
            tokio::task::yield_now().await;
        }
        writer.await.unwrap();
        pool.close().await;
        let _ = std::fs::remove_file(path);
    });
}
