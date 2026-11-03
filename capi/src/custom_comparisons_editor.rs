//! The Custom Comparisons Editor edits the custom comparisons stored in a
//! layout's Custom Comparisons Component. It follows the same rules as the
//! desktop LiveSplit component Custom Comparisons: names are unique (ignoring
//! case) and can't be one of LiveSplit's own comparisons or one the run
//! already has, a comparison that can't be read is kept exactly as it is, and
//! renaming a comparison updates the other comparisons that use it.
//!
//! Comparisons are imported and exported in the JSON format shared with the
//! desktop component. Functions that can fail return an empty string on
//! success and otherwise a message meant for the runner.

use super::{Json, output_str, output_vec, str};
use livesplit_core::{
    LayoutEditor, Run, TimingMethod,
    comparison::custom::{self, Editor as CustomComparisonsEditor},
};
use std::os::raw::c_char;

/// type
pub type OwnedCustomComparisonsEditor = Box<CustomComparisonsEditor>;
/// type
pub type NullableOwnedCustomComparisonsEditor = Option<OwnedCustomComparisonsEditor>;

fn result(result: Result<(), String>) -> *const c_char {
    output_str(result.err().unwrap_or_default())
}

/// Creates a new Custom Comparisons Editor for the comparisons stored in the
/// component selected in the Layout Editor. The run provides the comparisons
/// the custom comparisons can't be named after, like the ones stored in the
/// splits. If the selected component isn't a Custom Comparisons Component,
/// <NULL> is returned instead.
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsEditor_new(
    layout_editor: &LayoutEditor,
    run: &Run,
) -> NullableOwnedCustomComparisonsEditor {
    let comparisons = layout_editor.custom_comparisons()?;
    Some(Box::new(CustomComparisonsEditor::new(
        comparisons.iter().cloned(),
        run,
    )))
}

/// Closes the Custom Comparisons Editor and stores the comparisons in the
/// component selected in the Layout Editor. Returns whether they were stored,
/// which they aren't if the selected component isn't a Custom Comparisons
/// Component anymore. In case you want to implement a Cancel Button, just
/// drop the editor instead.
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsEditor_close(
    this: OwnedCustomComparisonsEditor,
    layout_editor: &mut LayoutEditor,
) -> bool {
    layout_editor.set_custom_comparisons((*this).close())
}

/// Creates a new Custom Comparisons Editor for the comparisons in the
/// settings of the Custom Comparisons plugin, as an application keeps them
/// (`{"comparisons": [...]}`). The run provides the comparisons the custom
/// comparisons can't be named after, like the ones stored in the splits.
/// Empty settings start an empty list. If the settings can't be read, <NULL>
/// is returned instead.
///
/// # Safety
///
/// `settings` must be a valid string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn CustomComparisonsEditor_from_settings(
    settings: *const c_char,
    run: &Run,
) -> NullableOwnedCustomComparisonsEditor {
    // SAFETY: The caller guarantees that `settings` is valid.
    let comparisons = custom::settings::read(unsafe { str(settings) }).ok()?;
    Some(Box::new(CustomComparisonsEditor::new(comparisons, run)))
}

/// Closes the Custom Comparisons Editor and returns the comparisons as the
/// settings of the Custom Comparisons plugin. This is also the file format
/// for exporting and importing all of them at once.
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsEditor_into_settings(
    this: OwnedCustomComparisonsEditor,
) -> *const c_char {
    output_str(custom::settings::write((*this).close()))
}

/// Reads the settings of the Custom Comparisons plugin and returns the
/// comparisons in them, each as its own text, as a JSON array. If the
/// settings can't be read, JSON `null` is returned instead.
///
/// # Safety
///
/// `settings` must be a valid string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn CustomComparisonsEditor_read_settings(settings: *const c_char) -> Json {
    // SAFETY: The caller guarantees that `settings` is valid.
    match custom::settings::read(unsafe { str(settings) }) {
        Ok(comparisons) => output_vec(|o| {
            serde_json::to_writer(o, &comparisons).unwrap();
        }),
        Err(_) => output_str("null"),
    }
}

/// drop
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsEditor_drop(this: OwnedCustomComparisonsEditor) {
    drop(this);
}

/// Encodes the state of the list of comparisons as JSON, for the run the
/// comparisons are used with: for each comparison its name, whether it could
/// be read, why it doesn't work (if it doesn't) and how many other
/// comparisons use it.
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsEditor_state_as_json(
    this: &CustomComparisonsEditor,
    run: &Run,
) -> Json {
    output_vec(|o| {
        this.state(run).write_json(o).unwrap();
    })
}

/// Returns the number of comparisons.
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsEditor_len(this: &CustomComparisonsEditor) -> usize {
    this.len()
}

/// Returns the name of the comparison at the index. For a comparison that
/// can't be read, this is the best guess. This panics if the index is out of
/// bounds.
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsEditor_name(
    this: &CustomComparisonsEditor,
    index: usize,
) -> *const c_char {
    output_str(this.name(index))
}

/// Checks whether the name can be used for a new comparison. Returns an empty
/// string if it can and otherwise why not.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn CustomComparisonsEditor_check_name(
    this: &CustomComparisonsEditor,
    name: *const c_char,
) -> *const c_char {
    // SAFETY: The caller guarantees that `name` is valid.
    output_str(
        this.check_name(unsafe { str(name) }, None)
            .unwrap_or_default(),
    )
}

