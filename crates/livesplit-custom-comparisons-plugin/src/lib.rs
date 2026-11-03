//! The Custom Comparisons comparison plugin.
//!
//! Its settings hold the comparisons a runner built out of blocks, each in
//! the JSON format of the desktop LiveSplit component *Custom Comparisons*:
//!
//! ```json
//! { "comparisons": [ { "version": 4, "name": "...", ... }, ... ] }
//! ```
//!
//! It is built as a WebAssembly module that implements the comparison plugin
//! ABI described in `livesplit_core::comparison::plugin::abi`:
//!
//! ```sh
//! cargo build -p livesplit-custom-comparisons-plugin --target wasm32-unknown-unknown --release
//! ```
//!
//! The functions behind the exports are public, so the plugin can also be
//! tested without a WebAssembly runtime.

use livesplit_core::comparison::{
    custom::{
        Evaluator, Library, format,
        run_data::{AttemptData, RunData, SegmentData},
        settings,
    },
    plugin::abi::{
        ABI_VERSION, DescribeInput, DescribeOutput, GenerateInput, GenerateOutput, Snapshot,
        SplitTimes,
    },
};
use std::cell::RefCell;
use time::OffsetDateTime;

/// The id that names this plugin.
pub const ID: &str = settings::PLUGIN_ID;

/// Reads the settings into a library of programs. Comparisons that can't be
/// read are left out, like in the desktop LiveSplit: the editor shows what is
/// wrong with them.
pub fn library(settings: &str) -> Result<Library, String> {
    let texts = settings::read(settings)?;
    Ok(Library::new(
        texts
            .iter()
            .filter_map(|text| Some(format::read(text).ok()?.program)),
    ))
}

/// `lscp_describe`: the names of the custom comparisons.
pub fn describe(input: &DescribeInput) -> DescribeOutput {
    match library(&input.settings) {
        Ok(library) => DescribeOutput {
            comparisons: library.programs().iter().map(|p| p.name.clone()).collect(),
            error: None,
        },
        Err(error) => DescribeOutput {
            comparisons: Vec::new(),
            error: Some(error),
        },
    }
}

/// `lscp_generate`: evaluates one custom comparison for both timing methods.
pub fn generate(input: &GenerateInput) -> GenerateOutput {
    let library = match library(&input.settings) {
        Ok(library) => library,
        Err(error) => return failed(error),
    };
    let evaluator = Evaluator::new(library.programs());
    let mut output = GenerateOutput::default();
    for real_time in [true, false] {
        let run = run_data(&input.run, real_time);
        let Some(result) = evaluator.evaluate(&input.comparison, &run) else {
            return failed(format!(
                "There is no comparison named '{}'.",
                input.comparison
            ));
        };
        if output.error.is_none() {
            output.error = result.error;
        }
        if real_time {
            output.real = result.split_times;
        } else {
            output.game = result.split_times;
        }
    }
    output
}

fn failed(error: String) -> GenerateOutput {
    GenerateOutput {
        error: Some(error),
        ..Default::default()
    }
}

fn date(millis: i64) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000).ok()
}

fn pick(times: &SplitTimes, real_time: bool) -> Vec<Option<i64>> {
    if real_time {
        times.real.clone()
    } else {
        times.game.clone()
    }
}

/// Turns the snapshot into what the evaluator works on, for one timing
/// method.
pub fn run_data(run: &Snapshot, real_time: bool) -> RunData {
    let segments = run
        .segments
        .iter()
        .map(|segment| SegmentData {
            name: segment.name.clone(),
            // Only the runner's own attempts. Ids below 1 hold times that
            // were made up when segments were added or the history imported.
            history: segment
                .history
                .iter()
                .filter(|entry| entry.attempt >= 1)
                .map(|entry| {
                    let time = if real_time { entry.real } else { entry.game };
                    (entry.attempt, time)
                })
                .collect(),
        })
        .collect();

    let attempts = run
        .attempts
        .iter()
        .map(|attempt| {
            let data = AttemptData {
                when: attempt.ended.or(attempt.started).and_then(date),
                final_time: if real_time {
                    attempt.real
                } else {
                    attempt.game
                },
            };
            (attempt.id, data)
        })
        .collect();

    let comparisons = run
        .comparisons
        .iter()
        .map(|(name, times)| (name.clone(), pick(times, real_time)))
        .collect();

    RunData {
        segments,
        attempts,
        comparisons,
        now: date(run.now).unwrap_or(OffsetDateTime::UNIX_EPOCH),
    }
}

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
