//! The block tree of a custom comparison.
//!
//! The tree mirrors the blocks of the block editor one to one. Every block
//! either produces a *list of times* for each segment ([`Samples`]) or a
//! *single time* for each segment ([`Times`]). Encoding that distinction in two
//! separate Rust types means an ill-typed program (say, a median of a single
//! time) can't even be constructed, so the evaluator never has to deal with it.
//! The JSON reader in [`format`](super::format) is the only place that has to
//! report such mistakes, and it does so with the location of the bad block.
//!
//! Everything is per segment. Times are only summed up into split times at the
//! very end of an evaluation.
//!
//! All durations are stored as whole nanoseconds ([`Nanos`]). Using integers
//! instead of [`TimeSpan`](crate::TimeSpan) keeps the arithmetic exact (for
//! example "stretch to finish in" distributes the rounding leftovers so the
//! total matches the goal to the nanosecond) and keeps the engine independent
//! of the floating point representation of other parts of livesplit-core.

use crate::platform::prelude::*;

/// A duration in whole nanoseconds.
pub type Nanos = i64;

/// The number of nanoseconds in a second.
pub const NANOS_PER_SECOND: Nanos = 1_000_000_000;

/// A user defined comparison: a name and the block that calculates its times.
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    /// The name of the comparison as it shows up in the list of comparisons.
    pub name: String,
    /// The block that calculates one time for each segment.
    pub root: Times,
}

/// Blocks that produce a list of past times for each segment. Sources and
/// filters produce these.
#[derive(Clone, Debug, PartialEq)]
pub enum Samples {
    /// Every recorded time for each segment, from the segment history. Skipped
    /// segments are left out, and so is the segment right after a skip,
    /// because its recorded time covers both segments.
    SegmentHistory,
    /// Keeps, for each segment, the `count` most recent times recorded for that
    /// segment. This counts completions of the segment, not attempts.
    LastN {
        /// How many times to keep. At least 1.
        count: u32,
        /// The list to keep times from.
        source: Box<Samples>,
    },
    /// Keeps only times that came from attempts which finished the whole run.
    CompletedRunsOnly {
        /// The list to filter.
        source: Box<Samples>,
    },
    /// Keeps only times from attempts that ended within the last `days` days.
    /// Attempts without a recorded date are left out, since their age is
    /// unknown.
    RecentDays {
        /// How many days to look back. At least 1.
        days: u32,
        /// The list to filter.
        source: Box<Samples>,
    },
    /// Per segment, drops the fastest and the slowest `percent` % of the times
    /// (rounded down, so small lists are left alone).
    TrimOutliers {
        /// The share to drop from each end, from 0 to
        /// [`MAX_TRIM_PERCENT`](super::format::MAX_TRIM_PERCENT).
        percent: f64,
        /// The list to trim.
        source: Box<Samples>,
    },
    /// Keeps only times from attempts that finished the run faster than
    /// `limit`.
    FinishedUnder {
        /// The final time a run has to beat.
        limit: Nanos,
        /// The list to filter.
        source: Box<Samples>,
    },
    /// Leaves a segment's list empty unless it has at least `count` times, so a
    /// rule built on too little data can be replaced with a fallback.
    AtLeast {
        /// The minimum number of times. At least 1.
        count: u32,
        /// The list to check.
        source: Box<Samples>,
    },
    /// Keeps only times from attempts that got at least as far as the given
    /// segment: they have a recorded (or skipped) time for it. Useful for games
    /// where most runs reset early, so "completed runs only" keeps too little.
    ReachedSegment {
        /// The segment's number, starting at 1.
        segment: u32,
        /// The list to filter.
        source: Box<Samples>,
    },
}

/// The ways a list of times can be reduced to a single time.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AggregateKind {
    /// The fastest time.
    Min,
    /// The slowest time.
    Max,
    /// The mean of all the times.
    Average,
    /// The middle time. For an even number of times, the mean of the two
    /// middle ones.
    Median,
}

/// Which of two times to pick.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PickKind {
    /// The faster of the two.
    Faster,
    /// The slower of the two.
    Slower,
}

/// One arm of a [`Times::SegmentRange`] block.
#[derive(Clone, Debug, PartialEq)]
pub struct RangeCase {
    /// The first segment of the range, starting at 1.
    pub from: u32,
    /// The last segment of the range, inclusive. Not before `from`.
    pub to: u32,
    /// The times to use for the segments in the range.
    pub times: Times,
}

