//! The vault's password and backups, end to end: real files in a temporary
//! folder, opened, changed, closed and reopened through `Store`.

use std::path::{Path, PathBuf};

use super::*;
use crate::backup;
use crate::images::{self, tests::sample};
use crate::{NoteDraft, NoteFilter, NoteQuery};

fn password(text: &str) -> Password {
    Password::new(text)
}

/// A vault file with a child, a note about her with a phrase to look for,
/// and a photo, so blobs are copied too.
async fn vault(dir: &Path) -> (PathBuf, Store) {
    let path = dir.join("vault.db");
    let store = Store::open(&path).await.unwrap();
    let emma = store
        .add_child(ChildDraft {
            name: "Emma".into(),
            color: 0xe0795b,
        })
        .await
        .unwrap();
    let note = store
        .add_note(NoteDraft {
            title: "First bike ride".into(),
            body: "She refused the training wheels".into(),
            children: vec![emma.id],
            labels: vec![],
        })
        .await
        .unwrap();
    let photo = images::prepare(sample(image::ImageFormat::Png, 40, 30)).unwrap();
    store.add_image(note.id, photo).await.unwrap();
    (path, store)
}

/// Checks everything [`vault`] wrote is there.
async fn assert_contents(store: &Store) {
    let children = store.children().await.unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].name, "Emma");
    let notes = store
        .notes(NoteQuery {
            filter: NoteFilter::Child(children[0].id),
            search: String::new(),
        })
        .await
        .unwrap();
    assert_eq!(notes.len(), 1, "the note is still linked to her");
    let note = store.note(notes[0].id).await.unwrap();
    assert_eq!(note.body, "She refused the training wheels");
    let images = store.note_images(note.id).await.unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!((images[0].width, images[0].height), (40, 30));
    store.display_image(images[0].id).await.unwrap();
}

/// Whether `phrase` appears anywhere in the vault's files.
fn on_disk(path: &Path, phrase: &str) -> bool {
    [path.to_owned(), crate::files::wal_path(path)]
        .iter()
        .filter_map(|file| std::fs::read(file).ok())
        .any(|bytes| bytes.windows(phrase.len()).any(|w| w == phrase.as_bytes()))
}

#[tokio::test]
async fn setting_a_password_encrypts_the_vault() {
    let dir = tempfile::tempdir().unwrap();
    let (path, store) = vault(dir.path()).await;
    store.close().await.unwrap();
    assert!(
        on_disk(&path, "training wheels"),
        "the test can see plaintext"
    );

    let store = Store::open(&path).await.unwrap();
    store
        .set_password(None, password("correct horse"))
        .await
        .unwrap();

    assert!(store.is_encrypted().await.unwrap());
    assert_contents(&store).await;
    store.close().await.unwrap();
    assert_eq!(lock::inspect(&path).unwrap(), VaultKind::Encrypted);
    assert!(!on_disk(&path, "training wheels"));
    assert!(!on_disk(&path, "Emma"));
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name != "vault.db" && name != "vault.key" && name != "vault.db-wal")
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[tokio::test]
async fn an_encrypted_vault_needs_its_password() {
    let dir = tempfile::tempdir().unwrap();
    let (path, store) = vault(dir.path()).await;
    store
        .set_password(None, password("correct horse"))
        .await
        .unwrap();
    store.close().await.unwrap();

    assert!(matches!(Store::open(&path).await, Err(Error::Locked)));
    assert!(matches!(
        Store::unlock(&path, password("wrong horse")).await,
        Err(Error::WrongPassword)
    ));
    let store = Store::unlock(&path, password("correct horse"))
        .await
        .unwrap();
    assert_contents(&store).await;

    // It keeps working: writes go to the encrypted file.
    store.add_label("milestone").await.unwrap();
    store.close().await.unwrap();
    let store = Store::unlock(&path, password("correct horse"))
        .await
        .unwrap();
    assert_eq!(store.labels().await.unwrap()[0].name, "milestone");
}

