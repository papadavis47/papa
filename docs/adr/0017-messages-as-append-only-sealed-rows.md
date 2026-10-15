# 0017. Messages: append-only sealed rows, current-key only, and polling

- **Status:** Proposed
- **Date:** 2026-10-15

## Context

Phase 13 lets a father and each child write to each other (PLAN.md §5). The share database already travels between them (ADR 0004, 0015, 0016). Forces at play:

- **Turso's sync is last-push-wins.** Until now only the father wrote to a share, so that never mattered. Now both sides write to the same database.
- **The child's token may only add rows** to `message` and `receipt` on Turso Cloud (the relay's fine-grained permissions, ADR 0015). It can't update or delete them.
- **One content key per share and epoch** is shared by father and child. Either side can seal anything the other can open.
- **A new key exists because the old one may have leaked.** Whoever has it, and a token from before, can still write rows until the token runs out.
- **The child's app opened the share read-only** (ADR 0016) and kept no state of its own beside the replica and the sign-in.
- **Turso 0.8.2's sync stats** count the changes not yet pushed (`cdc_operations`), but never set `last_push_unix_time`.
- **No push notifications** (PLAN.md §1): messages arrive when an app syncs.

## Decision

- **Rows are append-only, and each table has one writer.** A `message` row is written by its sender, a `receipt` row by the recipient. Ids are UUIDv7, made by the app that writes the message, so the optimistic copy on screen and the stored one are the same message. Nothing is updated in place, so last-push-wins has nothing to settle. The one exception is the father's: after a new key he reseals every message, the child's too, under the new epoch.
- **A message is sealed under the share's content key** (XChaCha20-Poly1305) with associated data `papa share v1 / message / share / id / sender / created_at / epoch`. The server can't move a message, change who sent it, when, or under which key, without it failing to open. Receipts carry only an id and a time, unsealed.
- **Only messages under the current key count.** The father's app takes in the child's messages of the current epoch only; anything else, or anything that won't open, is skipped and left where it is. The child's app shows and acknowledges only Dad's messages of the current epoch. A leaked old key can't put words in the child's mouth after a new key.
- **Before making a new key, the father's app receives once** (best effort), so messages already sent under the old key aren't lost to the new one.
- **The sync gains a step:** pull → *receive* (the child's new messages and receipts into the vault) → project (missing or old-epoch messages sealed, receipts for what Dad read) → apply → push → mark the father's messages that reached the cloud as sent. Like notes, the projection compares with what the replica holds, so a failed sync is repaired by the next one.
- **The vault keeps the conversation** in its own `message` table (migration 5), in plain text like notes. It belongs to the child's record, not the share's: stopping sharing deletes the cloud copy, not Dad's.
- **The child's app now writes to the share,** but only messages (a plain insert) and receipts. It still never migrates it: messages wait until Dad's app has synced schema 2.
- **The child's outbox:** `child/outbox.json` lists the ids written but not yet pushed, saved before the row so a crash can't make a message look sent. Whether anything is waiting comes from Turso's `cdc_operations`. Every pull pushes first and then empties the outbox. A message written offline shows "Sending…" across restarts until then.
- **Optimistic UI:** the conversation shows a draft at once, then the stored message replaces it by id; if storing fails, the draft goes and its text returns to the box.
- **Polling:** the father's app syncs every 30 s, at once after he sends, and 2 s after he reads (so receipts from several conversations share one sync). The child's app pulls every 30 s (was 60 s) and pushes at once after sending or reading.
- **Read means seen:** a conversation is marked read when it's the one showing and the window is in front, checked again when the window is activated. A message arriving while it isn't brings a notification.
- **Layout:** the father's Messages pane (sidebar footer, ⌘⇧M, ⌘K "Write to {child}") lists conversations in the middle column and shows one on the right. Child mode gets the sidebar ADR 0016 deferred: "Notes from Dad" and "Messages".

## Consequences

- No conflict resolution code, and no server code: the relay is unchanged.
- **A message the child sends under the old key after a new one is made is lost.** The child's app finds out the key changed at its next relay check (within 15 minutes, ADR 0016) and asks for the new key; the message stays in their replica but Dad's app won't take it.
- **The content key is shared, so `sender` is "whoever has the key".** The child, or someone with their current key, could seal a row marked as Dad's. Dad's app ignores it; the child's app would show it. Fixing that needs per-sender signing keys, which isn't worth it while the key is the child's own secret and a new key ends it.
- **Metadata visible to the server:** who wrote each message, when, its size, and when it was read. A server could fake a receipt; it can't fake or change a message.
- Polling every 30 s is a small request per open app and child; Turso Cloud's cost per pull is still to be measured.
- **Still not checked against Turso Cloud:** that a child token with `data_add` on `message` and `receipt` can push these rows (ADR 0014–0016). Local mode checks no tokens.
- The child's app holds one more file, deleted on sign-in and sign-out.
