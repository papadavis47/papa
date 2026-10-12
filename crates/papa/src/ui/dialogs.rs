//! Dialogs for adding, editing and deleting children and labels.
//!
//! Each takes an `on_save`/`on_delete` callback instead of touching app state
//! itself, like the other components in `ui/`. Names are checked here with the
//! same rules `papa-core` enforces, so a mistake keeps the dialog open with a
//! message instead of failing after it closed.

use std::rc::Rc;

use gpui_kit::component::{
    WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
};
use gpui_kit::{
    App, AppContext as _, ClickEvent, Entity, ParentElement as _, SharedString, Window, px,
};
use papa_core::model::{clean_child_name, clean_label_name};
use papa_core::{Child, ChildDraft, Error, Label};

use super::name_form::NameForm;
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
    let title = SharedString::from(title);
    let on_delete = Rc::new(on_delete);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on_delete = on_delete.clone();
        alert
            .title(title.clone())
            .description(description)
            .confirm()
            .ok_text("Delete")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |event, window, cx| {
                on_delete(event, window, cx);
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
