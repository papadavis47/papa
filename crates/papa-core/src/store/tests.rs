//! Tests go through the public `Store` handle, so they cover the actor thread
//! and the SQL together. Each test gets its own in-memory or temporary vault.

use super::db::SELECT_IMAGES;
use super::*;
use crate::images::{self, tests::sample};
use crate::{EncodedImage, ImageFormat, NoteDraft, NoteFilter, NoteQuery, PreparedImage};

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

// --- Notes ---

fn note(title: &str, body: &str) -> NoteDraft {
    NoteDraft {
        title: title.into(),
        body: body.into(),
        ..NoteDraft::default()
    }
}

fn query(filter: NoteFilter, search: &str) -> NoteQuery {
    NoteQuery {
        filter,
        search: search.into(),
    }
}

async fn titles(store: &Store, query: NoteQuery) -> Vec<String> {
    let notes = store.notes(query).await.unwrap();
    notes.into_iter().map(|note| note.title).collect()
}

#[tokio::test]
async fn notes_are_listed_newest_first() {
    let store = Store::open_in_memory().await.unwrap();
    for title in ["First", "Second", "Third"] {
        store.add_note(note(title, "")).await.unwrap();
    }

    let all = titles(&store, NoteQuery::default()).await;

    assert_eq!(all, ["Third", "Second", "First"]);
}

#[tokio::test]
async fn a_new_note_round_trips() {
    let store = Store::open_in_memory().await.unwrap();
    let emma = store.add_child(draft("Emma", 0)).await.unwrap();
    let funny = store.add_label("funny").await.unwrap();

    let added = store
        .add_note(NoteDraft {
            title: "  First bike ride ".into(),
            body: "She refused the *training wheels*.".into(),
            children: vec![emma.id],
            labels: vec![funny.id],
        })
        .await
        .unwrap();

    assert_eq!(added.title, "First bike ride", "titles are trimmed");
    assert_eq!(added.created_at, added.updated_at);
    assert_eq!(store.note(added.id).await.unwrap(), added);
    let listed = store.notes(NoteQuery::default()).await.unwrap();
    assert_eq!(listed, [added.summary(0)]);
    assert_eq!(listed[0].preview, "She refused the training wheels.");
}

#[tokio::test]
async fn saving_text_keeps_links_and_creation_time() {
    let store = Store::open_in_memory().await.unwrap();
    let emma = store.add_child(draft("Emma", 0)).await.unwrap();
    let added = store
        .add_note(NoteDraft {
            children: vec![emma.id],
            ..note("Draft", "")
        })
        .await
        .unwrap();

    let saved = store
        .update_note_text(added.id, "School play", "Leo was a tree.")
        .await
        .unwrap();

    assert_eq!(saved.title, "School play");
    assert_eq!(saved.body, "Leo was a tree.");
    assert_eq!(saved.children, [emma.id]);
    assert_eq!(saved.created_at, added.created_at);
    assert!(saved.updated_at >= added.updated_at);
    assert_eq!(store.note(added.id).await.unwrap(), saved);
}

#[tokio::test]
async fn notes_can_be_filtered_by_child_or_label() {
    let store = Store::open_in_memory().await.unwrap();
    let emma = store.add_child(draft("Emma", 0)).await.unwrap();
    let leo = store.add_child(draft("Leo", 0)).await.unwrap();
    let funny = store.add_label("funny").await.unwrap();
    let both = NoteDraft {
        children: vec![emma.id, leo.id],
        labels: vec![funny.id],
        ..note("Beach day", "")
    };
    let emma_only = NoteDraft {
        children: vec![emma.id],
        ..note("Bike ride", "")
    };
    store.add_note(both).await.unwrap();
    store.add_note(emma_only).await.unwrap();
    store.add_note(note("Unassigned", "")).await.unwrap();

    let for_emma = titles(&store, query(NoteFilter::Child(emma.id), "")).await;
    let for_leo = titles(&store, query(NoteFilter::Child(leo.id), "")).await;
    let funny_notes = titles(&store, query(NoteFilter::Label(funny.id), "")).await;

    assert_eq!(for_emma, ["Bike ride", "Beach day"]);
    assert_eq!(for_leo, ["Beach day"]);
    assert_eq!(funny_notes, ["Beach day"]);
}

