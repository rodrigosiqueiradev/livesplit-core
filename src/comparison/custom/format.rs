//! Reads and writes the custom comparison file format.
//!
//! A custom comparison is stored, exported and shared as a small versioned
//! JSON document:
//!
//! ```json
//! {
//!   "format": "livesplit-custom-comparison",
//!   "version": 1,
//!   "name": "Safe Pace",
//!   "root": { "type": "fallback", "primary": { ... }, "fallback": { ... } }
//! }
//! ```
//!
//! A file is pure data: it can only describe blocks from the fixed set in
//! [`ast`](super::ast). Other members are ignored.
//!
//! The version lets a later release add blocks or change the format: a file
//! with a newer version than this build understands is refused with a message
//! asking the runner to update, instead of being misread. Every value is
//! validated while reading, and every error names the place it
//! happened, like `root.source.count: must be a whole number from 1 to 10000.`,
//! because these messages are shown to runners who are editing or importing a
//! comparison.

use super::ast::{
    AggregateKind, NANOS_PER_SECOND, Nanos, PickKind, Program, RangeCase, Samples, Times,
};
use crate::platform::prelude::*;
use core::fmt;
use serde_json::{Map, Value};

/// The `format` value that identifies a custom comparison file.
pub const FORMAT_ID: &str = "livesplit-custom-comparison";

/// The newest format version this build understands, which is also the
/// version every file is written with.
pub const CURRENT_VERSION: u32 = 1;

/// The longest allowed comparison name, in characters.
pub const MAX_NAME_LENGTH: usize = 64;
/// The largest count for "last N" and "at least N" and the largest half-life.
pub const MAX_COUNT: u32 = 10_000;
/// The largest number of days for the day based blocks.
pub const MAX_DAYS: u32 = 36_500;
/// The largest goal or limit time, in seconds (100 hours).
pub const MAX_TOTAL_SECONDS: f64 = 360_000.0;
/// The largest segment number a range or "reached segment" can name.
pub const MAX_SEGMENT: u32 = 10_000;
/// The largest number of ranges in a segment range block.
pub const MAX_CASES: usize = 100;
/// The largest amount "add time" and "round" accept, in seconds.
pub const MAX_SECONDS: f64 = 3600.0;
/// The smallest percentage for the scale block.
pub const MIN_SCALE_PERCENT: f64 = 1.0;
/// The largest percentage for the scale block.
pub const MAX_SCALE_PERCENT: f64 = 1000.0;
/// The largest share the trim block may drop from each end.
pub const MAX_TRIM_PERCENT: f64 = 45.0;

/// A file that can't be read. The message is meant for the runner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatError(pub String);

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl core::error::Error for FormatError {}

type Result<T> = core::result::Result<T, FormatError>;

// ------------------------------------------------------------------ Writing

/// Writes a comparison as indented JSON, the way files are exported.
pub fn write(program: &Program) -> String {
    serde_json::to_string_pretty(&to_value(program)).expect("A JSON value can always be written")
}

/// Writes a comparison as JSON on a single line, the way comparisons are
/// stored and copied to the clipboard, so chat apps don't mangle them.
pub fn write_compact(program: &Program) -> String {
    serde_json::to_string(&to_value(program)).expect("A JSON value can always be written")
}

fn to_value(program: &Program) -> Value {
    let mut root = Map::new();
    root.insert("format".into(), FORMAT_ID.into());
    root.insert("version".into(), CURRENT_VERSION.into());
    root.insert("name".into(), program.name.as_str().into());
    root.insert("root".into(), write_times(&program.root));
    Value::Object(root)
}

fn typed(type_name: &str) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("type".into(), type_name.into());
    map
}

/// Seconds, rounded to milliseconds.
fn seconds(nanos: Nanos) -> Value {
    let millis = libm::round(nanos as f64 / 1_000_000.0);
    number(millis / 1000.0)
}

