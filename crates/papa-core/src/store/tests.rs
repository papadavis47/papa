//! Tests go through the public `Store` handle, so they cover the actor thread
//! and the SQL together. Each test gets its own in-memory or temporary vault.

use super::*;

fn draft(name: &str, color: u32) -> ChildDraft {
    ChildDraft {
        name: name.into(),
        color,
    }
}

fn names<T>(items: &[T], name: impl Fn(&T) -> &str) -> Vec<&str> {
    items.iter().map(name).collect()
}

#[tokio::test]
async fn a_new_vault_is_empty() {
    let store = Store::open_in_memory().await.unwrap();

    assert!(store.children().await.unwrap().is_empty());
    assert!(store.labels().await.unwrap().is_empty());
}

#[tokio::test]
async fn children_are_listed_in_the_order_they_were_added() {
    let store = Store::open_in_memory().await.unwrap();

    let emma = store.add_child(draft("  Emma ", 0xe0795b)).await.unwrap();
    store.add_child(draft("Leo", 0x5b9be0)).await.unwrap();
    store.add_child(draft("Ada", 0x6bbf7a)).await.unwrap();

    assert_eq!(emma.name, "Emma", "names are trimmed");
    let children = store.children().await.unwrap();
    assert_eq!(names(&children, |c| &c.name), ["Emma", "Leo", "Ada"]);
    assert_eq!(children[0], emma);
}

#[tokio::test]
async fn updating_a_child_keeps_its_id_and_position() {
    let store = Store::open_in_memory().await.unwrap();
    let emma = store.add_child(draft("Emma", 0xe0795b)).await.unwrap();
    store.add_child(draft("Leo", 0x5b9be0)).await.unwrap();

    let updated = store
        .update_child(emma.id, draft("Emmy", 0x123456))
        .await
        .unwrap();

    assert_eq!(updated.id, emma.id);
    let children = store.children().await.unwrap();
    assert_eq!(children[0], updated);
    assert_eq!(children[0].color, 0x123456);
}

#[tokio::test]
async fn deleting_a_child_removes_only_that_child() {
    let store = Store::open_in_memory().await.unwrap();
    let emma = store.add_child(draft("Emma", 0)).await.unwrap();
    store.add_child(draft("Leo", 0)).await.unwrap();

    store.delete_child(emma.id).await.unwrap();

    let children = store.children().await.unwrap();
    assert_eq!(names(&children, |c| &c.name), ["Leo"]);
}

#[tokio::test]
async fn invalid_children_are_rejected() {
    let store = Store::open_in_memory().await.unwrap();

    let blank = store.add_child(draft("   ", 0)).await;
    let bad_color = store.add_child(draft("Emma", 0x1_00_00_00)).await;

    assert!(matches!(blank, Err(Error::EmptyName)));
    assert!(matches!(bad_color, Err(Error::InvalidColor(_))));
    assert!(store.children().await.unwrap().is_empty());
}

#[tokio::test]
async fn changing_a_missing_row_reports_not_found() {
    let store = Store::open_in_memory().await.unwrap();

    let update = store.update_child(ChildId::new(), draft("Emma", 0)).await;
    let delete = store.delete_child(ChildId::new()).await;
    let rename = store.rename_label(LabelId::new(), "funny").await;
    let delete_label = store.delete_label(LabelId::new()).await;

    assert!(matches!(update, Err(Error::NotFound("child"))));
    assert!(matches!(delete, Err(Error::NotFound("child"))));
    assert!(matches!(rename, Err(Error::NotFound("label"))));
    assert!(matches!(delete_label, Err(Error::NotFound("label"))));
}

#[tokio::test]
async fn labels_are_sorted_ignoring_case() {
    let store = Store::open_in_memory().await.unwrap();

    for name in ["school", "Milestone", "#funny"] {
        store.add_label(name).await.unwrap();
    }

    let labels = store.labels().await.unwrap();
    assert_eq!(
        names(&labels, |l| &l.name),
        ["funny", "Milestone", "school"]
    );
}

#[tokio::test]
async fn label_names_are_unique_ignoring_case() {
    let store = Store::open_in_memory().await.unwrap();
    store.add_label("Funny").await.unwrap();
    let school = store.add_label("school").await.unwrap();

    let added = store.add_label("#FUNNY").await;
    let renamed = store.rename_label(school.id, "funny").await;

    assert!(matches!(added, Err(Error::DuplicateLabel(name)) if name == "Funny"));
    assert!(matches!(renamed, Err(Error::DuplicateLabel(_))));
}

#[tokio::test]
async fn a_label_can_be_renamed_to_a_different_case_of_its_own_name() {
    let store = Store::open_in_memory().await.unwrap();
    let label = store.add_label("school").await.unwrap();

    let renamed = store.rename_label(label.id, "School").await.unwrap();

    assert_eq!(renamed.id, label.id);
    assert_eq!(store.labels().await.unwrap(), [renamed]);
}

#[tokio::test]
async fn deleting_a_label_frees_its_name() {
    let store = Store::open_in_memory().await.unwrap();
    let label = store.add_label("funny").await.unwrap();

    store.delete_label(label.id).await.unwrap();

    assert!(store.labels().await.unwrap().is_empty());
    store.add_label("funny").await.unwrap();
}

#[tokio::test]
async fn clones_share_one_vault() {
    let store = Store::open_in_memory().await.unwrap();
    let clone = store.clone();

    clone.add_label("funny").await.unwrap();

    assert_eq!(store.labels().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_vault_file_keeps_its_data_after_reopening() {
    let dir = tempfile::tempdir().unwrap();
    // A folder that doesn't exist yet: opening creates it.
    let path = dir.path().join("nested").join("vault.db");

    let store = Store::open(&path).await.unwrap();
    store.add_child(draft("Emma", 0xe0795b)).await.unwrap();
    store.add_label("funny").await.unwrap();
    drop(store);

    let store = Store::open(&path).await.unwrap();
    let children = store.children().await.unwrap();
    assert_eq!(names(&children, |c| &c.name), ["Emma"]);
    assert_eq!(store.labels().await.unwrap().len(), 1);
}

#[tokio::test]
async fn migrations_bring_a_new_vault_to_the_latest_version() {
    let db = Db::open(&Location::Memory).await.unwrap();

    let version = migrations::schema_version(db.connection()).await.unwrap();

    assert_eq!(version, migrations::LATEST);
}

#[tokio::test]
async fn a_vault_from_a_newer_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault.db");
    let future = migrations::LATEST + 1;
    {
        let db = Db::open(&Location::File(path.clone())).await.unwrap();
        db.connection()
            .execute(&format!("PRAGMA user_version = {future}"), ())
            .await
            .unwrap();
    }

    let result = Store::open(&path).await;

    assert!(matches!(
        result,
        Err(Error::SchemaTooNew { found, supported })
            if found == future && supported == migrations::LATEST
    ));
}