#[tokio::test]
async fn search_matches_every_word_ignoring_case() {
    let store = Store::open_in_memory().await.unwrap();
    let emma = store.add_child(draft("Emma", 0)).await.unwrap();
    let bike = NoteDraft {
        children: vec![emma.id],
        ..note("First bike ride", "Émilie came too.")
    };
    store.add_note(bike).await.unwrap();
    store
        .add_note(note("School play", "Leo was a tree."))
        .await
        .unwrap();

    let all = NoteFilter::All;
    assert_eq!(
        titles(&store, query(all, "BIKE")).await,
        ["First bike ride"]
    );
    assert_eq!(
        titles(&store, query(all, "émilie")).await,
        ["First bike ride"]
    );
    assert_eq!(
        titles(&store, query(all, "bik tre")).await,
        Vec::<String>::new()
    );
    assert_eq!(titles(&store, query(all, "tree")).await, ["School play"]);
    // A search narrows the filter, it doesn't replace it.
    let emma_tree = query(NoteFilter::Child(emma.id), "tree");
    assert!(titles(&store, emma_tree).await.is_empty());
    // SQL wildcards are just characters.
    assert!(titles(&store, query(all, "%")).await.is_empty());
}

#[tokio::test]
async fn links_are_listed_in_sidebar_order() {
    let store = Store::open_in_memory().await.unwrap();
    let emma = store.add_child(draft("Emma", 0)).await.unwrap();
    let leo = store.add_child(draft("Leo", 0)).await.unwrap();
    let school = store.add_label("school").await.unwrap();
    let funny = store.add_label("Funny").await.unwrap();
    let added = store.add_note(note("Beach day", "")).await.unwrap();

    let note = store
        .set_note_children(added.id, vec![leo.id, emma.id, leo.id])
        .await
        .unwrap();
    let note = store
        .set_note_labels(note.id, vec![school.id, funny.id])
        .await
        .unwrap();

    assert_eq!(
        note.children,
        [emma.id, leo.id],
        "sorted, duplicates dropped"
    );
    assert_eq!(note.labels, [funny.id, school.id]);
    let listed = store.notes(NoteQuery::default()).await.unwrap();
    assert_eq!(listed[0].children, note.children);
    assert_eq!(listed[0].labels, note.labels);

    let cleared = store.set_note_children(note.id, vec![]).await.unwrap();
    assert!(cleared.children.is_empty());
    assert_eq!(cleared.labels, note.labels);
}

#[tokio::test]
async fn linking_a_missing_child_changes_nothing() {
    let store = Store::open_in_memory().await.unwrap();
    let emma = store.add_child(draft("Emma", 0)).await.unwrap();
    let added = store
        .add_note(NoteDraft {
            children: vec![emma.id],
            ..note("Beach day", "")
        })
        .await
        .unwrap();

    let result = store
        .set_note_children(added.id, vec![ChildId::new()])
        .await;
    let new_note = store
        .add_note(NoteDraft {
            children: vec![ChildId::new()],
            ..note("Orphan", "")
        })
        .await;

    assert!(matches!(result, Err(Error::Database(_))), "{result:?}");
    assert!(matches!(new_note, Err(Error::Database(_))), "{new_note:?}");
    assert_eq!(store.note(added.id).await.unwrap().children, [emma.id]);
    assert_eq!(titles(&store, NoteQuery::default()).await, ["Beach day"]);
}

#[tokio::test]
async fn deleting_a_child_or_label_keeps_its_notes() {
    let store = Store::open_in_memory().await.unwrap();
    let emma = store.add_child(draft("Emma", 0)).await.unwrap();
    let funny = store.add_label("funny").await.unwrap();
    let added = store
        .add_note(NoteDraft {
            children: vec![emma.id],
            labels: vec![funny.id],
            ..note("Beach day", "")
        })
        .await
        .unwrap();

    store.delete_child(emma.id).await.unwrap();
    store.delete_label(funny.id).await.unwrap();

    let note = store.note(added.id).await.unwrap();
    assert!(note.children.is_empty());
    assert!(note.labels.is_empty());
}