#[tokio::test]
async fn changing_the_password_needs_the_current_one() {
    let dir = tempfile::tempdir().unwrap();
    let (path, store) = vault(dir.path()).await;
    store
        .set_password(None, password("correct horse"))
        .await
        .unwrap();
    let before = std::fs::read(&path).unwrap();

    let wrong = store
        .set_password(Some(password("wrong horse")), password("battery staple"))
        .await;
    assert!(matches!(wrong, Err(Error::WrongPassword)));
    let missing = store.set_password(None, password("battery staple")).await;
    assert!(matches!(missing, Err(Error::WrongPassword)));

    store
        .set_password(Some(password("correct horse")), password("battery staple"))
        .await
        .unwrap();
    store.close().await.unwrap();

    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "only the key file changes"
    );
    assert!(matches!(
        Store::unlock(&path, password("correct horse")).await,
        Err(Error::WrongPassword)
    ));
    let store = Store::unlock(&path, password("battery staple"))
        .await
        .unwrap();
    assert_contents(&store).await;
}

#[tokio::test]
async fn removing_the_password_decrypts_the_vault() {
    let dir = tempfile::tempdir().unwrap();
    let (path, store) = vault(dir.path()).await;
    store
        .set_password(None, password("correct horse"))
        .await
        .unwrap();

    let wrong = store.remove_password(password("wrong horse")).await;
    assert!(matches!(wrong, Err(Error::WrongPassword)));
    assert!(store.is_encrypted().await.unwrap());

    store
        .remove_password(password("correct horse"))
        .await
        .unwrap();
    assert!(!store.is_encrypted().await.unwrap());
    assert_contents(&store).await;
    store.close().await.unwrap();

    assert_eq!(lock::inspect(&path).unwrap(), VaultKind::Plain);
    assert!(!key_path(&path).exists());
    let store = Store::open(&path).await.unwrap();
    assert_contents(&store).await;
}

#[tokio::test]
async fn new_passwords_must_be_long_enough() {
    let dir = tempfile::tempdir().unwrap();
    let (path, store) = vault(dir.path()).await;

    let result = store.set_password(None, password("short")).await;

    assert!(matches!(result, Err(Error::PasswordTooShort)));
    assert!(!store.is_encrypted().await.unwrap());
    store.close().await.unwrap();
    assert_eq!(lock::inspect(&path).unwrap(), VaultKind::Plain);
}

#[tokio::test]
async fn a_stray_key_file_next_to_a_plain_vault_is_removed() {
    // What a crash between writing the key file and swapping the vault in
    // leaves behind.
    let dir = tempfile::tempdir().unwrap();
    let (path, store) = vault(dir.path()).await;
    store.close().await.unwrap();
    std::fs::write(key_path(&path), "{}").unwrap();

    let store = Store::open(&path).await.unwrap();

    assert_contents(&store).await;
    assert!(!key_path(&path).exists());
}

#[tokio::test]
async fn an_encrypted_vault_without_its_key_file_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let (path, store) = vault(dir.path()).await;
    store
        .set_password(None, password("correct horse"))
        .await
        .unwrap();
    store.close().await.unwrap();
    std::fs::remove_file(key_path(&path)).unwrap();

    let result = Store::unlock(&path, password("correct horse")).await;

    assert!(matches!(result, Err(Error::KeyFileMissing(_))));
}

#[tokio::test]
async fn a_closed_store_refuses_requests() {
    let store = Store::open_in_memory().await.unwrap();
    let other = store.clone();

    store.close().await.unwrap();

    assert!(matches!(other.children().await, Err(Error::StoreClosed)));
}

// --- Backups ---

#[tokio::test]
async fn a_plain_backup_restores() {
    let dir = tempfile::tempdir().unwrap();
    let (path, store) = vault(dir.path()).await;
    let backup = dir.path().join("backup.papabackup");
    store.backup(&backup).await.unwrap();
    // Changed after the backup: the restore brings back the old state.
    store.add_label("after the backup").await.unwrap();
    let info = backup::inspect(&backup).unwrap();
    assert!(!info.encrypted);
    assert!(on_disk(&backup, "training wheels"), "plain stays plain");

    let prepared = backup::prepare_restore(&backup, &path, None).await.unwrap();
    store.close().await.unwrap();
    let kept = prepared.kept_path();
    prepared.commit().unwrap();

    let store = Store::open(&path).await.unwrap();
    assert_contents(&store).await;
    assert!(store.labels().await.unwrap().is_empty());
    store.close().await.unwrap();
    let kept = Store::open(&kept).await.unwrap();
    assert_eq!(
        kept.labels().await.unwrap().len(),
        1,
        "the old vault is kept"
    );
}

