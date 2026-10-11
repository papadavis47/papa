# AGENTS.md

## Project
`papa` — Rust (edition 2024) desktop app built on GPUI via [gpui-kit](https://gpui-kit.com).

## Dependencies
- Only dep: `gpui-kit`. It bundles gpui (`gpui-pre`), gpui_platform (font-kit/x11/wayland), gpui-base, gpui-component, assets at matching versions.
- Never add `gpui`, `gpui_platform`, or `gpui-component` as separate deps — creates a second, incompatible gpui crate.
- Import everything through the facade: `use gpui_kit::*;`, components via `gpui_kit::component::...`.
- App entry: `gpui_kit::application().run(|cx| { gpui_kit::init(cx); ... })`. `init` required before using components.
- `Cargo.lock` committed; bump deps deliberately with `cargo update`.

## Commands
- Build: `cargo build`
- Run: `cargo run`
- Lint: `cargo clippy`

## Off-limits
- `learning/` is the owner's private notes, gitignored on purpose. Don't read, edit, cite, or rely on it unless the user explicitly asks you to use a specific file there.