/// Checks whether the comparison at the index can be renamed to the name.
/// Returns an empty string if it can and otherwise why not.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn CustomComparisonsEditor_check_rename(
    this: &CustomComparisonsEditor,
    index: usize,
    name: *const c_char,
) -> *const c_char {
    // SAFETY: The caller guarantees that `name` is valid.
    output_str(
        this.check_name(unsafe { str(name) }, Some(index))
            .unwrap_or_default(),
    )
}

/// Returns a name that isn't taken yet, based on the name provided: "Pace",
/// "Pace (2)", ...
#[unsafe(no_mangle)]
pub unsafe extern "C" fn CustomComparisonsEditor_suggest_name(
    this: &CustomComparisonsEditor,
    name: *const c_char,
) -> *const c_char {
    // SAFETY: The caller guarantees that `name` is valid.
    output_str(this.suggest_name(unsafe { str(name) }))
}

/// Adds a comparison from its JSON text, like an imported file or text pasted
/// from a chat. Returns an empty string on success and otherwise why it can't
/// be added. If its name is taken, it can be added under another name with
/// CustomComparisonsEditor_add_as.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn CustomComparisonsEditor_add(
    this: &mut CustomComparisonsEditor,
    text: *const c_char,
) -> *const c_char {
    // SAFETY: The caller guarantees that `text` is valid.
    result(this.add(unsafe { str(text) }))
}

/// Adds a comparison from its JSON text under another name. Returns an empty
/// string on success and otherwise why it can't be added.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn CustomComparisonsEditor_add_as(
    this: &mut CustomComparisonsEditor,
    text: *const c_char,
    name: *const c_char,
) -> *const c_char {
    // SAFETY: The caller guarantees that `text` and `name` are valid.
    result(this.add_as(unsafe { str(text) }, unsafe { str(name) }))
}

/// Replaces the comparison at the index with the one in the JSON text, like
/// after editing it. The comparisons using its old name are not updated, use
/// CustomComparisonsEditor_rename for that. Returns an empty string on success
/// and otherwise why it can't be replaced. This panics if the index is out of
/// bounds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn CustomComparisonsEditor_replace(
    this: &mut CustomComparisonsEditor,
    index: usize,
    text: *const c_char,
) -> *const c_char {
    // SAFETY: The caller guarantees that `text` is valid.
    result(this.replace(index, unsafe { str(text) }))
}

/// Renames the comparison at the index and updates the comparisons that use
/// it. Returns an empty string on success and otherwise why it can't be
/// renamed. This panics if the index is out of bounds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn CustomComparisonsEditor_rename(
    this: &mut CustomComparisonsEditor,
    index: usize,
    name: *const c_char,
) -> *const c_char {
    // SAFETY: The caller guarantees that `name` is valid.
    result(this.rename(index, unsafe { str(name) }))
}

/// Removes the comparison at the index. The comparisons using it stop
/// working. This panics if the index is out of bounds.
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsEditor_remove(this: &mut CustomComparisonsEditor, index: usize) {
    this.remove(index);
}

/// Moves the comparison at the index up by one. Returns whether it moved.
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsEditor_move_up(
    this: &mut CustomComparisonsEditor,
    index: usize,
) -> bool {
    this.move_up(index)
}

/// Moves the comparison at the index down by one. Returns whether it moved.
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsEditor_move_down(
    this: &mut CustomComparisonsEditor,
    index: usize,
) -> bool {
    this.move_down(index)
}

/// Returns how many of the other comparisons use the comparison at the index.
/// They stop working if it's removed. This panics if the index is out of
/// bounds.
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsEditor_used_by(
    this: &CustomComparisonsEditor,
    index: usize,
) -> usize {
    this.used_by(index)
}

/// Exports the comparison at the index as indented JSON, for saving it as a
/// file. A comparison that can't be read is exported exactly as stored. This
/// panics if the index is out of bounds.
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsEditor_export(
    this: &CustomComparisonsEditor,
    index: usize,
) -> *const c_char {
    output_str(this.export(index))
}

/// Exports the comparison at the index as JSON on a single line, for copying
/// it into a chat. A comparison that can't be read is exported exactly as
/// stored. This panics if the index is out of bounds.
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsEditor_export_compact(
    this: &CustomComparisonsEditor,
    index: usize,
) -> *const c_char {
    output_str(this.export_compact(index))
}

/// Calculates a preview of a comparison being edited, given as JSON text, for
/// the run and timing method, and encodes it as JSON: for each segment its
/// name, segment time and split time in seconds and the block the time came
/// from, or why the comparison can't be calculated. The index is the
/// comparison the text replaces. An index that is out of bounds means a new
/// comparison.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn CustomComparisonsEditor_preview_as_json(
    this: &CustomComparisonsEditor,
    text: *const c_char,
    index: usize,
    run: &Run,
    method: TimingMethod,
) -> Json {
    // SAFETY: The caller guarantees that `text` is valid.
    let text = unsafe { str(text) };
    let index = (index < this.len()).then_some(index);
    let preview = this.preview(text, index, run, method);
    output_vec(|o| {
        preview.write_json(o).unwrap();
    })
}
