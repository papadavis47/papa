//! Papa: a private, local-first journal for fathers.

mod app;
mod placeholder;
mod state;
mod theme;
mod ui;

use gpui_kit::component::TitleBar;
use gpui_kit::{AppContext as _, WindowBounds, WindowDecorations, WindowOptions, px, size};

fn main() {
    gpui_kit::application()
        // The icon SVGs used by the components (and by us).
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            app::init(cx);

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1100.), px(720.)), cx)),
                window_min_size: Some(size(px(720.), px(480.))),
                // On Linux, ask the compositor not to draw its own title bar on
                // top of ours. macOS and Windows ignore this.
                window_decorations: Some(WindowDecorations::Client),
                // We draw our own title bar, so let it own dragging and double-clicks.
                ..TitleBar::window_options()
            };

            // `open_window` wraps our view in gpui-kit's `Root`, which hosts
            // dialogs, sheets and notifications above it.
            gpui_kit::open_window(options, cx, |window, cx| {
                window.set_window_title("Papa");
                theme::follow_system(window, cx);
                cx.new(|cx| app::PapaApp::new(window, cx))
            })
            .expect("failed to open the main window");
        });
}
