//! Vim mode: modal editing for a note's text, on top of the text field.
//!
//! gpui-kit's text field has no Vim support, so this is a small layer of
//! our own. It's split so nearly all of it is plain Rust, tested without a
//! window:
//!
//! - [`text`]: the note as characters and lines.
//! - [`motion`]: where `w`, `$`, `gg` and the others move the cursor.
//! - [`parser`]: keys in, commands out (`2d3w` → delete six words).
//! - [`machine`]: the modes, and what each command does to the text, as
//!   [`Effect`]s.
//! - [`field`]: the only part that knows GPUI. It reads keystrokes and
//!   plays the effects on the text field.
//!
//! With the setting off, nothing here is attached to the editor at all.

pub mod field;
pub mod key;
pub mod machine;
pub mod motion;
pub mod parser;
pub mod text;

pub use field::VimLayer;
pub use machine::{Effect, Status};