#[tokio::test]
async fn a_deleted_note_is_gone_and_its_text_wiped() {
    let store = Store::open_in_memory().await.unwrap();
    let emma = store.add_child(draft("Emma", 0)).await.unwrap();
    let added = store
        .add_note(NoteDraft {
            children: vec![emma.id],
            ..note("Secret", "Private words")
        })
        .await
        .unwrap();

    store.delete_note(added.id).await.unwrap();

    assert!(store.notes(NoteQuery::default()).await.unwrap().is_empty());
    let for_emma = query(NoteFilter::Child(emma.id), "");
    assert!(store.notes(for_emma).await.unwrap().is_empty());
    assert!(matches!(
        store.note(added.id).await,
        Err(Error::NotFound("note"))
    ));
    // Changing it again reports it missing, like any other row.
    let save = store.update_note_text(added.id, "x", "y").await;
    let delete = store.delete_note(added.id).await;
    assert!(matches!(save, Err(Error::NotFound("note"))));
    assert!(matches!(delete, Err(Error::NotFound("note"))));
}

#[tokio::test]
async fn a_deleted_note_leaves_only_a_tombstone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault.db");
    let store = Store::open(&path).await.unwrap();
    let added = store
        .add_note(note("Secret", "Private words"))
        .await
        .unwrap();
    store.delete_note(added.id).await.unwrap();
    drop(store);

    let db = Db::open(&Location::File(path)).await.unwrap();
    let mut rows = db
        .connection()
        .query(
            "SELECT title, body_md, deleted_at IS NOT NULL FROM note WHERE id = ?1",
            [added.id.to_string()],
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let (title, body, deleted): (String, String, i64) = (
        row.get(0).unwrap(),
        row.get(1).unwrap(),
        row.get(2).unwrap(),
    );
    assert_eq!((title.as_str(), body.as_str(), deleted), ("", "", 1));
}

// --- Images ---

fn png(width: u32, height: u32) -> PreparedImage {
    images::prepare(sample(image::ImageFormat::Png, width, height)).unwrap()
}

#[tokio::test]
async fn an_image_round_trips_with_its_thumbnail() {
    let store = Store::open_in_memory().await.unwrap();
    let note = store.add_note(note("Drawing", "")).await.unwrap();
    let prepared = png(300, 200);

    let added = store.add_image(note.id, prepared.clone()).await.unwrap();

    assert_eq!(added.note_id, note.id);
    assert_eq!(added.format, ImageFormat::Png);
    assert_eq!((added.width, added.height), (300, 200));
    assert_eq!(added.thumb, prepared.thumb);
    assert_eq!(
        store.note_images(note.id).await.unwrap(),
        std::slice::from_ref(&added)
    );
    // Small and upright: the original is shown as it is.
    assert_eq!(
        store.display_image(added.id).await.unwrap(),
        prepared.original
    );
    let listed = store.notes(NoteQuery::default()).await.unwrap();
    assert_eq!(listed[0].images, 1);
    let touched = store.note(note.id).await.unwrap();
    assert!(touched.updated_at >= note.updated_at);
}

#[tokio::test]
async fn a_large_image_is_displayed_from_its_smaller_copy() {
    let store = Store::open_in_memory().await.unwrap();
    let note = store.add_note(note("Beach", "")).await.unwrap();
    let prepared = images::prepare(sample(image::ImageFormat::Jpeg, 3000, 2000)).unwrap();

    let added = store.add_image(note.id, prepared.clone()).await.unwrap();

    let shown = store.display_image(added.id).await.unwrap();
    assert_eq!(Some(shown), prepared.display);
}

#[tokio::test]
async fn images_are_listed_in_the_order_they_were_added() {
    let store = Store::open_in_memory().await.unwrap();
    let note = store.add_note(note("Holiday", "")).await.unwrap();

    let mut added = Vec::new();
    for width in [30, 10, 20] {
        added.push(store.add_image(note.id, png(width, 10)).await.unwrap());
    }

    assert_eq!(store.note_images(note.id).await.unwrap(), added);
}

#[tokio::test]
async fn attaching_the_same_image_twice_keeps_one_copy() {
    let store = Store::open_in_memory().await.unwrap();
    let drawing = store.add_note(note("Drawing", "")).await.unwrap();
    let other = store.add_note(note("Fridge", "")).await.unwrap();

    let first = store.add_image(drawing.id, png(20, 20)).await.unwrap();
    let again = store.add_image(drawing.id, png(20, 20)).await.unwrap();
    let elsewhere = store.add_image(other.id, png(20, 20)).await.unwrap();

    assert_eq!(again, first);
    assert_eq!(store.note_images(drawing.id).await.unwrap().len(), 1);
    assert_ne!(elsewhere.id, first.id, "another note gets its own copy");
}

