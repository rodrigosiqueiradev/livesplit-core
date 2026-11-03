//! The run, as the evaluator sees it.
//!
//! The evaluator doesn't work on [`Segment`](crate::Segment)s directly but on
//! this small, timing method specific snapshot. That keeps the evaluator free
//! of livesplit-core's bookkeeping (comparisons stored inside the segments,
//! `Time` holding both timing methods, negative attempt ids for imported
//! history, ...) and makes it trivial to construct runs in tests.

use super::ast::Nanos;
use crate::platform::{DateTime, prelude::*};
use alloc::collections::BTreeMap;

/// Everything the evaluator needs from a run, for one timing method.
#[derive(Clone, Debug)]
pub struct RunData {
    /// The segments, in order.
    pub segments: Vec<SegmentData>,
    /// The attempts, by their index.
    pub attempts: BTreeMap<i32, AttemptData>,
    /// Existing comparisons a program may refer to (other than the custom
    /// comparisons being evaluated), as cumulative split times: one entry per
    /// segment.
    pub comparisons: BTreeMap<String, Vec<Option<Nanos>>>,
    /// "Now", for blocks like "from the last 30 days".
    pub now: DateTime,
}

/// A segment of the run.
#[derive(Clone, Debug, Default)]
pub struct SegmentData {
    /// The segment's name.
    pub name: String,
    /// Attempt index to recorded segment time. `None` means the segment was
    /// skipped in that attempt. A missing key means the attempt never reached
    /// the segment.
    pub history: BTreeMap<i32, Option<Nanos>>,
}

/// An attempt of the run.
#[derive(Clone, Debug, Default)]
pub struct AttemptData {
    /// When the attempt ended (or started, if no end was recorded). `None` if
    /// unknown.
    pub when: Option<DateTime>,
    /// The attempt's final time, if it finished the whole run.
    pub final_time: Option<Nanos>,
}

impl AttemptData {
    /// Whether the attempt finished the whole run.
    pub const fn completed(&self) -> bool {
        self.final_time.is_some()
    }
}

impl RunData {
    /// The number of segments.
    pub const fn segment_count(&self) -> usize {
        self.segments.len()
    }
}