/// Writes whole numbers without a fraction, so `103` doesn't become `103.0`.
fn number(value: f64) -> Value {
    if libm::trunc(value) == value && libm::fabs(value) < 1e15 {
        (value as i64).into()
    } else {
        value.into()
    }
}

fn with(mut map: Map<String, Value>, key: &str, value: impl Into<Value>) -> Map<String, Value> {
    map.insert(key.into(), value.into());
    map
}

fn write_samples(node: &Samples) -> Value {
    Value::Object(match node {
        Samples::SegmentHistory => typed("segmentHistory"),
        Samples::LastN { count, source } => with(
            with(typed("lastN"), "count", *count),
            "source",
            write_samples(source),
        ),
        Samples::CompletedRunsOnly { source } => {
            with(typed("completedRunsOnly"), "source", write_samples(source))
        }
        Samples::RecentDays { days, source } => with(
            with(typed("recentDays"), "days", *days),
            "source",
            write_samples(source),
        ),
        Samples::TrimOutliers { percent, source } => with(
            with(typed("trimOutliers"), "percent", number(*percent)),
            "source",
            write_samples(source),
        ),
        Samples::FinishedUnder { limit, source } => with(
            with(typed("finishedUnder"), "totalSeconds", seconds(*limit)),
            "source",
            write_samples(source),
        ),
        Samples::AtLeast { count, source } => with(
            with(typed("atLeast"), "count", *count),
            "source",
            write_samples(source),
        ),
        Samples::ReachedSegment { segment, source } => with(
            with(typed("reachedSegment"), "segment", *segment),
            "source",
            write_samples(source),
        ),
    })
}

fn write_times(node: &Times) -> Value {
    Value::Object(match node {
        Times::Comparison { name } => with(typed("comparison"), "name", name.as_str()),
        Times::Attempt { number } => with(typed("attempt"), "number", *number),
        Times::BestRecentRun { days } => with(typed("bestRecentRun"), "days", *days),
        Times::Aggregate { kind, source } => with(
            with(typed("aggregate"), "kind", kind_name(*kind)),
            "source",
            write_samples(source),
        ),
        Times::Percentile { percent, source } => with(
            with(typed("percentile"), "percent", number(*percent)),
            "source",
            write_samples(source),
        ),
        Times::WeightedAverage { half_life, source } => with(
            with(typed("weightedAverage"), "halfLife", *half_life),
            "source",
            write_samples(source),
        ),
        Times::AddTime { amount, source } => with(
            with(typed("addTime"), "seconds", seconds(*amount)),
            "source",
            write_times(source),
        ),
        Times::Scale { percent, source } => with(
            with(typed("scale"), "percent", number(*percent)),
            "source",
            write_times(source),
        ),
        Times::StretchToTotal { total, source } => with(
            with(typed("stretchToTotal"), "totalSeconds", seconds(*total)),
            "source",
            write_times(source),
        ),
        Times::Blend { percent, from, to } => with(
            with(
                with(typed("blend"), "percent", number(*percent)),
                "from",
                write_times(from),
            ),
            "to",
            write_times(to),
        ),
        Times::Pick {
            kind,
            first,
            second,
        } => with(
            with(
                with(
                    typed("pick"),
                    "kind",
                    match kind {
                        PickKind::Faster => "faster",
                        PickKind::Slower => "slower",
                    },
                ),
                "first",
                write_times(first),
            ),
            "second",
            write_times(second),
        ),
        Times::Clamp { source, low, high } => with(
            with(
                with(typed("clamp"), "source", write_times(source)),
                "low",
                write_times(low),
            ),
            "high",
            write_times(high),
        ),
        Times::Round { step, source } => with(
            with(typed("round"), "step", seconds(*step)),
            "source",
            write_times(source),
        ),
        Times::SegmentRange { cases, otherwise } => with(
            with(
                typed("segmentRange"),
                "cases",
                cases
                    .iter()
                    .map(|c| {
                        Value::Object(with(
                            with(with(Map::new(), "from", c.from), "to", c.to),
                            "times",
                            write_times(&c.times),
                        ))
                    })
                    .collect::<Vec<_>>(),
            ),
            "otherwise",
            write_times(otherwise),
        ),
        Times::Fallback { primary, fallback } => with(
            with(typed("fallback"), "primary", write_times(primary)),
            "fallback",
            write_times(fallback),
        ),
    })
}