#[tokio::test]
async fn a_deleted_image_is_gone() {
    let store = Store::open_in_memory().await.unwrap();
    let note = store.add_note(note("Drawing", "")).await.unwrap();
    let kept = store.add_image(note.id, png(10, 10)).await.unwrap();
    let removed = store.add_image(note.id, png(20, 20)).await.unwrap();

    store.delete_image(removed.id).await.unwrap();

    assert_eq!(store.note_images(note.id).await.unwrap(), [kept]);
    assert!(matches!(
        store.display_image(removed.id).await,
        Err(Error::NotFound("image"))
    ));
    assert!(matches!(
        store.delete_image(removed.id).await,
        Err(Error::NotFound("image"))
    ));
}

#[tokio::test]
async fn images_can_only_be_attached_to_live_notes() {
    let store = Store::open_in_memory().await.unwrap();
    let note = store.add_note(note("Gone", "")).await.unwrap();
    store.delete_note(note.id).await.unwrap();

    let attached = store.add_image(note.id, png(10, 10)).await;

    assert!(matches!(attached, Err(Error::NotFound("note"))));
}

#[tokio::test]
async fn deleting_a_note_deletes_its_images() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault.db");
    let store = Store::open(&path).await.unwrap();
    let note = store.add_note(note("Secret", "")).await.unwrap();
    store.add_image(note.id, png(10, 10)).await.unwrap();

    store.delete_note(note.id).await.unwrap();
    drop(store);

    let db = Db::open(&Location::File(path)).await.unwrap();
    for table in ["image", "image_data"] {
        let mut rows = db
            .connection()
            .query(&format!("SELECT count(*) FROM {table}"), ())
            .await
            .unwrap();
        let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(count, 0, "rows left in {table}");
    }
}

#[tokio::test]
async fn large_images_survive_reopening_the_vault() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault.db");
    let store = Store::open(&path).await.unwrap();
    let note = store.add_note(note("Big", "")).await.unwrap();
    // The store doesn't decode images, so made-up bytes do. 12 MB spans
    // thousands of database pages.
    let original: Vec<u8> = (0..12_000_000u32).map(|i| (i % 251) as u8).collect();
    let thumb = EncodedImage {
        format: ImageFormat::Jpeg,
        bytes: vec![1, 2, 3],
    };
    let image = PreparedImage {
        original: EncodedImage {
            format: ImageFormat::Jpeg,
            bytes: original.clone(),
        },
        display: None,
        thumb: thumb.clone(),
        width: 4000,
        height: 3000,
        sha256: "made-up".into(),
    };
    let added = store.add_image(note.id, image).await.unwrap();
    drop(store);

    let store = Store::open(&path).await.unwrap();

    let shown = store.display_image(added.id).await.unwrap();
    assert!(shown.bytes == original, "the bytes came back changed");
    assert_eq!(store.note_images(note.id).await.unwrap()[0].thumb, thumb);
}

#[tokio::test]
async fn image_queries_never_scan_the_image_bytes() {
    // A scan of `image_data` reads every original in the vault: seconds,
    // not milliseconds. Checked on the plans, as timing would be flaky.
    let db = Db::open(&Location::Memory).await.unwrap();
    let queries = [
        format!("{SELECT_IMAGES} WHERE image.note_id = ?1 AND ?2 ORDER BY image.id"),
        format!("{SELECT_IMAGES} WHERE image.note_id = ?1 AND image.sha256 = ?2"),
        "SELECT mime, bytes FROM image_data WHERE image_id = ?1 AND kind = ?2".into(),
    ];
    for sql in queries {
        // Every query takes two parameters; `AND ?2` pads the first.
        let mut rows = db
            .connection()
            .query(&format!("EXPLAIN QUERY PLAN {sql}"), ("x", 1))
            .await
            .unwrap();
        while let Some(row) = rows.next().await.unwrap() {
            let step: String = row.get(3).unwrap();
            assert!(!step.starts_with("SCAN"), "{step}\nin: {sql}");
        }
    }
}
