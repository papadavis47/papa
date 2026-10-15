# 0016. Child mode: a mode is a type, the replica is the offline cache, and the server travels with the key

- **Status:** Accepted
- **Date:** 2026-10-15

## Context

Phase 12 builds the child's side of sharing (ADR 0004, 0015): the same binary opens either a father's vault or the notes his child can read. Forces at play:

- **Two very different apps in one window.** The father's has a vault, an editor, labels, a sidebar of children, a lock and sharing dialogs. The child's has a list of decrypted notes, read-only, and nothing else of his.
- **Children read on the go.** The app should open, and show what it had, without a network.
- **Turso's sync replica already is a cache.** A test at the start of the phase showed that an existing replica opens offline in about 30 ms and reads what the last pull brought. Creating one needs the server, and a failed attempt leaves a file behind.
- **The child signs in with Dad's email and a key, and nothing else** (PLAN.md §2.2). The key has to name the server unless it's Papa's own; ADR 0015 left the format to this phase.
- **Tokens outlive keys.** A child's database token is good for an hour (ADR 0014). Dad can make a new key or stop sharing at any time. In local mode the sync server even recreates a deleted database on the next pull, so a pull can't be trusted to fail.
- **Keys and the share move separately.** The relay learns a new key at once, but the share is resealed only at Dad's next sync. A child given the new key in between finds a share their key can't open yet.

## Decision

- **The window's root holds the mode in an enum:** `Opening | Welcome(WelcomeScreen) | Father(PapaApp) | Child(ChildApp)`. Each mode is its own view type with only its own state. The images of a note come from `ImageSource::{Vault(Store), Shared(ChildSession)}`, and only `Vault` hands out the `Store` that adding or deleting an image needs, so the child's gallery can't do either. A `ModeStatus` global tells the Settings window which pages apply.
- **Which mode opens:** a vault on disk means father mode, as before, even if a child's sign-in is there too. A saved sign-in means child mode. With neither, a welcome screen asks "Start my own vault" or "Open notes shared with me". The child's files live in `child/` next to where the vault would be, so `PAPA_VAULT` moves both.
- **`ChildSession`, an actor on a `papa-child` thread**, like `Sharing`: sign in, open, pull, a note's images, one image, sign out. The replica (`child/share.db`) is the offline cache and holds only ciphertext. It opens with the network down; notes are decrypted into memory when the app opens and after each pull. The sign-in, including the access key, is kept in `child/session.json`, readable only by the child's user account, so the app opens without asking.
- **The child's app never writes to the share.** It opens the replica without migrating it: the schema is Dad's to make. Until his first sync the database is empty, and the app says nothing has been shared yet.
- **The key card:** `PAPA-XXXX-…` on Papa's own server, `PAPA-XXXX-…@papa.example.org` on another (https assumed), `…@http://host:port` on a plain-http one. Only the secret symbols go into the derived keys, so keys already handed out keep working. The father's sharing dialog shows and copies the card.
- **Asking the relay is how a child learns a key stopped working.** A token is reused for at most 15 minutes, then the relay is asked for a new one. After a pull that fails, or that brings a share the key can't open, the relay is asked at once. If it refuses the key, the app says "Your key no longer works" and offers to sign in again; the notes already on screen stay readable until then.
- **A share the key can't open, with a key the relay accepts, is waiting, not broken:** Dad's app hasn't resealed it yet. The app says so and shows the notes after his next sync. Rows sealed under an older epoch are skipped for the same reason. A row of the current epoch that won't open is an error: it was changed or moved.
- **A failed sign-in leaves nothing behind**, and signing in replaces whatever share was there.
- **Child mode has two panes**, the list and the note, with no sidebar until Phase 13 brings Messages. Polls every 60 s, on opening, and on ⌘R. PDF export shares the father's code: `save_pdf` is generic over the view and takes the document as an async closure.

## Consequences

- Father mode is unchanged for an existing install: the vault exists, so it opens as before.
- A key on a child's computer is as safe as that computer's user account: it sits in a file only that account can read. The OS keychain (PLAN.md §7) would be better, and comes later.
- A stopped share or a replaced key reaches a running child's app within 15 minutes, or at once on Turso Cloud, where the database is gone. A restart notices at the first pull. One token request every 15 minutes per child is light for the relay.
- Dates, titles and bodies are decrypted only in memory. Images are decrypted when shown, and a gallery decrypts each display copy to read its size.
- **Still not checked against Turso Cloud:** whether a child token with fine-grained permissions can bootstrap and pull (ADR 0014, 0015). Local mode checks no tokens.
- The child's app has no command palette. Signing out lives in Settings → Shared with me.