const fn kind_name(kind: AggregateKind) -> &'static str {
    match kind {
        AggregateKind::Min => "min",
        AggregateKind::Max => "max",
        AggregateKind::Average => "average",
        AggregateKind::Median => "median",
    }
}

// ------------------------------------------------------------------ Reading

/// Reads a comparison file. Errors are meant for the runner.
pub fn read(text: &str) -> Result<Program> {
    let json: Value = serde_json::from_str(text)
        .map_err(|e| FormatError(format!("This isn't valid JSON: {e}")))?;

    let not_ours = || FormatError("This isn't a Custom Comparisons file.".into());
    let root = json.as_object().ok_or_else(not_ours)?;
    if root.get("format").and_then(Value::as_str) != Some(FORMAT_ID) {
        return Err(not_ours());
    }

    let version = read_int(root, "version", "", 1, u32::MAX)?;
    if version > CURRENT_VERSION {
        return Err(FormatError(
            "This comparison was made with a newer version of Custom Comparisons. Update to use it."
                .into(),
        ));
    }

    let name = read_string(root, "name", "")?;
    if name.trim().is_empty() {
        return Err(FormatError("The comparison has no name.".into()));
    }
    if name.trim().chars().count() > MAX_NAME_LENGTH {
        return Err(FormatError(format!(
            "The comparison name is longer than {MAX_NAME_LENGTH} characters."
        )));
    }

    Ok(Program {
        name: name.trim().to_string(),
        root: read_times(required(root, "root", "")?, "root")?,
    })
}

const LIST_TYPES: &[&str] = &[
    "segmentHistory",
    "lastN",
    "completedRunsOnly",
    "recentDays",
    "trimOutliers",
    "finishedUnder",
    "atLeast",
    "reachedSegment",
];

