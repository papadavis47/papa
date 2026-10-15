//! Dialogs for adding, editing and deleting children and labels, for the
//! vault's password, and for the father's Papa account.
//!
//! Each takes an `on_save`/`on_delete` callback instead of touching app state
//! itself, like the other components in `ui/`. Names are checked here with the
//! same rules `papa-core` enforces, so a mistake keeps the dialog open with a
//! message instead of failing after it closed.

use std::rc::Rc;

use gpui_kit::component::{
    Disableable as _, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
};
use gpui_kit::{
    App, AppContext as _, ClickEvent, Entity, ParentElement as _, SharedString, Window, px,
};
use papa_core::model::{clean_child_name, clean_label_name};
use papa_core::{Child, ChildDraft, Error, Label};

use super::account_form::{AccountEntry, AccountForm, AccountPurpose};
use super::name_form::NameForm;
use super::password_form::{PasswordEntry, PasswordForm, PasswordPurpose};
use super::sentence;

/// Opens the "Add child" dialog, or "Edit child" when `child` is given.
pub fn child_dialog(
    child: Option<&Child>,
    color: u32,
    on_save: impl Fn(&ChildDraft, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let (title, name) = match child {
        Some(child) => ("Edit child", child.name.as_str()),
        None => ("Add child", ""),
    };
    let form = cx.new(|cx| NameForm::new(name, Some(color), window, cx));

    open_form(title, form, window, cx, move |form, window, cx| {
        let (name, chosen) = form.read(cx).values(cx);
        let draft = ChildDraft {
            name: clean_child_name(&name).map_err(sentence)?,
            color: chosen.unwrap_or(color),
        };
        on_save(&draft, window, cx);
        Ok(())
    });
}

/// Opens the "Add label" dialog, or "Rename label" when `label` is given.
/// `taken` holds the names of the other labels.
pub fn label_dialog(
    label: Option<&Label>,
    taken: Vec<String>,
    on_save: impl Fn(&String, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let (title, name) = match label {
        Some(label) => ("Rename label", label.name.as_str()),
        None => ("Add label", ""),
    };
    let form = cx.new(|cx| NameForm::new(name, None, window, cx));

    open_form(title, form, window, cx, move |form, window, cx| {
        let (name, _) = form.read(cx).values(cx);
        let name = clean_label_name(&name).map_err(sentence)?;
        // The same comparison as the database's NOCASE: ASCII only.
        if let Some(existing) = taken.iter().find(|t| t.eq_ignore_ascii_case(&name)) {
            return Err(sentence(Error::DuplicateLabel(existing.clone())));
        }
        on_save(&name, window, cx);
        Ok(())
    });
}

/// Asks before deleting. `on_delete` runs only if the user confirms.
pub fn confirm_delete(
    title: String,
    description: &'static str,
    on_delete: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    confirm_danger(title, description, "Delete", on_delete, window, cx);
}

/// Asks before something that can't be undone: `ok` names it on a red
/// button, next to Cancel.
pub fn confirm_danger(
    title: String,
    description: &'static str,
    ok: &'static str,
    on_ok: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let title = SharedString::from(title);
    let on_ok = Rc::new(on_ok);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on_ok = on_ok.clone();
        alert
            .title(title.clone())
            .description(description)
            .confirm()
            .ok_text(ok)
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |event, window, cx| {
                on_ok(event, window, cx);
                true
            })
    });
}

/// Opens a dialog around `form` with Cancel and Save buttons.
///
/// `submit` runs on Save (or Enter). `Ok` closes the dialog; `Err` shows the
/// message under the field and keeps the dialog open.
fn open_form(
    title: &'static str,
    form: Entity<NameForm>,
    window: &mut Window,
    cx: &mut App,
    submit: impl Fn(&Entity<NameForm>, &mut Window, &mut App) -> Result<(), String> + 'static,
) {
    let submit = Rc::new(submit);
    let name_input = form.read(cx).name_input();

    // The builder runs on every render of the dialog, so it clones what it
    // needs rather than moving it.
    window.open_dialog(cx, move |dialog, _, _| {
        let form = form.clone();
        let submit = submit.clone();
        dialog
            .title(title)
            .w(px(380.))
            .child(form.clone())
            .footer(
                DialogFooter::new()
                    .child(DialogClose::new().trigger(|button| button.label("Cancel")))
                    .child(DialogAction::new().child(Button::new("save").primary().label("Save"))),
            )
            .on_ok(move |_, window, cx| match submit(&form, window, cx) {
                Ok(()) => true,
                Err(message) => {
                    form.update(cx, |form, cx| form.set_error(message, cx));
                    false
                }
            })
    });
    // Type straight away, without clicking the field first.
    name_input.update(cx, |input, cx| input.focus(window, cx));
}