impl RangeCase {
    /// Whether the segment with the given number (starting at 1) is in this
    /// range.
    pub const fn contains(&self, segment: u32) -> bool {
        segment >= self.from && segment <= self.to
    }
}

/// Blocks that produce a single time (or nothing) for each segment.
#[derive(Clone, Debug, PartialEq)]
pub enum Times {
    /// An existing comparison, like the Personal Best, a generated comparison
    /// like Best Segments, or another custom comparison.
    Comparison {
        /// The comparison's name.
        name: String,
    },
    /// The segment times of one past attempt, by its number in the attempt
    /// history. Segments the attempt skipped or never reached are empty.
    Attempt {
        /// The attempt's number, starting at 1.
        number: i32,
    },
    /// The segments of the fastest finished attempt that ended within the last
    /// `days` days: a "recent PB". Empty if no run finished in that time.
    BestRecentRun {
        /// How many days to look back. At least 1.
        days: u32,
    },
    /// Reduces each segment's list of times to one time. A segment without any
    /// times stays empty.
    Aggregate {
        /// How to reduce the list.
        kind: AggregateKind,
        /// The list of times.
        source: Box<Samples>,
    },
    /// The given percentile of each segment's times, interpolating linearly
    /// between neighbouring times. 0 is the fastest, 100 the slowest and 50 is
    /// exactly the median.
    Percentile {
        /// From 0 to 100.
        percent: f64,
        /// The list of times.
        source: Box<Samples>,
    },
    /// An average where recent times count more: the newest time has the
    /// weight 1, and the weight halves for every `half_life` times that are
    /// older.
    WeightedAverage {
        /// After how many older times the weight halves. At least 1.
        half_life: u32,
        /// The list of times.
        source: Box<Samples>,
    },
    /// Adds (or, if negative, subtracts) a fixed amount to every segment that
    /// has a time. A segment never goes below zero.
    AddTime {
        /// The amount to add.
        amount: Nanos,
        /// The times to adjust.
        source: Box<Times>,
    },
    /// Scales every segment by a percentage (105 is 5 % slower).
    Scale {
        /// A positive percentage.
        percent: f64,
        /// The times to scale.
        source: Box<Times>,
    },
    /// Scales every segment by the same factor so they add up to exactly
    /// `total`. If any segment is empty, the total can't be known, so the
    /// whole result is empty.
    StretchToTotal {
        /// The goal time.
        total: Nanos,
        /// The times to stretch.
        source: Box<Times>,
    },
    /// Per segment, `percent` % of the way from `from` to `to`. Empty where
    /// either side is empty.
    Blend {
        /// From 0 (all `from`) to 100 (all `to`).
        percent: f64,
        /// The times at 0 %.
        from: Box<Times>,
        /// The times at 100 %.
        to: Box<Times>,
    },
    /// Per segment, the faster (or slower) of two times. Where one side is
    /// empty, the other one is used.
    Pick {
        /// Whether to pick the faster or the slower time.
        kind: PickKind,
        /// The first times. Ties go to these.
        first: Box<Times>,
        /// The second times.
        second: Box<Times>,
    },
    /// Keeps each segment between two limits. A limit that is empty for a
    /// segment doesn't apply to it. If the limits cross, `low` ("never faster
    /// than") wins.
    Clamp {
        /// The times to keep within the limits.
        source: Box<Times>,
        /// Never faster than these.
        low: Box<Times>,
        /// Never slower than these.
        high: Box<Times>,
    },
    /// Rounds every segment to the nearest multiple of `step` (halves round
    /// up). Split times, being sums of segments, end up on the same step.
    Round {
        /// The step to round to. Positive.
        step: Nanos,
        /// The times to round.
        source: Box<Times>,
    },
    /// Uses different times for different ranges of segments. The first range
    /// that contains a segment wins. Segments no range covers use `otherwise`.
    SegmentRange {
        /// The ranges, in order.
        cases: Vec<RangeCase>,
        /// The times for every other segment.
        otherwise: Box<Times>,
    },
    /// Uses `primary`, filling only its empty segments from `fallback`.
    Fallback {
        /// The times to use where they exist.
        primary: Box<Times>,
        /// The times to fill gaps with.
        fallback: Box<Times>,
    },
}