#[tokio::test]
async fn an_encrypted_backup_needs_the_password_it_was_made_with() {
    let dir = tempfile::tempdir().unwrap();
    let (path, store) = vault(dir.path()).await;
    store
        .set_password(None, password("correct horse"))
        .await
        .unwrap();
    let backup = dir.path().join("backup.papabackup");
    store.backup(&backup).await.unwrap();
    // A new password since: the backup still opens with the old one.
    store
        .set_password(Some(password("correct horse")), password("battery staple"))
        .await
        .unwrap();
    assert!(!on_disk(&backup, "training wheels"));
    assert!(backup::inspect(&backup).unwrap().encrypted);

    for wrong in [None, Some(password("battery staple"))] {
        let result = backup::prepare_restore(&backup, &path, wrong.as_ref()).await;
        assert!(matches!(result, Err(Error::WrongPassword)), "{wrong:?}");
    }
    let prepared = backup::prepare_restore(&backup, &path, Some(&password("correct horse")))
        .await
        .unwrap();
    store.close().await.unwrap();
    prepared.commit().unwrap();

    let store = Store::unlock(&path, password("correct horse"))
        .await
        .unwrap();
    assert_contents(&store).await;
}

#[tokio::test]
async fn an_encrypted_backup_replaces_a_plain_vault() {
    let dir = tempfile::tempdir().unwrap();
    let (path, store) = vault(dir.path()).await;
    store
        .set_password(None, password("correct horse"))
        .await
        .unwrap();
    let backup = dir.path().join("backup.papabackup");
    store.backup(&backup).await.unwrap();
    store
        .remove_password(password("correct horse"))
        .await
        .unwrap();

    let prepared = backup::prepare_restore(&backup, &path, Some(&password("correct horse")))
        .await
        .unwrap();
    store.close().await.unwrap();
    prepared.commit().unwrap();

    assert_eq!(lock::inspect(&path).unwrap(), VaultKind::Encrypted);
    let store = Store::unlock(&path, password("correct horse"))
        .await
        .unwrap();
    assert_contents(&store).await;
}

#[tokio::test]
async fn a_restore_that_is_not_committed_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (path, store) = vault(dir.path()).await;
    let backup = dir.path().join("backup.papabackup");
    store.backup(&backup).await.unwrap();

    let prepared = backup::prepare_restore(&backup, &path, None).await.unwrap();
    drop(prepared);

    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| !name.starts_with("vault.db") && name != "backup.papabackup")
        .collect();
    assert!(names.is_empty(), "{names:?}");
    assert_contents(&store).await;
}

#[tokio::test]
async fn files_that_are_not_backups_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (path, store) = vault(dir.path()).await;
    store.close().await.unwrap();

    for (name, bytes) in [
        ("empty", &b""[..]),
        ("text", b"hello"),
        ("bad header", b"PAPA BACKUP\nnot json\n"),
    ] {
        let file = dir.path().join(name);
        std::fs::write(&file, bytes).unwrap();
        assert!(
            matches!(backup::inspect(&file), Err(Error::NotABackup(_))),
            "{name}"
        );
    }
    // A vault isn't a backup either, though it's close.
    assert!(matches!(backup::inspect(&path), Err(Error::NotABackup(_))));
}

#[tokio::test]
async fn a_backup_from_a_newer_papa_is_refused_before_anything_changes() {
    let dir = tempfile::tempdir().unwrap();
    let (path, store) = vault(dir.path()).await;
    let backup = dir.path().join("backup.papabackup");
    store.backup(&backup).await.unwrap();
    // Rewrite the header's schema version, as a newer Papa would write it.
    let bytes = std::fs::read(&backup).unwrap();
    let text = String::from_utf8_lossy(&bytes[..200]).into_owned();
    let from = format!("\"schema_version\":{}", migrations::LATEST);
    assert!(text.contains(&from), "{text}");
    let to = format!("\"schema_version\":{}", migrations::LATEST + 1);
    let at = text.find(&from).unwrap();
    let mut newer = bytes[..at].to_vec();
    newer.extend_from_slice(to.as_bytes());
    newer.extend_from_slice(&bytes[at + from.len()..]);
    std::fs::write(&backup, newer).unwrap();

    let result = backup::prepare_restore(&backup, &path, None).await;

    assert!(matches!(result, Err(Error::SchemaTooNew { .. })));
    assert_contents(&store).await;
}

#[test]
fn backups_are_named_by_date() {
    let date = jiff::civil::date(2026, 10, 14);
    assert_eq!(backup::file_name(date), "Papa backup 2026-10-14.papabackup");
}
