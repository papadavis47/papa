# 0018. Packaging: cargo-packager, a `dist` profile, and draft releases from tags

- **Status:** Accepted
- **Date:** 2026-10-16

## Context

Phase 14 turns `cargo run` into something a person can install (PLAN.md §8, §11). Forces at play:

- **The owner isn't ready to release.** The goal is scaffolding: everything in place and checked, nothing published until he decides.
- **Linux and macOS first, Windows later** (PLAN.md §1), but Windows is wanted, so it should be in the scaffolding from the start.
- **The binary is already self-contained.** Icons (`assets.rs`) and themes (`theme.rs`) are compiled in with `include_bytes!`/`include_str!`, Typst's fonts come embedded with `typst-kit`, and data lives in the OS folders `dirs` picks. There are no resource files to ship beside the executable.
- **What the binary needs from the system** (Linux, `readelf -d`): glibc, libgcc, libxcb, libxkbcommon and libxkbcommon-x11. GPUI loads Wayland, Vulkan and EGL with `dlopen`, so they don't show up as links but must be there at run time. On macOS, gpui-kit turns on GPUI's `runtime_shaders`, so building needs only the Xcode command line tools, not full Xcode.
- **No signing accounts yet.** No Apple Developer ID, no Windows code-signing certificate.
- **A plain `--release` build** is 154 MB on Linux: 72 MB of code, 22 MB of data (fonts, themes, icons) and 37 MB of symbols. It takes 9½ minutes from clean on the orb's 4 cores.
- **GitHub's macOS runner has 7 GB of memory.** `codegen-units = 1` ran out of memory on the 7 GB orb while compiling `turso_core`.

## Decision

- **cargo-packager** (0.11.8, pinned in CI) builds every format from one `[package.metadata.packager]` block in `crates/papa/Cargo.toml`: `.deb` and AppImage on Linux, `.app` and `.dmg` on macOS, an NSIS installer and a WiX MSI on Windows. Each is built on its own OS. Formats are chosen on the command line (`--formats`), because the platform defaults include ones Papa doesn't want (pacman).
- **A `dist` profile** for packages: `release` plus thin LTO and no symbols. Measured on Linux: 118 MB instead of 154 MB, built in about the same time. Thin LTO makes the code slightly *bigger* (more inlining across crates), so the saving is all symbols. `release` stays Cargo's default, symbols kept, for debugging a crash.
- **One icon source:** `crates/papa/icons/papa.svg`, Lucide's `book-heart` (the welcome screen's) on the Papa Paper terracotta. `render.py` (resvg plus the Python standard library) renders the Linux PNGs, `papa.icns` and `icon.ico` from it. They are committed, so packaging needs no SVG tool. The `.icns` is written directly rather than left to cargo-packager, which would name the Retina PNGs' Linux folders wrongly (`1024x1024@2`) and can't take a 1024 px image without one.
- **Identity:** product name "Papa", binary `papa`, bundle identifier `com.johnwilliamdavis.papa` (reverse DNS of a domain the owner holds). On Linux, windows set the Wayland app id `papa`, matching the installed `papa.desktop`, so the dock shows Papa's name and icon.
- **macOS is signed ad hoc** (`signing-identity = "-"`). The bundle is sealed, so a downloaded copy is "unverified" (System Settings → Privacy & Security → Open Anyway) rather than "damaged". Swapping in a Developer ID and the `APPLE_*` variables later adds real signing and notarisation without other changes.
- **Windows:** `build.rs` embeds `icon.ico` as resource 1 with `winresource` (only when the target is Windows), which is where GPUI's window and Explorer look for the icon. Release builds use the `windows` subsystem, so no console opens next to the app. Neither installer is signed.
- **Builds run on the oldest supported runners:** Ubuntu 22.04 (so the Linux binary needs glibc 2.35, not the 2.39 of newer runners), and macOS with `MACOSX_DEPLOYMENT_TARGET=11.0`. Only x86_64 Linux and Windows, and Apple silicon Macs, for now.
- **CI (`release.yml`):**
  - A pull request that touches packaging builds and checks every package and keeps them as workflow artifacts. A broken package shows up before a release does.
  - Only a `v*` tag makes a release, and only a **draft** one, with a `SHA256SUMS` file. The job first checks that the tag matches the version in `Cargo.toml`. Publishing stays a manual click.

## Consequences

- Nothing ships until the owner pushes a tag and publishes the draft. Until then the packages exist only as PR artifacts.
- Packaging is one command per OS, and the same command locally and in CI. On Linux the `.deb` is 48 MB and the AppImage 45 MB.
- **Unsigned downloads warn on both macOS and Windows.** Real signing needs an Apple Developer account (paid yearly) and a Windows certificate or a signing service.
- **The `.deb`'s dependency list is maintained by hand.** It has to change if GPUI starts linking something new. The AppImage carries its own libraries (xkbcommon among them), except the ones every desktop provides: glibc, libxcb, Wayland and the graphics drivers.
- **No auto-update.** cargo-packager can sign packages for its updater; that waits until there are releases to update from.
- **Not covered yet:** Intel Macs (a second macOS target, or a universal binary), Linux ARM, Flatpak, Homebrew or winget. Each is a matrix entry or a manifest when wanted.
- **Data on Windows** goes where `dirs` puts it: the vault and settings both under `%APPDATA%\papa`. Papa has never been run on Windows, so the first Windows build is the first test of everything else too.
