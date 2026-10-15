//! The Settings window: gpui-kit's `Settings` layout, with a sidebar of
//! pages and a search box, over the [`AppSettings`] global.
//!
//! Each field is a pair of closures: one reads the value from the global,
//! the other writes it back through [`AppSettings::update`]. Writing
//! notifies the global's observers, so the main window restyles at once
//! and this one redraws with the new value.
//!
//! The vault's buttons (password, lock, backup) are different: the vault
//! belongs to the main window, so they send it an action and bring it
//! forward, and read the [`VaultStatus`] global it publishes.

use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, IndexPath, Sizable as _, StyledExt as _,
    ThemeMode, TitleBar,
    button::Button,
    group_box::GroupBoxVariant,
    searchable_list::{SearchableListItem, SearchableVec},
    select::{Select, SelectEvent, SelectState},
    setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Action, AnyWindowHandle, App, AppContext as _, Context, Entity, Global, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, WindowBounds,
    WindowDecorations, WindowOptions, div, px, size,
};
use papa_core::paths;
use papa_core::settings::{
    self as defaults, Appearance, ColorMode, Density, NoteMode, PaperSize, Settings as PapaSettings,
};

use crate::app::{
    AccountStatus, ChangePassword, CreateAccount, ExportBackup, LockVault, RemovePassword,
    RestoreBackup, SignIn, SignOut, SyncNow, VaultStatus, dispatch_to_main,
};
use crate::settings::AppSettings;
use crate::theme;

/// The open Settings window, so a second ⌘, brings it to the front instead
/// of opening another.
struct SettingsWindow(AnyWindowHandle);

impl Global for SettingsWindow {}

/// Opens the Settings window, or brings it forward if it's open.
pub fn open(cx: &mut App) {
    if let Some(SettingsWindow(handle)) = cx.try_global::<SettingsWindow>() {
        // Fails once the window has been closed.
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(820.), px(600.)), cx)),
        window_min_size: Some(size(px(560.), px(400.))),
        window_decorations: Some(WindowDecorations::Client),
        ..TitleBar::window_options()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        window.set_window_title("Papa Settings");
        cx.new(|cx| SettingsPanel::new(window, cx))
    });
    match opened {
        Ok((handle, _)) => cx.set_global(SettingsWindow(handle)),
        Err(err) => crate::ui::notify_error("Couldn't open Settings", err, cx),
    }
}

/// The three fonts the father can choose.
#[derive(Clone, Copy)]
enum FontSlot {
    Ui,
    Reading,
    Writing,
}

impl FontSlot {
    fn get(self, appearance: &Appearance) -> &Option<String> {
        match self {
            Self::Ui => &appearance.ui_font,
            Self::Reading => &appearance.reading_font,
            Self::Writing => &appearance.writing_font,
        }
    }

    fn get_mut(self, appearance: &mut Appearance) -> &mut Option<String> {
        match self {
            Self::Ui => &mut appearance.ui_font,
            Self::Reading => &mut appearance.reading_font,
            Self::Writing => &mut appearance.writing_font,
        }
    }

    /// What `None` means for this font.
    fn default_label(self) -> &'static str {
        match self {
            Self::Ui => "System default",
            Self::Reading | Self::Writing => "Same as interface",
        }
    }
}

/// One entry of a font picker: a family, or the slot's default (`None`).
#[derive(Clone)]
struct FontChoice {
    title: SharedString,
    value: Option<SharedString>,
}

impl SearchableListItem for FontChoice {
    type Value = Option<SharedString>;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.value
    }
}

type FontPicker = Entity<SelectState<SearchableVec<FontChoice>>>;

/// The Settings window's root view.
///
/// An entity rather than a plain function of the global, because the font
/// pickers keep state between frames: whether the list is open, and what
/// is typed in its search box.
pub struct SettingsPanel {
    fonts: [(FontSlot, FontPicker); 3],
    _subscriptions: Vec<Subscription>,
}

