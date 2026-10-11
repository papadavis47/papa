# 0002. Cargo workspace; gpui-kit is the only UI dependency

- **Status:** Accepted
- **Date:** 2026-10-11

## Context

The app needs a UI, storage, crypto, PDF export and a server. GPUI is published as a family of `gpui-pre-*` crates that must move together. Adding `gpui`, `gpui_platform` or `gpui-component` next to `gpui-kit` would pull in a second, incompatible copy of GPUI.

## Decision

- Split the code into a Cargo workspace: `papa` (app), `papa-core`, `papa-proto`, `papa-pdf` and `papa-relay`.
- `gpui-kit` is the only UI dependency, and only `papa` depends on it. Import everything through the facade (`use gpui_kit::*;`).
- Non-UI crates are added deliberately to the crate that needs them.

## Consequences

- `papa-core` can be tested without a window, and the server never compiles GPUI.
- Heavy dependencies (Typst) stay isolated, which keeps incremental builds fast.
- Upgrading GPUI is a single `gpui-kit` bump.
