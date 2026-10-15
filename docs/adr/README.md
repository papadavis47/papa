# Architecture Decision Records

Short records of significant decisions: the context, what we chose, and what it costs us. New decisions get the next number. A superseded record is marked as such, not deleted.

| # | Decision | Status |
|---|---|---|
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted |
| [0002](0002-workspace-and-gpui-kit-only-ui-dependency.md) | Cargo workspace; gpui-kit is the only UI dependency | Accepted |
| [0003](0003-turso-for-local-storage.md) | Turso (`turso` crate) for local storage | Accepted |
| [0004](0004-database-per-child-end-to-end-encrypted-sharing.md) | One cloud database per shared child, end-to-end encrypted | Accepted |
| [0005](0005-central-invite-only-relay.md) | One central, invite-only relay, multi-tenant by design | Accepted |
| [0006](0006-typst-for-pdf-export.md) | Typst for PDF export | Accepted |
| [0007](0007-database-actor-thread.md) | The vault runs as an actor on its own thread | Accepted |
| [0008](0008-note-search-without-turso-fts.md) | Search notes in Rust; Turso's full-text search waits | Accepted |
| [0009](0009-images-stored-in-the-vault.md) | Images live in the vault, in three versions, one row each | Accepted |
| [0010](0010-settings-file-and-themes.md) | Settings in a JSON file of their own, held as a GPUI global | Accepted |
| [0011](0011-command-palette-inside-the-main-view.md) | The command palette lives inside the main view and runs actions | Accepted |
| [0012](0012-vim-mode-as-a-keystroke-layer.md) | Vim mode is a keystroke layer over the text field, with a pure state machine | Accepted |
| [0013](0013-vault-encryption-key-file-and-backups.md) | Vault encryption with a wrapped key, and backups as one file | Accepted |
| [0014](0014-relay-api-tokens-and-local-sync.md) | The relay: hashed secrets, two provisioning backends, and a local sync server for development | Accepted |
| [0015](0015-sharing-keys-and-idempotent-projection.md) | Sharing: access keys, sealed rows, and a projection that is safe to repeat | Accepted |
| [0016](0016-child-mode-as-a-type-and-the-replica-as-offline-cache.md) | Child mode: a mode is a type, the replica is the offline cache, and the server travels with the key | Proposed |

Template: copy [0000-template.md](0000-template.md).