impl SettingsPanel {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let installed = theme::installed_fonts(cx).to_vec();
        let mut subscriptions = Vec::new();
        let fonts = [FontSlot::Ui, FontSlot::Reading, FontSlot::Writing].map(|slot| {
            let picker = font_picker(slot, &installed, window, cx);
            subscriptions.push(cx.subscribe_in(
                &picker,
                window,
                move |_, _, event: &SelectEvent<SearchableVec<FontChoice>>, _, cx| {
                    let SelectEvent::Confirm(Some(font)) = event else {
                        return;
                    };
                    let font = font.as_ref().map(|font| font.to_string());
                    AppSettings::update(cx, |settings| {
                        *slot.get_mut(&mut settings.appearance) = font;
                    });
                },
            ));
            (slot, picker)
        });
        // Redraw on any change, wherever it came from: a field here, the
        // theme button in the main window, or a page's reset button. The
        // pickers hold their own selection, so bring them in line too.
        subscriptions.push(
            cx.observe_global_in::<AppSettings>(window, |this, window, cx| {
                this.sync_fonts(window, cx);
                cx.notify();
            }),
        );
        // The password buttons follow the vault: locked, encrypted or not.
        subscriptions.push(cx.observe_global::<VaultStatus>(|_, cx| cx.notify()));
        // The account buttons follow signing in and out.
        subscriptions.push(cx.observe_global::<AccountStatus>(|_, cx| cx.notify()));
        // A theme file added to the themes folder joins the lists.
        subscriptions
            .push(cx.observe_global::<gpui_kit::component::ThemeRegistry>(|_, cx| cx.notify()));
        Self {
            fonts,
            _subscriptions: subscriptions,
        }
    }

    fn sync_fonts(&self, window: &mut Window, cx: &mut Context<Self>) {
        for (slot, picker) in &self.fonts {
            let font: Option<SharedString> = slot
                .get(&AppSettings::get(cx).appearance)
                .clone()
                .map(Into::into);
            if picker.read(cx).selected_value() != Some(&font) {
                picker.update(cx, |picker, cx| {
                    picker.set_selected_value(&font, window, cx)
                });
            }
        }
    }

    fn picker(&self, slot: FontSlot) -> FontPicker {
        self.fonts[slot as usize].1.clone()
    }
}

/// A searchable list of the installed fonts, with the slot's default on top.
fn font_picker(
    slot: FontSlot,
    installed: &[SharedString],
    window: &mut Window,
    cx: &mut Context<SettingsPanel>,
) -> FontPicker {
    let default = FontChoice {
        title: slot.default_label().into(),
        value: None,
    };
    let choices: Vec<_> = std::iter::once(default)
        .chain(installed.iter().map(|font| FontChoice {
            title: font.clone(),
            value: Some(font.clone()),
        }))
        .collect();
    // A font that isn't installed (any more) shows as the default, which is
    // what's drawn in its place.
    let current = slot.get(&AppSettings::get(cx).appearance).as_deref();
    let row = current
        .and_then(|font| installed.iter().position(|name| name == font))
        .map_or(0, |ix| ix + 1);
    cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(choices),
            Some(IndexPath::default().row(row)),
            window,
            cx,
        )
        .searchable(true)
    })
}

impl Render for SettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(TitleBar::new().child(div().text_sm().font_semibold().child("Settings")))
            .child(
                div().flex_1().min_h_0().child(
                    Settings::new("settings")
                        .with_group_variant(GroupBoxVariant::Outline)
                        .sidebar_width(px(200.))
                        .pages([
                            self.appearance_page(cx),
                            editor_page(),
                            privacy_page(),
                            sharing_page(),
                            data_page(),
                        ]),
                ),
            )
    }
}

// --- Pages ---

