//! Papa: a private, local-first journal for fathers.

mod app;
mod assets;
mod child_app;
mod export;
mod mode;
mod palette;
mod settings;
mod settings_window;
mod state;
mod theme;
mod ui;
mod vim;

use gpui_kit::component::TitleBar;
use gpui_kit::{AppContext as _, WindowBounds, WindowDecorations, WindowOptions, px, size};

fn main() {
    gpui_kit::application()
        // The icon SVGs used by the components and by us.
        .with_assets(assets::PapaAssets)
        .run(|cx| {
            gpui_kit::init(cx);
            // Settings first: the theme is built from them, and both are in
            // place before the window draws its first frame.
            let settings_problem = settings::init(cx);
            theme::init(cx);
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
            // dialogs, sheets and notifications above it. Ours picks the
            // mode: the father's vault, or a child's shared notes.
            let (window, _) = gpui_kit::open_window(options, cx, |window, cx| {
                window.set_window_title("Papa");
                cx.new(|cx| mode::PapaWindow::new(settings_problem, window, cx))
            })
            .expect("failed to open the main window");
            app::quit_with(window, cx);
        });
}
