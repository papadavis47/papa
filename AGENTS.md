# AGENTS.md

## Project
`papa` — Rust (edition 2024) desktop app built on GPUI via [gpui-kit](https://gpui-kit.com). Roadmap and design: `PLAN.md`; decisions: `docs/adr/`.

## Layout
Cargo workspace; root `Cargo.toml` holds shared package metadata, lints and path deps.
- `crates/papa` — desktop app (only crate using gpui-kit); default member, so `cargo run` launches it.
- `crates/papa-core` — domain, Turso storage, crypto, sharing. No GPUI.
- `crates/papa-proto` — serde API types shared by app and relay.
- `crates/papa-pdf` — Markdown → PDF.
- `crates/papa-relay` — server binary.

## Dependencies
- Only UI dep: `gpui-kit`, and only the `papa` app crate depends on it. It bundles gpui (`gpui-pre`), gpui_platform (font-kit/x11/wayland), gpui-base, gpui-component, assets at matching versions.
- Never add `gpui`, `gpui_platform`, or `gpui-component` as separate deps — creates a second, incompatible gpui crate.
- Import everything through the facade: `use gpui_kit::*;`, components via `gpui_kit::component::...`.
- App entry: `gpui_kit::application().run(|cx| { gpui_kit::init(cx); ... })`. `init` required before using components.
- Non-UI crates (storage, crypto, serialization, PDF, HTTP) go in `papa-core`, `papa-pdf`, `papa-relay`, `papa-proto` — add them deliberately.
- `Cargo.lock` committed; bump deps deliberately with `cargo update`.

## Commands
- Build: `cargo build --workspace`
- Run app: `cargo run`
- Test: `cargo test --workspace`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings` (CI fails on warnings)
- Format: `cargo fmt --all` (CI runs `--check`)

## Public repo
- Never commit secrets, `.env` files, databases, or real family data. Config secrets come from env vars; document names in `.env.example`.
- CI runs gitleaks on every push and PR.

## Off-limits
- `learning/` is the owner's private notes, gitignored on purpose. Don't read, edit, cite, or rely on it unless the user explicitly asks you to use a specific file there.
