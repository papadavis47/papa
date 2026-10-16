# App icon

`papa.svg` is the source. Everything else here is rendered from it by
`render.py` and committed, so packaging needs no SVG tools:

- `32x32.png` … `512x512.png`: the Linux icons (`.deb` and AppImage);
- `papa.icns`: macOS, 16 to 1024 px including the Retina sizes;
- `icon.ico`: Windows, 16 to 256 px. `build.rs` also embeds it in `papa.exe`.

The glyph is [Lucide](https://lucide.dev)'s `book-heart`, the icon on Papa's
welcome screen, used under the [ISC license](https://lucide.dev/license).
The tile is the Papa Paper theme's terracotta, inside the 824 px rounded
square that macOS icons use on a 1024 px canvas.

After editing the SVG, render the rest again. It needs
[resvg](https://github.com/linebender/resvg) on the `PATH` (or in `$RESVG`):

```sh
python3 crates/papa/icons/render.py
```