fn read_times(value: &Value, path: &str) -> Result<Times> {
    let (obj, type_name) = as_block(value, path)?;
    let source = |key: &str| required(obj, key, path).and_then(|v| read_times(v, &join(path, key)));
    let samples =
        || required(obj, "source", path).and_then(|v| read_samples(v, &join(path, "source")));

    Ok(match type_name {
        "comparison" => {
            let name = read_string(obj, "name", path)?;
            if name.trim().is_empty() {
                return Err(error(path, "name", "pick a comparison"));
            }
            Times::Comparison {
                name: name.trim().to_string(),
            }
        }
        "attempt" => Times::Attempt {
            number: read_int(obj, "number", path, 1, i32::MAX as u32)? as i32,
        },
        "bestRecentRun" => Times::BestRecentRun {
            days: read_int(obj, "days", path, 1, MAX_DAYS)?,
        },
        "aggregate" => Times::Aggregate {
            kind: match read_string(obj, "kind", path)? {
                "min" => AggregateKind::Min,
                "max" => AggregateKind::Max,
                "average" => AggregateKind::Average,
                "median" => AggregateKind::Median,
                _ => return Err(error(path, "kind", "must be min, max, average or median")),
            },
            source: Box::new(samples()?),
        },
        "percentile" => Times::Percentile {
            source: Box::new(samples()?),
            percent: read_number(obj, "percent", path, 0.0, 100.0)?,
        },
        "weightedAverage" => Times::WeightedAverage {
            source: Box::new(samples()?),
            half_life: read_int(obj, "halfLife", path, 1, MAX_COUNT)?,
        },
        "addTime" => Times::AddTime {
            amount: to_nanos(read_number(
                obj,
                "seconds",
                path,
                -MAX_SECONDS,
                MAX_SECONDS,
            )?),
            source: Box::new(source("source")?),
        },
        "scale" => Times::Scale {
            source: Box::new(source("source")?),
            percent: read_number(obj, "percent", path, MIN_SCALE_PERCENT, MAX_SCALE_PERCENT)?,
        },
        "stretchToTotal" => Times::StretchToTotal {
            total: to_nanos(read_number(
                obj,
                "totalSeconds",
                path,
                0.001,
                MAX_TOTAL_SECONDS,
            )?),
            source: Box::new(source("source")?),
        },
        "blend" => Times::Blend {
            from: Box::new(source("from")?),
            to: Box::new(source("to")?),
            percent: read_number(obj, "percent", path, 0.0, 100.0)?,
        },
        "pick" => Times::Pick {
            kind: match read_string(obj, "kind", path)? {
                "faster" => PickKind::Faster,
                "slower" => PickKind::Slower,
                _ => return Err(error(path, "kind", "must be faster or slower")),
            },
            first: Box::new(source("first")?),
            second: Box::new(source("second")?),
        },
        "clamp" => Times::Clamp {
            source: Box::new(source("source")?),
            low: Box::new(source("low")?),
            high: Box::new(source("high")?),
        },
        "round" => {
            let step = read_number(obj, "step", path, 0.001, MAX_SECONDS)?;
            Times::Round {
                source: Box::new(source("source")?),
                step: to_nanos(step),
            }
        }
        "fallback" => Times::Fallback {
            primary: Box::new(source("primary")?),
            fallback: Box::new(source("fallback")?),
        },
        "segmentRange" => {
            let cases_path = join(path, "cases");
            let cases = required(obj, "cases", path)?
                .as_array()
                .ok_or_else(|| error(path, "cases", "must be a list"))?;
            if cases.len() > MAX_CASES {
                return Err(error(
                    path,
                    "cases",
                    &format!("can have at most {MAX_CASES} ranges"),
                ));
            }

            let mut parsed = Vec::with_capacity(cases.len());
            for (i, case) in cases.iter().enumerate() {
                let case_path = format!("{cases_path}[{i}]");
                let case = case
                    .as_object()
                    .ok_or_else(|| FormatError(format!("{case_path}: must be an object.")))?;
                let from = read_int(case, "from", &case_path, 1, MAX_SEGMENT)?;
                let to = read_int(case, "to", &case_path, 1, MAX_SEGMENT)?;
                if to < from {
                    return Err(error(&case_path, "to", "must not be before \"from\""));
                }
                let times = read_times(
                    required(case, "times", &case_path)?,
                    &join(&case_path, "times"),
                )?;
                parsed.push(RangeCase { from, to, times });
            }

            Times::SegmentRange {
                cases: parsed,
                otherwise: Box::new(source("otherwise")?),
            }
        }
        list if LIST_TYPES.contains(&list) => {
            return Err(FormatError(format!(
                "{path}: \"{list}\" gives a list of times per segment. Put it inside a min, max, average, median, percentile or weighted average block."
            )));
        }
        other => return Err(FormatError(format!("{path}: unknown block \"{other}\"."))),
    })
}