/// Reads one setting.
fn get<T>(read: impl Fn(&PapaSettings) -> T + 'static) -> impl Fn(&App) -> T + 'static {
    move |cx| read(AppSettings::get(cx))
}

/// Writes one setting.
fn set<T>(write: impl Fn(&mut PapaSettings, T) + 'static) -> impl Fn(T, &mut App) + 'static {
    move |value, cx| AppSettings::update(cx, |settings| write(settings, value))
}

fn options(pairs: &[(&'static str, &'static str)]) -> Vec<(SharedString, SharedString)> {
    pairs
        .iter()
        .map(|&(value, label)| (value.into(), label.into()))
        .collect()
}

/// A dropdown of the registered themes for `mode`.
fn theme_options(mode: ThemeMode, cx: &App) -> Vec<(SharedString, SharedString)> {
    theme::theme_names(mode, cx)
        .into_iter()
        .map(|name| (name.clone(), name))
        .collect()
}

/// Rounded to hundredths, so 1.6 (an `f32`) doesn't show as 1.600000023841858.
fn round2(value: f32) -> f64 {
    (f64::from(value) * 100.).round() / 100.
}

impl SettingsPanel {
    fn appearance_page(&self, cx: &App) -> SettingPage {
        let mode = SettingField::dropdown(
            options(&[
                ("system", "Match system"),
                ("light", "Light"),
                ("dark", "Dark"),
            ]),
            get(|s| {
                match s.appearance.mode {
                    ColorMode::System => "system",
                    ColorMode::Light => "light",
                    ColorMode::Dark => "dark",
                }
                .into()
            }),
            set(|s, value: SharedString| {
                s.appearance.mode = match value.as_ref() {
                    "light" => ColorMode::Light,
                    "dark" => ColorMode::Dark,
                    _ => ColorMode::System,
                }
            }),
        )
        .default_value("system");
        let light_theme = SettingField::scrollable_dropdown(
            theme_options(ThemeMode::Light, cx),
            get(|s| s.appearance.light_theme.clone().into()),
            set(|s, value: SharedString| s.appearance.light_theme = value.into()),
        )
        .default_value(defaults::DEFAULT_LIGHT_THEME);
        let dark_theme = SettingField::scrollable_dropdown(
            theme_options(ThemeMode::Dark, cx),
            get(|s| s.appearance.dark_theme.clone().into()),
            set(|s, value: SharedString| s.appearance.dark_theme = value.into()),
        )
        .default_value(defaults::DEFAULT_DARK_THEME);
        let density = SettingField::dropdown(
            options(&[("comfortable", "Comfortable"), ("compact", "Compact")]),
            get(|s| {
                match s.appearance.density {
                    Density::Comfortable => "comfortable",
                    Density::Compact => "compact",
                }
                .into()
            }),
            set(|s, value: SharedString| {
                s.appearance.density = match value.as_ref() {
                    "compact" => Density::Compact,
                    _ => Density::Comfortable,
                }
            }),
        )
        .default_value("comfortable");
        let reduce_motion = SettingField::switch(
            get(|s| s.appearance.reduce_motion),
            set(|s, value: bool| s.appearance.reduce_motion = value),
        )
        .default_value(false);
        let text_size = SettingField::number_input(
            NumberFieldOptions {
                min: f64::from(*defaults::TEXT_SIZES.start()),
                max: f64::from(*defaults::TEXT_SIZES.end()),
                step: 1.,
            },
            get(|s| round2(s.appearance.text_size)),
            set(|s, value: f64| s.appearance.text_size = value as f32),
        )
        .default_value(f64::from(defaults::DEFAULT_TEXT_SIZE));
        let line_height = SettingField::number_input(
            NumberFieldOptions {
                min: f64::from(*defaults::LINE_HEIGHTS.start()),
                max: f64::from(*defaults::LINE_HEIGHTS.end()),
                step: 0.1,
            },
            get(|s| round2(s.appearance.line_height)),
            set(|s, value: f64| s.appearance.line_height = value as f32),
        )
        .default_value(round2(defaults::DEFAULT_LINE_HEIGHT));

        SettingPage::new("Appearance")
            .icon(IconName::Palette)
            .default_open(true)
            .group(
                SettingGroup::new()
                    .title("Theme")
                    .item(
                        SettingItem::new("Mode", mode)
                            .description("Light, dark, or whichever your system uses."),
                    )
                    .item(SettingItem::new("Light theme", light_theme))
                    .item(SettingItem::new("Dark theme", dark_theme))
                    .item(
                        SettingItem::new(
                            "Your themes",
                            open_folder_button(paths::themes_dir().ok()),
                        )
                        .description(
                            "Theme files you put in this folder appear above. \
                                 Papa reloads them as soon as they change.",
                        )
                        .keywords(["custom", "folder", "json"]),
                    ),
            )
            .group(
                SettingGroup::new()
                    .title("Text")
                    .item(
                        SettingItem::new("Interface font", self.font_field(FontSlot::Ui))
                            .keywords(["typeface", "family"]),
                    )
                    .item(
                        SettingItem::new("Reading font", self.font_field(FontSlot::Reading))
                            .description("For notes in Preview.")
                            .keywords(["typeface", "family", "preview"]),
                    )
                    .item(
                        SettingItem::new("Writing font", self.font_field(FontSlot::Writing))
                            .description("For notes while you edit them.")
                            .keywords(["typeface", "family", "editor", "monospace"]),
                    )
                    .item(
                        SettingItem::new("Text size", text_size)
                            .description("In pixels. Everything in the window scales with it.")
                            .keywords(["font size", "zoom", "scale"]),
                    )
                    .item(
                        SettingItem::new("Line height", line_height)
                            .description("Space between the lines of a note.")
                            .keywords(["spacing", "leading"]),
                    ),
            )
            .group(
                SettingGroup::new()
                    .title("Layout")
                    .item(
                        SettingItem::new("Density", density)
                            .description("Compact shows one line per note in the list.")
                            .keywords(["spacing", "compact", "comfortable"]),
                    )
                    .item(
                        SettingItem::new("Reduce motion", reduce_motion)
                            .description("Show notes and panels at once, without animating them.")
                            .keywords(["animation", "accessibility", "fade"]),
                    ),
            )
    }

    /// A font picker as a settings field, with a reset to the default.
    fn font_field(&self, slot: FontSlot) -> SettingField<SharedString> {
        let picker = self.picker(slot);
        SettingField::render(move |_, _, _| Select::new(&picker).small().w(px(220.))).on_reset(
            move |cx| slot.get(&AppSettings::get(cx).appearance).is_some(),
            move |_, cx| {
                AppSettings::update(cx, |s| *slot.get_mut(&mut s.appearance) = None);
            },
        )
    }
}

fn editor_page() -> SettingPage {
    let open_in = SettingField::dropdown(
        options(&[("preview", "Preview"), ("edit", "Edit")]),
        get(|s| {
            match s.editor.open_notes_in {
                NoteMode::Preview => "preview",
                NoteMode::Edit => "edit",
            }
            .into()
        }),
        set(|s, value: SharedString| {
            s.editor.open_notes_in = match value.as_ref() {
                "edit" => NoteMode::Edit,
                _ => NoteMode::Preview,
            }
        }),
    )
    .default_value("preview");
    let autosave = SettingField::number_input(
        NumberFieldOptions {
            min: *defaults::AUTOSAVE_DELAYS.start() as f64,
            max: *defaults::AUTOSAVE_DELAYS.end() as f64,
            step: 100.,
        },
        get(|s| s.editor.autosave_delay_ms as f64),
        set(|s, value: f64| s.editor.autosave_delay_ms = value.round() as u64),
    )
    .default_value(defaults::DEFAULT_AUTOSAVE_DELAY as f64);
    let vim_mode = SettingField::switch(
        get(|s| s.editor.vim_mode),
        set(|s, value: bool| s.editor.vim_mode = value),
    )
    .default_value(false);

    SettingPage::new("Editor").icon(IconName::FileText).group(
        SettingGroup::new()
            .item(
                SettingItem::new("Open notes in", open_in)
                    .description("A new note always opens for editing.")
                    .keywords(["preview", "edit", "mode"]),
            )
            .item(
                SettingItem::new("Autosave delay", autosave)
                    .description("Milliseconds of quiet before what you type is saved.")
                    .keywords(["save", "debounce"]),
            )
            .item(
                SettingItem::new("Vim mode", vim_mode)
                    .description(
                        "Edit notes with Vim's keys. Press i to type, Esc to stop, :w to save.",
                    )
                    .keywords(["vi", "modal", "keys", "normal", "insert"]),
            ),
    )
}

fn privacy_page() -> SettingPage {
    let auto_lock = SettingField::dropdown(
        options(&[
            ("0", "Never"),
            ("1", "After 1 minute"),
            ("5", "After 5 minutes"),
            ("15", "After 15 minutes"),
            ("30", "After 30 minutes"),
            ("60", "After 1 hour"),
        ]),
        get(|s| s.privacy.auto_lock_minutes.to_string().into()),
        set(|s, value: SharedString| {
            s.privacy.auto_lock_minutes =
                value.parse().unwrap_or(defaults::DEFAULT_AUTO_LOCK_MINUTES)
        }),
    )
    .default_value(SharedString::from(
        defaults::DEFAULT_AUTO_LOCK_MINUTES.to_string(),
    ));

    SettingPage::new("Privacy").icon(IconName::Eye).group(
        SettingGroup::new()
            .title("Vault")
            .description(
                "With a password, your vault is encrypted on disk: without the \
                     password, its file is unreadable. Nobody can recover a forgotten password.",
            )
            .item(
                SettingItem::new("Password", password_buttons())
                    .description(
                        "Set one to encrypt your vault. Change it, or remove it to \
                             decrypt the vault.",
                    )
                    .keywords(["encrypt", "encryption", "lock", "security", "passphrase"]),
            )
            .item(
                SettingItem::new(
                    "Lock now",
                    vault_button("lock-now", "Lock", &LockVault, |status| {
                        status == VaultStatus::Open { encrypted: true }
                    }),
                )
                .description("Close the vault until the password is typed again.")
                .keywords(["lock", "close"]),
            )
            .item(
                SettingItem::new("Auto-lock", auto_lock)
                    .description(
                        "Lock after this long without a key press or the mouse moving. \
                             Only a vault with a password locks.",
                    )
                    .keywords(["idle", "timeout", "lock", "minutes"]),
            ),
    )
}

/// "Set password…", or "Change…" and "Remove…" once there is one.
fn password_buttons() -> SettingField<SharedString> {
    SettingField::render(move |_, _, cx| {
        let status = *cx.global::<VaultStatus>();
        let open = matches!(status, VaultStatus::Open { .. });
        let encrypted = status == VaultStatus::Open { encrypted: true };
        let button = |id: &'static str, label: &'static str, action: &'static dyn Action| {
            Button::new(id)
                .outline()
                .small()
                .label(label)
                .disabled(!open)
                .on_click(move |_, _, cx| dispatch_to_main(action, cx))
        };
        gpui_kit::component::h_flex()
            .gap_2()
            .when(!encrypted, |row| {
                row.child(button("set-password", "Set password…", &ChangePassword))
            })
            .when(encrypted, |row| {
                row.child(button("change-password", "Change…", &ChangePassword))
                    .child(button("remove-password", "Remove…", &RemovePassword))
            })
    })
}

/// A button that runs `action` in the main window, enabled when `enabled`
/// says so for the vault's state.
fn vault_button(
    id: &'static str,
    label: &'static str,
    action: &'static dyn Action,
    enabled: fn(VaultStatus) -> bool,
) -> SettingField<SharedString> {
    SettingField::render(move |_, _, cx| {
        Button::new(id)
            .outline()
            .small()
            .label(label)
            .disabled(!enabled(*cx.global::<VaultStatus>()))
            .on_click(move |_, _, cx| dispatch_to_main(action, cx))
    })
}

fn sharing_page() -> SettingPage {
    SettingPage::new("Sharing")
        .icon(IconName::Globe)
        .resettable(false)
        .group(
            SettingGroup::new()
                .title("Papa account")
                .description(
                    "Sharing notes with your children goes through a Papa server. Every \
                     shared note is encrypted on this computer first: the server only ever \
                     sees ciphertext, and never your labels or unshared notes.",
                )
                .item(
                    SettingItem::new("Account", account_buttons())
                        .description(
                            "Accounts are by invitation. Your children sign in with your email \
                             and the key Papa makes for each of them.",
                        )
                        .keywords(["sign in", "log in", "sign up", "server", "relay", "email"]),
                )
                .item(
                    SettingItem::new(
                        "Sync",
                        SettingField::render(|_, _, cx| {
                            let status = cx.global::<AccountStatus>();
                            Button::new("sync-now")
                                .outline()
                                .small()
                                .label("Sync now")
                                .disabled(status.signed_in.is_none() || status.shares == 0)
                                .on_click(|_, _, cx| dispatch_to_main(&SyncNow, cx))
                        }),
                    )
                    .description(
                        "Papa syncs shared notes a few seconds after each change, and when \
                         it opens. To share with a child, right-click them in the sidebar \
                         and choose Sharing….",
                    )
                    .keywords(["sync", "share", "cloud", "upload"]),
                ),
        )
}

/// "Sign in…" and "Create account…", or who's signed in and "Sign out".
fn account_buttons() -> SettingField<SharedString> {
    SettingField::render(move |_, _, cx| {
        let status = cx.global::<AccountStatus>().clone();
        let button = |id: &'static str, label: &'static str, action: &'static dyn Action| {
            Button::new(id)
                .outline()
                .small()
                .label(label)
                .disabled(!status.vault_open)
                .on_click(move |_, _, cx| dispatch_to_main(action, cx))
        };
        gpui_kit::component::h_flex()
            .gap_2()
            .when(status.signed_in.is_none(), |row| {
                row.child(button("sign-in", "Sign in…", &SignIn))
                    .child(button("create-account", "Create account…", &CreateAccount))
            })
            .when_some(status.signed_in.clone(), |row, (email, server)| {
                row.child(
                    v_flex().items_end().text_sm().child(email).child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(server),
                    ),
                )
                .child(button("sign-out", "Sign out", &SignOut))
            })
    })
}

