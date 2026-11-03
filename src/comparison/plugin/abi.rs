//! The data that crosses the boundary between livesplit-core and a comparison
//! plugin (ABI version 1).
//!
//! A plugin is a WebAssembly module that imports nothing and exports:
//!
//! ```text
//! memory
//! lscp_abi_version() -> i32                 // must return ABI_VERSION
//! lscp_alloc(len: i32) -> i32               // a buffer the host writes input to
//! lscp_describe(ptr: i32, len: i32) -> i64  // DescribeInput -> DescribeOutput
//! lscp_generate(ptr: i32, len: i32) -> i64  // GenerateInput -> GenerateOutput
//! ```
//!
//! Inputs and outputs are UTF-8 JSON. The host allocates the input with
//! `lscp_alloc`, after which the buffer belongs to the plugin. An output is
//! returned as `(ptr << 32) | len` and only needs to stay valid until the
//! host calls into the plugin again.
//!
//! Times are whole nanoseconds. Dates are milliseconds since the Unix epoch,
//! in UTC.

use crate::platform::prelude::*;
use alloc::collections::BTreeMap;
use serde_derive::{Deserialize, Serialize};

/// The version of the ABI described here.
pub const ABI_VERSION: i32 = 1;

/// Asks the plugin which comparisons it provides with these settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DescribeInput {
    /// The plugin's settings, as the application keeps them.
    pub settings: String,
}

/// The comparisons a plugin provides.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DescribeOutput {
    /// The names of the comparisons, in the order they should be listed.
    #[serde(default)]
    pub comparisons: Vec<String>,
    /// Set if the settings can't be used. Meant for the runner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Asks the plugin to calculate one of its comparisons.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenerateInput {
    /// The plugin's settings, as the application keeps them.
    pub settings: String,
    /// The comparison to calculate. One of the names from [`DescribeOutput`].
    pub comparison: String,
    /// The run to calculate it from.
    pub run: Snapshot,
}

/// A calculated comparison.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GenerateOutput {
    /// The split times in Real Time, one per segment. Missing ones are empty.
    #[serde(default)]
    pub real: Vec<Option<i64>>,
    /// The split times in Game Time, one per segment. Missing ones are empty.
    #[serde(default)]
    pub game: Vec<Option<i64>>,
    /// Set if the comparison can't be calculated. The comparison is then left
    /// empty. Meant for the runner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Everything a plugin may know about the run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    /// When the comparison is calculated.
    pub now: i64,
    /// The segments, in order.
    pub segments: Vec<SnapshotSegment>,
    /// The attempts, in order.
    pub attempts: Vec<SnapshotAttempt>,
    /// The comparisons already on the run, by name: the Personal Best, the
    /// ones stored in the splits (like a World Record) and the ones
    /// livesplit-core generates (Best Segments, Average Segments, ...). The
    /// built-in ones are always included, even if the runner turned them off.
    /// Comparisons of the plugin itself are left out.
    pub comparisons: BTreeMap<String, SplitTimes>,
}

/// A segment of the run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SnapshotSegment {
    /// The segment's name.
    pub name: String,
    /// The segment times the runner got on this segment, by attempt. An
    /// attempt that never reached the segment isn't listed. An empty time
    /// means the segment was skipped. Attempt ids below 1 hold times that
    /// weren't actually run, for example when the history was imported.
    pub history: Vec<HistoryEntry>,
}

/// One segment time from the segment history.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// The attempt's id.
    pub attempt: i32,
    /// The segment time in Real Time.
    pub real: Option<i64>,
    /// The segment time in Game Time.
    pub game: Option<i64>,
}

/// An attempt of the run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SnapshotAttempt {
    /// The attempt's id.
    pub id: i32,
    /// When the attempt started, if known.
    pub started: Option<i64>,
    /// When the attempt ended, if known.
    pub ended: Option<i64>,
    /// The final time in Real Time, if the attempt finished.
    pub real: Option<i64>,
    /// The final time in Game Time, if the attempt finished.
    pub game: Option<i64>,
}

/// Cumulative split times, one per segment, for both timing methods.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SplitTimes {
    /// The split times in Real Time.
    #[serde(default)]
    pub real: Vec<Option<i64>>,
    /// The split times in Game Time.
    #[serde(default)]
    pub game: Vec<Option<i64>>,
}
