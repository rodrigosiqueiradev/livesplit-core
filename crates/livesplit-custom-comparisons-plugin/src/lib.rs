//! The Custom Comparisons comparison plugin.
//!
//! Its settings hold the comparisons a runner built out of blocks, each in
//! the custom comparison JSON format:
//!
//! ```json
//! { "comparisons": [ { "version": 1, "name": "...", ... }, ... ] }
//! ```
//!
//! It is built as a WebAssembly module that implements the comparison plugin
//! ABI described in `livesplit_core::comparison::plugin::abi`:
//!
//! ```sh
//! cargo build -p livesplit-custom-comparisons-plugin --target wasm32-unknown-unknown --release
//! ```
//!
//! What the exports do lives in `livesplit_core::comparison::custom::exports`,
//! so it can be tested without a WebAssembly runtime.

use livesplit_core::comparison::{
    custom::{
        exports::{describe, failed, generate},
        settings,
    },
    plugin::abi::{ABI_VERSION, DescribeInput, DescribeOutput, GenerateInput},
};
use std::cell::RefCell;

/// The id that names this plugin.
pub const ID: &str = settings::PLUGIN_ID;

// The WebAssembly exports.

thread_local! {
    static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Takes back the input buffer the host filled.
///
/// # Safety
///
/// `ptr` and `len` must come from a call to [`lscp_alloc`].
unsafe fn take_input(ptr: *mut u8, len: i32) -> Box<[u8]> {
    // SAFETY: The buffer was leaked by `lscp_alloc` with exactly this length.
    unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(ptr, len as usize)) }
}

fn respond(output: &impl serde::Serialize) -> i64 {
    OUTPUT.with_borrow_mut(|buffer| {
        *buffer = serde_json::to_vec(output).unwrap_or_else(|_| {
            br#"{"error":"The plugin's output couldn't be written."}"#.to_vec()
        });
        ((buffer.as_ptr() as u32 as i64) << 32) | buffer.len() as i64
    })
}

/// Returns the version of the plugin interface this plugin implements.
#[unsafe(no_mangle)]
pub extern "C" fn lscp_abi_version() -> i32 {
    ABI_VERSION
}

/// Returns a buffer of `len` bytes for the host's input.
#[unsafe(no_mangle)]
pub extern "C" fn lscp_alloc(len: i32) -> *mut u8 {
    Box::into_raw(vec![0u8; len.max(0) as usize].into_boxed_slice()).cast()
}

/// Describes the custom comparisons in the settings.
///
/// # Safety
///
/// `ptr` and `len` must come from a call to [`lscp_alloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lscp_describe(ptr: *mut u8, len: i32) -> i64 {
    // SAFETY: Guaranteed by the caller.
    let input = unsafe { take_input(ptr, len) };
    match serde_json::from_slice::<DescribeInput>(&input) {
        Ok(input) => respond(&describe(&input)),
        Err(e) => respond(&DescribeOutput {
            error: Some(format!("The plugin's input couldn't be read: {e}")),
            ..Default::default()
        }),
    }
}

/// Calculates one custom comparison.
///
/// # Safety
///
/// `ptr` and `len` must come from a call to [`lscp_alloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lscp_generate(ptr: *mut u8, len: i32) -> i64 {
    // SAFETY: Guaranteed by the caller.
    let input = unsafe { take_input(ptr, len) };
    match serde_json::from_slice::<GenerateInput>(&input) {
        Ok(input) => respond(&generate(&input)),
        Err(e) => respond(&failed(format!("The plugin's input couldn't be read: {e}"))),
    }
}