fn data_page() -> SettingPage {
    let vault = paths::vault_path().ok();
    let settings_file = paths::settings_path().ok();
    let describe = |path: &Option<PathBuf>| match path {
        Some(path) => paths::display(path),
        None => "Not available on this system".to_owned(),
    };

    let paper = SettingField::dropdown(
        options(&[("a4", "A4"), ("letter", "US Letter")]),
        get(|s| {
            match s.export.paper {
                PaperSize::A4 => "a4",
                PaperSize::Letter => "letter",
            }
            .into()
        }),
        set(|s, value: SharedString| {
            s.export.paper = match value.as_ref() {
                "letter" => PaperSize::Letter,
                _ => PaperSize::A4,
            }
        }),
    )
    .default_value("a4");

    SettingPage::new("Data")
        .icon(IconName::HardDrive)
        .resettable(false)
        .group(
            SettingGroup::new().title("PDF export").item(
                SettingItem::new("Paper size", paper)
                    .description("The page size of exported notes and books.")
                    .keywords(["pdf", "print", "a4", "letter", "page"]),
            ),
        )
        .group(
            SettingGroup::new()
                .title("Backup")
                .description(
                    "A backup is one file with everything in your vault. Keep it on another \
                     disk. A backup of an encrypted vault is encrypted too, with the \
                     password the vault had that day.",
                )
                .item(
                    SettingItem::new(
                        "Back up",
                        vault_button("export-backup", "Back up…", &ExportBackup, |status| {
                            matches!(status, VaultStatus::Open { .. })
                        }),
                    )
                    .description("Save a backup of your vault now.")
                    .keywords(["backup", "export", "copy", "save"]),
                )
                .item(
                    SettingItem::new(
                        "Restore",
                        vault_button("restore-backup", "Restore…", &RestoreBackup, |_| true),
                    )
                    .description(
                        "Replace your vault with a backup. The vault it replaces is kept next to it.",
                    )
                    .keywords(["backup", "import", "restore", "recover"]),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Files")
                .description("Your vault and your settings are kept apart: the settings apply before the vault opens.")
                .item(
                    SettingItem::new("Vault", reveal_button(vault.clone()))
                        .description(describe(&vault))
                        .keywords(["database", "location", "path"]),
                )
                .item(
                    SettingItem::new("Settings file", reveal_button(settings_file.clone()))
                        .description(describe(&settings_file))
                        .keywords(["json", "location", "path"]),
                ),
        )
}

/// Shows `path` in the file manager.
fn reveal_button(path: Option<PathBuf>) -> SettingField<SharedString> {
    SettingField::render(move |options, _, _| {
        let path = path.clone();
        Button::new("reveal")
            .outline()
            .small()
            .label("Show in folder")
            .disabled(options.is_disabled() || path.is_none())
            .on_click(move |_, _, cx| {
                if let Some(path) = &path {
                    cx.reveal_path(path);
                }
            })
    })
}

/// Opens `dir` in the file manager, creating it first if needed.
fn open_folder_button(dir: Option<PathBuf>) -> SettingField<SharedString> {
    SettingField::render(move |_, _, _| {
        let dir = dir.clone();
        Button::new("open-themes")
            .outline()
            .small()
            .icon(IconName::FolderOpen)
            .label("Open folder")
            .disabled(dir.is_none())
            .on_click(move |_, _, cx| {
                if let Some(dir) = &dir {
                    match std::fs::create_dir_all(dir) {
                        Ok(()) => crate::open::open(dir, cx),
                        Err(err) => crate::ui::notify_error("Couldn't open the folder", err, cx),
                    }
                }
            })
    })
}