fn read_samples(value: &Value, path: &str) -> Result<Samples> {
    let (obj, type_name) = as_block(value, path)?;
    let source =
        || required(obj, "source", path).and_then(|v| read_samples(v, &join(path, "source")));

    Ok(match type_name {
        "segmentHistory" => Samples::SegmentHistory,
        "lastN" => Samples::LastN {
            source: Box::new(source()?),
            count: read_int(obj, "count", path, 1, MAX_COUNT)?,
        },
        "completedRunsOnly" => Samples::CompletedRunsOnly {
            source: Box::new(source()?),
        },
        "recentDays" => Samples::RecentDays {
            source: Box::new(source()?),
            days: read_int(obj, "days", path, 1, MAX_DAYS)?,
        },
        "trimOutliers" => Samples::TrimOutliers {
            source: Box::new(source()?),
            percent: read_number(obj, "percent", path, 0.0, MAX_TRIM_PERCENT)?,
        },
        "finishedUnder" => {
            let source = source()?;
            Samples::FinishedUnder {
                limit: to_nanos(read_number(
                    obj,
                    "totalSeconds",
                    path,
                    0.001,
                    MAX_TOTAL_SECONDS,
                )?),
                source: Box::new(source),
            }
        }
        "atLeast" => Samples::AtLeast {
            source: Box::new(source()?),
            count: read_int(obj, "count", path, 1, MAX_COUNT)?,
        },
        "reachedSegment" => Samples::ReachedSegment {
            source: Box::new(source()?),
            segment: read_int(obj, "segment", path, 1, MAX_SEGMENT)?,
        },
        "attempt" | "bestRecentRun" | "weightedAverage" | "clamp" | "round" | "comparison"
        | "aggregate" | "percentile" | "addTime" | "scale" | "stretchToTotal" | "blend"
        | "pick" | "fallback" | "segmentRange" => {
            return Err(FormatError(format!(
                "{path}: \"{type_name}\" gives one time per segment, but this slot needs a list of times (segment history, last N, or completed runs only)."
            )));
        }
        other => return Err(FormatError(format!("{path}: unknown block \"{other}\"."))),
    })
}

// ------------------------------------------------------------------ Helpers

fn as_block<'v>(value: &'v Value, path: &str) -> Result<(&'v Map<String, Value>, &'v str)> {
    let obj = value
        .as_object()
        .ok_or_else(|| FormatError(format!("{path}: expected a block.")))?;
    let type_name = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| FormatError(format!("{path}: the block has no \"type\".")))?;
    Ok((obj, type_name))
}

fn required<'v>(obj: &'v Map<String, Value>, key: &str, path: &str) -> Result<&'v Value> {
    obj.get(key)
        .filter(|v| !v.is_null())
        .ok_or_else(|| error(path, key, "is missing"))
}

fn read_string<'v>(obj: &'v Map<String, Value>, key: &str, path: &str) -> Result<&'v str> {
    required(obj, key, path)?
        .as_str()
        .ok_or_else(|| error(path, key, "must be text"))
}

fn read_number(obj: &Map<String, Value>, key: &str, path: &str, min: f64, max: f64) -> Result<f64> {
    required(obj, key, path)?
        .as_f64()
        .filter(|n| (min..=max).contains(n))
        .ok_or_else(|| {
            error(
                path,
                key,
                &format!(
                    "must be a number from {} to {}",
                    fmt_number(min),
                    fmt_number(max)
                ),
            )
        })
}

fn read_int(obj: &Map<String, Value>, key: &str, path: &str, min: u32, max: u32) -> Result<u32> {
    let problem = || {
        if max == u32::MAX || max == i32::MAX as u32 {
            format!("must be a whole number of at least {min}")
        } else {
            format!("must be a whole number from {min} to {max}")
        }
    };
    let n = required(obj, key, path)?
        .as_f64()
        .filter(|n| libm::trunc(*n) == *n && *n >= f64::from(min) && *n <= f64::from(max))
        .ok_or_else(|| error(path, key, &problem()))?;
    Ok(n as u32)
}

fn to_nanos(seconds: f64) -> Nanos {
    libm::round(seconds * NANOS_PER_SECOND as f64) as Nanos
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

fn error(path: &str, key: &str, problem: &str) -> FormatError {
    FormatError(format!("{}: {problem}.", join(path, key)))
}

/// Formats a limit with up to three decimals and no trailing zeros.
fn fmt_number(value: f64) -> String {
    let text = format!("{value:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}
