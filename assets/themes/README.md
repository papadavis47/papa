# Bundled themes

These theme files are compiled into the app (`crates/papa/src/theme.rs`) and appear in Settings → Appearance. Each file is a gpui-kit theme set: a name and a list of themes, each with a `mode` (`light` or `dark`) and its colours. The schema is gpui-kit's [`.theme-schema.json`](https://github.com/longbridge/gpui-kit/blob/v0.7.1/.theme-schema.json).

| File | Themes | Source |
|---|---|---|
| `papa-paper.json` | Papa Paper Light, Papa Paper Dark | Papa's own: warm paper and ink, with a terracotta accent. The defaults. |
| `ayu.json` | Ayu Light, Ayu Dark | gpui-kit v0.7.1, from [Zed's Ayu](https://github.com/zed-industries/zed/tree/main/assets/themes/ayu) |
| `catppuccin.json` | Catppuccin Latte, Catppuccin Mocha | gpui-kit v0.7.1 (Frappé and Macchiato left out), palette by [Catppuccin](https://github.com/catppuccin/catppuccin) |
| `everforest.json` | Everforest Light, Everforest Dark | gpui-kit v0.7.1, palette by [sainnhe](https://github.com/sainnhe/everforest) |
| `flexoki.json` | Flexoki Light, Flexoki Dark | gpui-kit v0.7.1, palette by [kepano](https://github.com/kepano/flexoki) |
| `gruvbox.json` | Gruvbox Light, Gruvbox Dark | gpui-kit v0.7.1, palette by [Pavel Pertsev](https://github.com/morhetz/gruvbox) |
| `solarized.json` | Solarized Light, Solarized Dark | gpui-kit v0.7.1, palette by [Ethan Schoonover](https://ethanschoonover.com/solarized) |

The files from gpui-kit are copied unchanged from its [`themes/`](https://github.com/longbridge/gpui-kit/tree/v0.7.1/themes) folder at tag v0.7.1 (Apache-2.0).

## Your own themes

Put more theme files in the `themes` folder next to `settings.json` (`~/.config/papa/themes` on Linux, `~/Library/Application Support/papa/themes` on macOS; Settings → Data has a button that opens it). Papa reloads them as soon as a file changes. A theme there with the same name as a bundled one replaces it.
