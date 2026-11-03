//! A comparison plugin is a WebAssembly module that calculates comparisons
//! from the run's segment history, like the Custom Comparisons plugin. Load
//! one with its settings and install it on a Timer to make its comparisons
//! available. Where the module and its settings are kept is up to you.

use super::{Json, output_str, output_vec, slice, str};
use livesplit_core::{
    Timer,
    comparison::plugin::{self, Plugin as ComparisonPlugin},
};
use std::{cell::RefCell, os::raw::c_char};

/// type
pub type OwnedComparisonPlugin = Box<ComparisonPlugin>;
/// type
pub type NullableOwnedComparisonPlugin = Option<OwnedComparisonPlugin>;

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Loads a comparison plugin from its WebAssembly module and gives it its
/// settings. On the web the browser's own WebAssembly engine runs it,
/// elsewhere wasmtime does. If the plugin can't be loaded, <NULL> is returned
/// and ComparisonPlugin_load_error tells why.
///
/// # Safety
///
/// `module` must be valid for `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ComparisonPlugin_new(
    id: *const c_char,
    settings: *const c_char,
    module: *const u8,
    length: usize,
) -> NullableOwnedComparisonPlugin {
    // SAFETY: The caller guarantees that the strings are valid.
    let (id, settings) = unsafe { (str(id), str(settings)) };
    // SAFETY: The caller guarantees that `module` is valid for `length`.
    let module = unsafe { slice(module, length) };
    let result =
        plugin::engine_for(module).and_then(|engine| ComparisonPlugin::new(id, settings, engine));
    LAST_ERROR.with_borrow_mut(|e| {
        *e = result
            .as_ref()
            .err()
            .map(|e| e.0.clone())
            .unwrap_or_default();
    });
    result.ok().map(Box::new)
}

/// Why the last call to ComparisonPlugin_new on this thread returned <NULL>,
/// in words meant for the runner. Empty if it didn't.
#[unsafe(no_mangle)]
pub extern "C" fn ComparisonPlugin_load_error() -> *const c_char {
    LAST_ERROR.with_borrow(|e| output_str(e))
}

/// drop
#[unsafe(no_mangle)]
pub extern "C" fn ComparisonPlugin_drop(this: OwnedComparisonPlugin) {
    drop(this);
}

/// The plugin's id.
#[unsafe(no_mangle)]
pub extern "C" fn ComparisonPlugin_id(this: &ComparisonPlugin) -> *const c_char {
    output_str(this.id())
}

/// The names of the comparisons the plugin provides, as a JSON array.
#[unsafe(no_mangle)]
pub extern "C" fn ComparisonPlugin_comparisons(this: &ComparisonPlugin) -> Json {
    output_vec(|o| {
        serde_json::to_writer(o, this.comparisons()).unwrap();
    })
}

/// Installs the plugin on the timer's run, replacing the comparisons an
/// earlier version of the same plugin installed, and calculates them. The
/// plugin can be dropped afterwards. The current attempt is kept. If the
/// current comparison no longer exists, the timer switches to the Personal
/// Best. Returns the names of the comparisons that weren't installed because
/// the run already has a comparison with that name, as a JSON array.
#[unsafe(no_mangle)]
pub extern "C" fn ComparisonPlugin_install(this: &ComparisonPlugin, timer: &mut Timer) -> Json {
    let left_out = timer.install_comparison_plugin(this);
    output_vec(|o| {
        serde_json::to_writer(o, &left_out).unwrap();
    })
}

/// Removes the comparisons the plugin with this id installed on the timer's
/// run. If the current comparison no longer exists, the timer switches to the
/// Personal Best.
///
/// # Safety
///
/// `id` must be a valid string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ComparisonPlugin_uninstall(id: *const c_char, timer: &mut Timer) {
    // SAFETY: The caller guarantees that `id` is valid.
    timer.remove_comparison_plugin(unsafe { str(id) });
}