/// Opens a password dialog. `on_submit` gets the passwords once they pass
/// the dialog's own checks, and starts the work.
///
/// The dialog stays open, busy, while the vault works: an Argon2id
/// derivation, maybe a rewrite of the whole vault. `on_submit`'s work ends
/// it with `PasswordForm::set_error` (try again) or `window.close_dialog`.
/// While busy it can't be cancelled, so the dialog that closes is this one.
pub fn password_dialog(
    purpose: PasswordPurpose,
    note: Option<SharedString>,
    on_submit: impl Fn(PasswordEntry, Entity<PasswordForm>, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let form = cx.new(|cx| PasswordForm::new(purpose, note, window, cx));
    let first = form.read(cx).first_input();
    let on_submit = Rc::new(on_submit);
    let danger = matches!(purpose, PasswordPurpose::Remove | PasswordPurpose::Restore);

    window.open_dialog(cx, move |dialog, _, cx| {
        let form = form.clone();
        let on_submit = on_submit.clone();
        let busy = form.read(cx).is_busy();
        let ok = Button::new("ok")
            .label(purpose.ok_label())
            .loading(busy)
            .with_variant(if danger {
                ButtonVariant::Danger
            } else {
                ButtonVariant::Primary
            });
        dialog
            .title(purpose.title())
            .w(px(420.))
            .close_button(!busy)
            .keyboard(!busy)
            .overlay_closable(!busy)
            .child(form.clone())
            .footer(
                DialogFooter::new()
                    .child(
                        DialogClose::new()
                            .trigger(move |button| button.label("Cancel").disabled(busy)),
                    )
                    .child(DialogAction::new().child(ok)),
            )
            .on_ok(move |_, window, cx| {
                if form.read(cx).is_busy() {
                    return false;
                }
                match form.read(cx).entry(cx) {
                    Ok(entry) => {
                        form.update(cx, |form, cx| form.set_busy(cx));
                        redraw_dialogs(window, cx);
                        on_submit(entry, form.clone(), window, cx);
                    }
                    Err(message) => form.update(cx, |form, cx| form.set_error(message, cx)),
                }
                false
            })
    });
    first.update(cx, |input, cx| input.focus(window, cx));
}

/// Opens the dialog to sign in, or to create an account. Like
/// [`password_dialog`], it stays open and busy while `on_submit`'s work
/// talks to the server, which ends it with `AccountForm::set_error` or
/// `window.close_dialog`.
pub fn account_dialog(
    purpose: AccountPurpose,
    server: String,
    email: String,
    on_submit: impl Fn(AccountEntry, Entity<AccountForm>, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let form = cx.new(|cx| AccountForm::new(purpose, server, email, window, cx));
    let first = form.read(cx).first_input(cx);
    let on_submit = Rc::new(on_submit);

    window.open_dialog(cx, move |dialog, _, cx| {
        let form = form.clone();
        let on_submit = on_submit.clone();
        let busy = form.read(cx).is_busy();
        dialog
            .title(purpose.title())
            .w(px(420.))
            .close_button(!busy)
            .keyboard(!busy)
            .overlay_closable(!busy)
            .child(form.clone())
            .footer(
                DialogFooter::new()
                    .child(
                        DialogClose::new()
                            .trigger(move |button| button.label("Cancel").disabled(busy)),
                    )
                    .child(
                        DialogAction::new().child(
                            Button::new("ok")
                                .primary()
                                .label(purpose.ok_label())
                                .loading(busy),
                        ),
                    ),
            )
            .on_ok(move |_, window, cx| {
                if form.read(cx).is_busy() {
                    return false;
                }
                match form.read(cx).entry(cx) {
                    Ok(entry) => {
                        form.update(cx, |form, cx| form.set_busy(cx));
                        redraw_dialogs(window, cx);
                        on_submit(entry, form.clone(), window, cx);
                    }
                    Err(message) => form.update(cx, |form, cx| form.set_error(message, cx)),
                }
                false
            })
    });
    first.update(cx, |input, cx| input.focus(window, cx));
}

/// Rebuilds the open dialogs. A dialog's buttons are built by its builder
/// closure, which runs when the window's `Root` renders, not when the form
/// inside changes: after the form turns busy or idle, ask `Root` to render.
///
/// Deferred, because this may run inside one of `Root`'s own handlers
/// (the dialog's OK), and an entity can't be updated while it's updating.
pub fn redraw_dialogs(window: &mut Window, cx: &mut App) {
    window.defer(cx, |window, cx| {
        gpui_kit::component::Root::update(window, cx, |_, _, cx| cx.notify());
    });
}
