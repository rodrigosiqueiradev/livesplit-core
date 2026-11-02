//! Tests for custom comparisons. Most of them are ports of the desktop
//! component's engine tests, so both implementations are held to the same
//! behaviour. They build [`RunData`] directly, which keeps each case short and
//! readable; the tests at the end go through a real [`Timer`] instead, to make
//! sure the generator is wired up correctly.

use super::{
    Editor, Library, Program,
    ast::{AggregateKind, NANOS_PER_SECOND, Nanos, PickKind, RangeCase, Samples, Times},
    check_name,
    evaluator::{ComparisonResult, Evaluator},
    format::{self, Document},
    run_data::{AttemptData, RunData, SegmentData},
};
use crate::{
    Layout, Run, Segment, TimeSpan, Timer, TimingMethod,
    component::{custom_comparisons, group},
    layout::ComponentSettings,
    platform::{DateTime, Duration, prelude::*},
    util::tests_helper::run_with_splits,
};
use alloc::collections::BTreeMap;

// ------------------------------------------------------------------ Helpers

fn now() -> DateTime {
    // 2026-10-31 12:00 UTC
    DateTime::from_unix_timestamp(1_793_448_000).unwrap()
}

fn ns(seconds: f64) -> Nanos {
    libm::round(seconds * NANOS_PER_SECOND as f64) as Nanos
}

fn secs(times: &[Option<Nanos>]) -> Vec<Option<f64>> {
    times
        .iter()
        .map(|t| t.map(|t| libm::round(t as f64 / 1e3) / 1e6))
        .collect()
}

/// Builds runs the way LiveSplit records them. Each attempt lists segment
/// times in seconds; a shorter list means the runner reset there, and `None`
/// means the segment was skipped. An attempt is completed only if it has a
/// time for the last segment.
struct RunBuilder {
    names: Vec<&'static str>,
    attempts: Vec<(Vec<Option<f64>>, Option<f64>)>,
    comparisons: BTreeMap<String, Vec<Option<Nanos>>>,
}

impl RunBuilder {
    fn new(names: &[&'static str]) -> Self {
        Self {
            names: names.to_vec(),
            attempts: Vec::new(),
            comparisons: BTreeMap::new(),
        }
    }

    fn attempt(mut self, times: &[Option<f64>]) -> Self {
        self.attempts.push((times.to_vec(), None));
        self
    }

    /// An attempt with every segment present.
    fn a(self, times: &[f64]) -> Self {
        let times: Vec<_> = times.iter().map(|&t| Some(t)).collect();
        self.attempt(&times)
    }

    fn days_ago(mut self, days: f64, times: &[f64]) -> Self {
        self.attempts
            .push((times.iter().map(|&t| Some(t)).collect(), Some(days)));
        self
    }

    /// An existing comparison, as cumulative split times in seconds.
    fn comparison(mut self, name: &str, splits: &[Option<f64>]) -> Self {
        self.comparisons
            .insert(name.into(), splits.iter().map(|s| s.map(ns)).collect());
        self
    }

    fn build(self) -> RunData {
        let mut segments: Vec<SegmentData> = self
            .names
            .iter()
            .map(|n| SegmentData {
                name: (*n).into(),
                history: BTreeMap::new(),
            })
            .collect();
        let mut attempts = BTreeMap::new();
        for (i, (times, days_ago)) in self.attempts.iter().enumerate() {
            let index = i as i32 + 1;
            for (segment, time) in segments.iter_mut().zip(times) {
                segment.history.insert(index, time.map(ns));
            }
            let completed = times.len() == self.names.len() && times.last().unwrap().is_some();
            let final_time = completed.then(|| ns(times.iter().map(|t| t.unwrap_or(0.0)).sum()));
            let when = days_ago.map(|d| now() - Duration::seconds_f64(d * 86_400.0));
            attempts.insert(index, AttemptData { when, final_time });
        }
        RunData {
            segments,
            attempts,
            comparisons: self.comparisons,
            now: now(),
        }
    }
}

fn eval(root: Times, run: &RunData) -> ComparisonResult {
    let program = Program {
        name: "Test".into(),
        root,
    };
    Evaluator::new([&program]).evaluate("Test", run).unwrap()
}

fn seg(root: Times, run: &RunData) -> Vec<Option<f64>> {
    let result = eval(root, run);
    assert!(result.succeeded(), "{:?}", result.error);
    secs(&result.segment_times)
}

fn history() -> Box<Samples> {
    Box::new(Samples::SegmentHistory)
}

fn cmp(name: &str) -> Box<Times> {
    Box::new(Times::Comparison { name: name.into() })
}

fn agg(kind: AggregateKind, source: Box<Samples>) -> Times {
    Times::Aggregate { kind, source }
}

fn last_n(count: u32, source: Box<Samples>) -> Box<Samples> {
    Box::new(Samples::LastN { count, source })
}

fn completed(source: Box<Samples>) -> Box<Samples> {
    Box::new(Samples::CompletedRunsOnly { source })
}

// ------------------------------------------------------------------ History

#[test]
fn median_of_all_times() {
    let run = RunBuilder::new(&["A", "B"])
        .a(&[10.0, 20.0])
        .a(&[12.0, 22.0])
        .a(&[11.0, 30.0])
        .build();
    assert_eq!(
        seg(agg(AggregateKind::Median, history()), &run),
        [Some(11.0), Some(22.0)]
    );
}

#[test]
fn aggregates_min_max_average_and_even_median() {
    let run = RunBuilder::new(&["A"])
        .a(&[10.0])
        .a(&[20.0])
        .a(&[30.0])
        .a(&[40.0])
        .build();
    assert_eq!(seg(agg(AggregateKind::Min, history()), &run), [Some(10.0)]);
    assert_eq!(seg(agg(AggregateKind::Max, history()), &run), [Some(40.0)]);
    assert_eq!(
        seg(agg(AggregateKind::Average, history()), &run),
        [Some(25.0)]
    );
    assert_eq!(
        seg(agg(AggregateKind::Median, history()), &run),
        [Some(25.0)]
    );
}

#[test]
fn skipped_segment_and_the_combined_one_after_it_are_left_out() {
    let run = RunBuilder::new(&["A", "B", "C"])
        .attempt(&[Some(10.0), None, Some(50.0)])
        .a(&[11.0, 20.0, 30.0])
        .build();
    assert_eq!(
        seg(agg(AggregateKind::Max, history()), &run),
        [Some(11.0), Some(20.0), Some(30.0)]
    );
}

#[test]
fn reset_attempts_contribute_until_they_reset() {
    let run = RunBuilder::new(&["A", "B"])
        .a(&[5.0])
        .a(&[10.0, 20.0])
        .build();
    assert_eq!(
        seg(agg(AggregateKind::Min, history()), &run),
        [Some(5.0), Some(20.0)]
    );
}

#[test]
fn empty_segment_empties_every_later_split() {
    let run = RunBuilder::new(&["A", "B", "C"]).a(&[10.0]).build();
    let result = eval(agg(AggregateKind::Min, history()), &run);
    assert_eq!(secs(&result.segment_times), [Some(10.0), None, None]);
    assert_eq!(secs(&result.split_times), [Some(10.0), None, None]);
}

// ------------------------------------------------------------------ Filters

#[test]
fn last_n_counts_completions_of_the_segment() {
    let run = RunBuilder::new(&["A", "B"])
        .a(&[1.0, 1.0])
        .a(&[2.0])
        .a(&[3.0])
        .a(&[4.0, 4.0])
        .build();
    let root = agg(AggregateKind::Min, last_n(2, history()));
    assert_eq!(seg(root, &run), [Some(3.0), Some(1.0)]);
}

#[test]
fn completed_runs_only() {
    let run = RunBuilder::new(&["A", "B"])
        .a(&[1.0])
        .a(&[5.0, 5.0])
        .build();
    assert_eq!(
        seg(agg(AggregateKind::Min, completed(history())), &run),
        [Some(5.0), Some(5.0)]
    );
}

#[test]
fn recent_days_drops_old_and_undated_attempts() {
    let run = RunBuilder::new(&["A"])
        .days_ago(40.0, &[1.0])
        .days_ago(10.0, &[5.0])
        .a(&[2.0])
        .build();
    let root = agg(
        AggregateKind::Min,
        Box::new(Samples::RecentDays {
            days: 30,
            source: history(),
        }),
    );
    assert_eq!(seg(root, &run), [Some(5.0)]);
}

fn ten_attempts() -> RunData {
    let mut builder = RunBuilder::new(&["A"]);
    for t in [50.0, 1.0, 20.0, 21.0, 22.0, 23.0, 24.0, 25.0, 26.0, 99.0] {
        builder = builder.a(&[t]);
    }
    builder.build()
}

fn trim(percent: f64, source: Box<Samples>) -> Box<Samples> {
    Box::new(Samples::TrimOutliers { percent, source })
}

#[test]
fn trim_drops_both_ends_and_keeps_attempt_order() {
    let run = ten_attempts();
    assert_eq!(
        seg(agg(AggregateKind::Min, trim(10.0, history())), &run),
        [Some(20.0)]
    );
    assert_eq!(
        seg(agg(AggregateKind::Max, trim(10.0, history())), &run),
        [Some(50.0)]
    );
    // After dropping 1 and 99, "last 2" still means the newest survivors.
    let root = agg(AggregateKind::Average, last_n(2, trim(10.0, history())));
    assert_eq!(seg(root, &run), [Some(25.5)]);
}

#[test]
fn trim_leaves_small_lists_alone() {
    let run = RunBuilder::new(&["A"])
        .a(&[1.0])
        .a(&[5.0])
        .a(&[9.0])
        .build();
    assert_eq!(
        seg(agg(AggregateKind::Min, trim(10.0, history())), &run),
        [Some(1.0)]
    );
}

#[test]
fn finished_under_keeps_only_faster_runs() {
    let run = RunBuilder::new(&["A", "B"])
        .a(&[10.0, 50.0])
        .a(&[12.0, 40.0])
        .a(&[11.0])
        .a(&[30.0, 30.0])
        .build();
    let root = agg(
        AggregateKind::Max,
        Box::new(Samples::FinishedUnder {
            limit: ns(60.0),
            source: history(),
        }),
    );
    assert_eq!(seg(root, &run), [Some(12.0), Some(40.0)]);
}

#[test]
fn at_least_empties_thin_segments() {
    let run = RunBuilder::new(&["A", "B"])
        .a(&[1.0, 2.0])
        .a(&[3.0])
        .build();
    let root = agg(
        AggregateKind::Min,
        Box::new(Samples::AtLeast {
            count: 2,
            source: history(),
        }),
    );
    assert_eq!(seg(root, &run), [Some(1.0), None]);
}

#[test]
fn reached_segment_keeps_attempts_that_got_that_far() {
    let run = RunBuilder::new(&["A", "B", "C"])
        .a(&[10.0])
        .a(&[20.0, 5.0])
        .a(&[30.0, 6.0, 7.0])
        .build();
    let reached = |segment| {
        agg(
            AggregateKind::Max,
            Box::new(Samples::ReachedSegment {
                segment,
                source: history(),
            }),
        )
    };
    assert_eq!(seg(reached(2), &run), [Some(30.0), Some(6.0), Some(7.0)]);
    assert_eq!(seg(reached(9), &run), [None, None, None]);
}

#[test]
fn reached_segment_counts_a_skip_as_reached() {
    let run = RunBuilder::new(&["A", "B", "C"])
        .attempt(&[Some(10.0), None, Some(20.0)])
        .a(&[99.0])
        .build();
    let root = agg(
        AggregateKind::Max,
        Box::new(Samples::ReachedSegment {
            segment: 2,
            source: history(),
        }),
    );
    assert_eq!(seg(root, &run)[0], Some(10.0));
}

// ------------------------------------------------------------------ Aggregates

#[test]
fn percentile_interpolates_and_50_is_the_median() {
    let run = RunBuilder::new(&["A"])
        .a(&[10.0])
        .a(&[20.0])
        .a(&[30.0])
        .a(&[40.0])
        .build();
    let p = |percent| Times::Percentile {
        percent,
        source: history(),
    };
    assert_eq!(seg(p(0.0), &run), [Some(10.0)]);
    assert_eq!(seg(p(50.0), &run), [Some(25.0)]);
    assert_eq!(seg(p(100.0), &run), [Some(40.0)]);
    assert_eq!(seg(p(70.0), &run), [Some(31.0)]);
}

#[test]
fn weighted_average_halves_every_half_life() {
    let run = RunBuilder::new(&["A"])
        .a(&[40.0])
        .a(&[20.0])
        .a(&[10.0])
        .build();
    let root = Times::WeightedAverage {
        half_life: 1,
        source: history(),
    };
    // Newest first: 10 (weight 1), 20 (0.5), 40 (0.25).
    let expected = libm::round(30.0 / 1.75 * 1e6) / 1e6;
    assert_eq!(seg(root, &run), [Some(expected)]);
}

// ------------------------------------------------------------------ Times

#[test]
fn comparisons_become_segment_times() {
    let run = RunBuilder::new(&["A", "B", "C"])
        .comparison("PB", &[Some(10.0), Some(25.0), Some(45.0)])
        .build();
    assert_eq!(
        seg(Times::Comparison { name: "PB".into() }, &run),
        [Some(10.0), Some(15.0), Some(20.0)]
    );
}

#[test]
fn unknown_comparison_is_an_error() {
    let run = RunBuilder::new(&["A"]).build();
    let result = eval(
        Times::Comparison {
            name: "Nope".into(),
        },
        &run,
    );
    assert_eq!(
        result.error.as_deref(),
        Some("There is no comparison named 'Nope'.")
    );
    assert_eq!(result.split_times, [None]);
}

#[test]
fn attempt_and_missing_attempt() {
    let run = RunBuilder::new(&["A", "B"])
        .a(&[1.0, 2.0])
        .a(&[3.0])
        .build();
    assert_eq!(seg(Times::Attempt { number: 2 }, &run), [Some(3.0), None]);
    let result = eval(Times::Attempt { number: 7 }, &run);
    assert_eq!(
        result.error.as_deref(),
        Some("There's no attempt #7 in these splits.")
    );
}

#[test]
fn best_recent_run() {
    let run = RunBuilder::new(&["A", "B"])
        .days_ago(60.0, &[5.0, 5.0])
        .days_ago(10.0, &[10.0, 12.0])
        .days_ago(5.0, &[9.0, 11.0])
        .days_ago(1.0, &[1.0])
        .build();
    assert_eq!(
        seg(Times::BestRecentRun { days: 30 }, &run),
        [Some(9.0), Some(11.0)]
    );
    assert_eq!(seg(Times::BestRecentRun { days: 2 }, &run), [None, None]);
}

fn abc() -> RunData {
    RunBuilder::new(&["A", "B", "C"])
        .comparison("S", &[Some(5.0), Some(25.0), Some(35.0)])
        .comparison("L", &[Some(8.0), Some(16.0), Some(24.0)])
        .comparison("H", &[Some(9.0), Some(21.0), Some(33.0)])
        .build()
}

#[test]
fn add_time_never_goes_below_zero() {
    let root = Times::AddTime {
        amount: ns(-6.0),
        source: cmp("S"),
    };
    assert_eq!(seg(root, &abc()), [Some(0.0), Some(14.0), Some(4.0)]);
}

#[test]
fn scale_and_blend() {
    let scale = Times::Scale {
        percent: 110.0,
        source: cmp("S"),
    };
    assert_eq!(seg(scale, &abc()), [Some(5.5), Some(22.0), Some(11.0)]);
    let blend = Times::Blend {
        percent: 25.0,
        from: cmp("S"),
        to: cmp("L"),
    };
    assert_eq!(seg(blend, &abc()), [Some(5.75), Some(17.0), Some(9.5)]);
}

#[test]
fn stretch_hits_the_goal_exactly() {
    let root = Times::StretchToTotal {
        total: ns(100.0),
        source: cmp("S"),
    };
    let result = eval(root, &abc());
    assert_eq!(result.split_times[2], Some(ns(100.0)));
}

#[test]
fn stretch_is_empty_when_a_segment_is_empty() {
    let run = RunBuilder::new(&["A", "B"]).a(&[10.0]).build();
    let root = Times::StretchToTotal {
        total: ns(100.0),
        source: Box::new(agg(AggregateKind::Min, history())),
    };
    assert_eq!(seg(root, &run), [None, None]);
}

#[test]
fn pick_faster_and_slower() {
    let pick = |kind| Times::Pick {
        kind,
        first: cmp("S"),
        second: cmp("L"),
    };
    assert_eq!(
        seg(pick(PickKind::Faster), &abc()),
        [Some(5.0), Some(8.0), Some(8.0)]
    );
    assert_eq!(
        seg(pick(PickKind::Slower), &abc()),
        [Some(8.0), Some(20.0), Some(10.0)]
    );
}

#[test]
fn clamp_keeps_between_limits() {
    let root = Times::Clamp {
        source: cmp("S"),
        low: cmp("L"),
        high: cmp("H"),
    };
    assert_eq!(seg(root, &abc()), [Some(8.0), Some(12.0), Some(10.0)]);
}

#[test]
fn clamp_low_wins_when_limits_cross_and_empty_limits_dont_apply() {
    let run = RunBuilder::new(&["A", "B", "C"])
        .comparison("S", &[Some(50.0), Some(90.0), Some(160.0)])
        .comparison("L", &[Some(20.0), Some(40.0), Some(60.0)])
        .comparison("H", &[Some(100.0), Some(115.0), None])
        .build();
    let root = Times::Clamp {
        source: cmp("S"),
        low: cmp("L"),
        high: cmp("H"),
    };
    assert_eq!(seg(root, &run), [Some(50.0), Some(20.0), Some(70.0)]);
}

#[test]
fn round_to_the_nearest_step() {
    let run = RunBuilder::new(&["A", "B"])
        .comparison("X", &[Some(10.26), Some(20.68)])
        .build();
    let root = Times::Round {
        step: ns(0.1),
        source: cmp("X"),
    };
    let result = eval(root, &run);
    assert_eq!(secs(&result.segment_times), [Some(10.3), Some(10.4)]);
    assert_eq!(secs(&result.split_times), [Some(10.3), Some(20.7)]);
}

#[test]
fn fallback_fills_only_gaps() {
    let run = RunBuilder::new(&["A", "B"])
        .a(&[10.0])
        .comparison("PB", &[Some(9.0), Some(30.0)])
        .build();
    let root = Times::Fallback {
        primary: Box::new(agg(AggregateKind::Min, history())),
        fallback: cmp("PB"),
    };
    assert_eq!(seg(root, &run), [Some(10.0), Some(21.0)]);
}

#[test]
fn segment_range_first_match_wins() {
    let root = Times::SegmentRange {
        cases: vec![
            RangeCase {
                from: 1,
                to: 2,
                times: Times::Comparison { name: "L".into() },
            },
            RangeCase {
                from: 2,
                to: 3,
                times: Times::Comparison { name: "H".into() },
            },
        ],
        otherwise: cmp("S"),
    };
    assert_eq!(seg(root, &abc()), [Some(8.0), Some(8.0), Some(12.0)]);
}

// ------------------------------------------------------------------ Programs

#[test]
fn programs_can_use_each_other() {
    let base = Program {
        name: "Base".into(),
        root: Times::AddTime {
            amount: ns(1.0),
            source: cmp("S"),
        },
    };
    let top = Program {
        name: "Top".into(),
        root: Times::Scale {
            percent: 200.0,
            source: cmp("Base"),
        },
    };
    let result = Evaluator::new([&top, &base])
        .evaluate("Top", &abc())
        .unwrap();
    assert_eq!(
        secs(&result.segment_times),
        [Some(12.0), Some(42.0), Some(22.0)]
    );
}

#[test]
fn loops_are_reported() {
    let a = Program {
        name: "A".into(),
        root: Times::Comparison { name: "B".into() },
    };
    let b = Program {
        name: "B".into(),
        root: Times::Comparison { name: "A".into() },
    };
    let result = Evaluator::new([&a, &b]).evaluate("A", &abc()).unwrap();
    assert_eq!(
        result.error.as_deref(),
        Some("These comparisons depend on each other in a loop: A → B → A")
    );
}

#[test]
fn origins_follow_the_branch_taken() {
    let run = RunBuilder::new(&["A", "B"])
        .a(&[10.0])
        .comparison("PB", &[Some(9.0), Some(30.0)])
        .build();
    let program = Program {
        name: "T".into(),
        root: Times::Fallback {
            primary: Box::new(Times::AddTime {
                amount: ns(1.0),
                source: Box::new(agg(AggregateKind::Min, history())),
            }),
            fallback: cmp("PB"),
        },
    };
    let result = Evaluator::new([&program])
        .evaluate_with_origins("T", &run)
        .unwrap();
    assert_eq!(result.origins.unwrap(), ["primary.source", "fallback"]);
}

#[test]
fn clamp_origins_point_at_the_limit_used() {
    let program = Program {
        name: "T".into(),
        root: Times::Clamp {
            source: cmp("S"),
            low: cmp("L"),
            high: cmp("H"),
        },
    };
    let result = Evaluator::new([&program])
        .evaluate_with_origins("T", &abc())
        .unwrap();
    assert_eq!(result.origins.unwrap(), ["low", "high", "source"]);
}

// ------------------------------------------------------------------ Names

#[test]
fn names() {
    assert_eq!(check_name("Fine", ["Other"]), None);
    assert!(check_name("  ", []).is_some());
    assert!(check_name("best segments", []).is_some());
    assert!(check_name("[race] mine", []).is_some());
    assert!(check_name("Taken", ["Taken"]).is_some());
    assert!(check_name(&"x".repeat(65), []).is_some());
}

// ------------------------------------------------------------------ Format

const SAFE_PACE: &str = r#"{
  "format": "livesplit-custom-comparison",
  "version": 1,
  "name": "Safe Pace",
  "root": {
    "type": "fallback",
    "primary": {
      "type": "segmentRange",
      "cases": [
        {
          "from": 1,
          "to": 3,
          "times": {
            "type": "addTime",
            "seconds": 0.5,
            "source": { "type": "comparison", "name": "Best Segments" }
          }
        }
      ],
      "otherwise": {
        "type": "aggregate",
        "kind": "median",
        "source": {
          "type": "lastN",
          "count": 10,
          "source": { "type": "completedRunsOnly", "source": { "type": "segmentHistory" } }
        }
      }
    },
    "fallback": { "type": "comparison", "name": "Personal Best" }
  },
  "workspace": { "blocks": { "languageVersion": 0 } }
}"#;

#[test]
fn reads_a_desktop_file_and_keeps_the_workspace() {
    let document = format::read(SAFE_PACE).unwrap();
    assert_eq!(document.program.name, "Safe Pace");
    assert!(matches!(document.program.root, Times::Fallback { .. }));
    assert!(document.workspace.is_some());
}

#[test]
fn round_trips_and_writes_the_lowest_version() {
    let document = format::read(SAFE_PACE).unwrap();
    let once = format::write(&document);
    assert_eq!(format::read(&once).unwrap(), document);
    assert!(once.contains("\"version\": 1"));
    assert!(once.contains("\"seconds\": 0.5"));
    assert!(once.contains("\"count\": 10"));
}

#[test]
fn every_version_4_block_round_trips() {
    let root = Times::Round {
        step: ns(0.1),
        source: Box::new(Times::Clamp {
            source: Box::new(Times::WeightedAverage {
                half_life: 5,
                source: Box::new(Samples::ReachedSegment {
                    segment: 3,
                    source: history(),
                }),
            }),
            low: cmp("Best Segments"),
            high: Box::new(Times::BestRecentRun { days: 30 }),
        }),
    };
    let document = Document {
        program: Program {
            name: "V4".into(),
            root,
        },
        workspace: None,
    };
    let text = format::write(&document);
    assert!(text.contains("\"version\": 4"));
    assert_eq!(format::read(&text).unwrap(), document);
}

#[test]
fn errors_name_the_place() {
    let wrap = |root: &str| {
        format!(
            r#"{{"format":"livesplit-custom-comparison","version":4,"name":"T","root":{root}}}"#
        )
    };
    let error = |root: &str| format::read(&wrap(root)).unwrap_err().0;

    assert_eq!(
        error(
            r#"{"type":"aggregate","kind":"min","source":{"type":"lastN","count":0,"source":{"type":"segmentHistory"}}}"#
        ),
        "root.source.count: must be a whole number from 1 to 10000."
    );
    assert!(error(r#"{"type":"segmentHistory"}"#).contains(
        "Put it inside a min, max, average, median, percentile or weighted average block"
    ));
    assert!(
        error(r#"{"type":"aggregate","kind":"min","source":{"type":"comparison","name":"PB"}}"#)
            .starts_with("root.source:")
    );
    assert!(error(r#"{"type":"clamp","source":{"type":"comparison","name":"X"},"low":{"type":"comparison","name":"Y"}}"#).starts_with("root.high"));
    assert!(error(r#"{"type":"nope"}"#).contains("unknown block"));
}

#[test]
fn newer_versions_and_other_files_are_refused() {
    let newer = SAFE_PACE.replace("\"version\": 1", "\"version\": 99");
    assert!(
        format::read(&newer)
            .unwrap_err()
            .0
            .contains("newer version")
    );
    assert_eq!(
        format::read("{}").unwrap_err().0,
        "This isn't a Custom Comparisons file."
    );
    assert!(
        format::read("not json")
            .unwrap_err()
            .0
            .starts_with("This isn't valid JSON")
    );
}

// ------------------------------------------------------------------ Generator

fn timer_with(programs: Vec<Program>) -> Timer {
    let mut run = Run::new();
    run.push_segment(Segment::new("A"));
    run.push_segment(Segment::new("B"));
    for generator in Library::new(programs).generators() {
        run.comparison_generators_mut().push(generator);
    }
    Timer::new(run).unwrap()
}

#[test]
fn generator_fills_the_comparison_after_each_attempt() {
    let program = Program {
        name: "Fastest +1s".into(),
        root: Times::AddTime {
            amount: ns(1.0),
            source: Box::new(agg(AggregateKind::Min, history())),
        },
    };
    let mut timer = timer_with(vec![program]);
    run_with_splits(&mut timer, &[10.0, 25.0]);
    run_with_splits(&mut timer, &[8.0, 30.0]);

    let s = TimeSpan::from_seconds;
    let run = timer.run();
    assert_eq!(
        run.segment(0).comparison("Fastest +1s").game_time,
        Some(s(9.0))
    );
    assert_eq!(
        run.segment(1).comparison("Fastest +1s").game_time,
        Some(s(25.0))
    );
}

#[test]
fn generator_can_use_built_ins_that_are_switched_off() {
    let program = Program {
        name: "Golds +1s".into(),
        root: Times::AddTime {
            amount: ns(1.0),
            source: cmp("Best Segments"),
        },
    };
    let mut timer = timer_with(vec![program]);
    run_with_splits(&mut timer, &[10.0, 25.0]);
    run_with_splits(&mut timer, &[8.0, 30.0]);

    // Keep only the custom generator: Best Segments is calculated anyway.
    let mut run = timer.run().clone();
    run.comparison_generators_mut()
        .retain(|g| g.name() == "Golds +1s");
    run.regenerate_comparisons();

    let s = TimeSpan::from_seconds;
    assert_eq!(
        run.segment(0).comparison("Golds +1s").game_time,
        Some(s(9.0))
    );
    assert_eq!(
        run.segment(1).comparison("Golds +1s").game_time,
        Some(s(25.0))
    );
}

#[test]
fn generator_handles_both_timing_methods() {
    let program = Program {
        name: "PB".into(),
        root: Times::Comparison {
            name: "Personal Best".into(),
        },
    };
    let mut timer = timer_with(vec![program]);
    run_with_splits(&mut timer, &[10.0, 25.0]);
    let run = timer.run();
    let time = run.segment(1).comparison("PB");
    assert_eq!(
        time.game_time,
        run.segment(1).personal_best_split_time().game_time
    );
    assert_eq!(
        time[TimingMethod::RealTime],
        run.segment(1).personal_best_split_time().real_time
    );
}

// ------------------------------------------------------------------ Layout

fn program_json(name: &str) -> String {
    format::write(&Document {
        program: Program {
            name: name.into(),
            root: Times::Comparison {
                name: "Personal Best".into(),
            },
        },
        workspace: None,
    })
}

fn layout_with(comparisons: &[&str]) -> Layout {
    let component = custom_comparisons::Component::with_settings(custom_comparisons::Settings {
        comparisons: comparisons.iter().map(|c| c.to_string()).collect(),
    });
    let mut group = group::Component::new();
    group.components.push(component.into());
    let mut layout = Layout::new();
    layout.push(group);
    layout
}

fn has_comparison(timer: &Timer, name: &str) -> bool {
    timer.run().comparisons().any(|c| c == name)
}

#[test]
fn layout_comparisons_are_installed_and_removed() {
    let (a, b) = (program_json("A"), program_json("B"));
    let layout = layout_with(&[&a, &b]);
    let mut timer = timer_with(Vec::new());
    run_with_splits(&mut timer, &[10.0, 25.0]);

    assert!(
        timer
            .set_custom_comparisons(layout.custom_comparisons())
            .is_empty()
    );
    assert!(has_comparison(&timer, "A") && has_comparison(&timer, "B"));
    // Calculated right away, not only after the next attempt.
    assert_eq!(
        timer.run().segment(1).comparison("A").game_time,
        Some(TimeSpan::from_seconds(25.0))
    );

    // Installing again replaces them instead of adding them twice.
    timer.set_custom_comparisons(layout_with(&[&a]).custom_comparisons());
    timer.set_current_comparison("A").unwrap();
    let names: Vec<_> = timer.run().comparisons().filter(|c| *c == "A").collect();
    assert_eq!(names.len(), 1);
    assert!(!has_comparison(&timer, "B"));

    // Removing the selected comparison falls back to the Personal Best.
    timer.set_custom_comparisons(Library::default());
    assert!(!has_comparison(&timer, "A"));
    assert_eq!(timer.current_comparison(), "Personal Best");
    assert!(has_comparison(&timer, "Best Segments"));
}

#[test]
fn layout_comparison_named_like_one_in_the_splits_is_left_out() {
    let mut timer = timer_with(Vec::new());
    let mut run = timer.run().clone();
    run.add_custom_comparison("World Record").unwrap();
    timer.set_run(run).unwrap();

    let layout = layout_with(&[&program_json("World Record"), &program_json("Mine")]);
    let left_out = timer.set_custom_comparisons(layout.custom_comparisons());
    assert_eq!(left_out, ["World Record"]);
    assert!(has_comparison(&timer, "Mine"));
    let count = timer
        .run()
        .comparisons()
        .filter(|c| *c == "World Record")
        .count();
    assert_eq!(count, 1);
}

#[test]
fn unreadable_layout_comparison_is_kept_verbatim() {
    let future = r#"{"format":"livesplit-custom-comparison","version":99,"name":"Future Pace"}"#;
    let layout = layout_with(&[&program_json("Kept"), future]);
    let library = layout.custom_comparisons();
    assert_eq!(library.programs().len(), 1);
    assert_eq!(library.programs()[0].name, "Kept");

    let json = serde_json::to_string(&layout.settings()).unwrap();
    let reloaded = Layout::from_settings(serde_json::from_str(&json).unwrap());
    let ComponentSettings::Group(group) = &reloaded.settings().components[0] else {
        panic!("expected a group");
    };
    let ComponentSettings::CustomComparisons(settings) = &group.components[0] else {
        panic!("expected the Custom Comparisons component");
    };
    assert_eq!(settings.comparisons[1], future);
}

// ------------------------------------------------------------------ Editor

fn editor_with(comparisons: &[&str]) -> (Editor, Timer) {
    let mut timer = timer_with(Vec::new());
    run_with_splits(&mut timer, &[10.0, 25.0]);
    let mut run = timer.run().clone();
    run.add_custom_comparison("World Record").unwrap();
    timer.set_run(run).unwrap();
    let editor = Editor::new(comparisons.iter().map(|c| c.to_string()), timer.run());
    (editor, timer)
}

fn editor_names(editor: &Editor) -> Vec<String> {
    (0..editor.len()).map(|i| editor.name(i)).collect()
}

fn using(name: &str, uses: &str) -> String {
    format::write_compact(&Document {
        program: Program {
            name: name.into(),
            root: Times::Fallback {
                primary: cmp(uses),
                fallback: cmp("Personal Best"),
            },
        },
        workspace: Some(serde_json::json!({
            "blocks": { "blocks": [{
                "type": "cc_fallback",
                "inputs": { "PRIMARY": { "block": {
                    "type": "cc_comparison",
                    "fields": { "NAME": uses },
                }}},
            }]},
        })),
    })
}

#[test]
fn editor_adds_comparisons_with_free_names_only() {
    let (mut editor, _timer) = editor_with(&[]);
    assert_eq!(editor.add(&program_json("Pace")), Ok(()));
    assert_eq!(
        editor.add(&program_json("pace")),
        Err("There's already a comparison called 'pace'.".into())
    );
    assert_eq!(editor.suggest_name("pace"), "pace (2)");
    assert_eq!(editor.add_as(&program_json("pace"), " Pace (2) "), Ok(()));
    assert!(editor.add(&program_json("Best Segments")).is_err());
    // A comparison stored in the splits can't be shadowed.
    assert_eq!(
        editor.add(&program_json("World Record")),
        Err("There's already a comparison called 'World Record'.".into())
    );
    assert_eq!(
        editor.add("nope"),
        Err("This isn't valid JSON: expected ident at line 1 column 2".into())
    );
    assert_eq!(editor_names(&editor), ["Pace", "Pace (2)"]);
    assert_eq!(editor.check_name("Pace", Some(0)), None);
    assert!(editor.check_name("Pace", Some(1)).is_some());

    // Stored on a single line, like the desktop component stores them.
    let stored = editor.close();
    assert!(stored.iter().all(|s| !s.contains('\n')));
}

#[test]
fn editor_rename_updates_the_comparisons_that_use_it() {
    let unrelated = using("Unrelated", "Best Segments");
    let (mut editor, _timer) =
        editor_with(&[&program_json("Base"), &using("User", "base"), &unrelated]);
    assert_eq!(editor.used_by(0), 1);

    assert_eq!(
        editor.rename(0, "Personal Best"),
        Err("'Personal Best' is one of LiveSplit's own comparisons. Pick another name.".into())
    );
    assert_eq!(editor.rename(0, " Safe "), Ok(()));
    assert_eq!(editor_names(&editor), ["Safe", "User", "Unrelated"]);
    assert_eq!(editor.used_by(0), 1);

    let stored = editor.close();
    let user = format::read(&stored[1]).unwrap();
    assert_eq!(
        user.program.root,
        Times::Fallback {
            primary: cmp("Safe"),
            fallback: cmp("Personal Best"),
        }
    );
    let workspace = user.workspace.unwrap();
    assert_eq!(
        workspace["blocks"]["blocks"][0]["inputs"]["PRIMARY"]["block"]["fields"]["NAME"],
        "Safe"
    );
    // Comparisons that don't use it stay exactly as they were.
    assert_eq!(stored[2], unrelated);
}

#[test]
fn editor_keeps_unreadable_comparisons_and_flags_duplicates() {
    let future = r#"{"format":"livesplit-custom-comparison","version":99,"name":"Future Pace"}"#;
    let (mut editor, timer) = editor_with(&[future, &program_json("Twin"), &program_json("TWIN")]);
    assert_eq!(editor_names(&editor), ["Future Pace", "Twin", "TWIN"]);
    assert_eq!(editor.export(0), future);
    assert_eq!(
        editor.rename(0, "Other"),
        Err("This comparison can't be read, so it can't be renamed.".into())
    );

    let state = editor.state(timer.run());
    assert!(!state.comparisons[0].readable);
    assert!(state.comparisons[1].problem.is_none());
    assert_eq!(
        state.comparisons[2].problem.as_deref(),
        Some("Another comparison is already called 'TWIN'. Rename one of them.")
    );
    assert_eq!(editor.library().programs().len(), 1);

    // Once the first one is gone, the second one works again.
    editor.remove(1);
    assert!(editor.state(timer.run()).comparisons[1].readable);
    assert!(editor.move_up(1));
    assert!(!editor.move_up(0));
    assert!(!editor.move_down(1));
    assert_eq!(editor.close()[1], future);
}

#[test]
fn editor_state_reports_comparisons_that_dont_work() {
    let (editor, timer) = editor_with(&[
        &program_json("Fine"),
        &using("Broken", "Nowhere"),
        &using("Fine Too", "Fine"),
    ]);
    let state = editor.state(timer.run());
    assert_eq!(state.comparisons[0].problem, None);
    assert_eq!(state.comparisons[0].used_by, 1);
    assert!(
        state.comparisons[1]
            .problem
            .as_deref()
            .unwrap()
            .contains("Nowhere")
    );
    assert_eq!(state.comparisons[2].problem, None);
}

#[test]
fn editor_preview_shows_times_and_where_they_came_from() {
    let (editor, timer) = editor_with(&[&program_json("Base")]);
    let preview = editor.preview(
        &using("Draft", "Base"),
        None,
        timer.run(),
        TimingMethod::GameTime,
    );
    assert_eq!(preview.error, None);
    let segments: Vec<_> = preview
        .segments
        .iter()
        .map(|s| {
            (
                s.name.as_str(),
                s.segment_time,
                s.split_time,
                s.origin.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        segments,
        [
            ("A", Some(10.0), Some(10.0), Some("primary")),
            ("B", Some(15.0), Some(25.0), Some("primary")),
        ]
    );

    // The draft replaces the comparison it's editing.
    let draft = using("Base", "Base");
    let preview = editor.preview(&draft, Some(0), timer.run(), TimingMethod::GameTime);
    assert!(preview.error.unwrap().contains("Base"));

    let preview = editor.preview("{}", None, timer.run(), TimingMethod::GameTime);
    assert_eq!(
        preview.error.as_deref(),
        Some("This isn't a Custom Comparisons file.")
    );
    assert_eq!(preview.segments.len(), 2);
    assert!(preview.segments.iter().all(|s| s.split_time.is_none()));
}

#[test]
fn layout_editor_reads_and_stores_the_selected_components_comparisons() {
    let mut layout = Layout::new();
    layout.push(crate::component::blank_space::Component::new());
    layout.push(custom_comparisons::Component::with_settings(
        custom_comparisons::Settings {
            comparisons: vec![program_json("A")],
        },
    ));
    let mut editor = crate::LayoutEditor::new(layout).unwrap();
    editor.select(0);
    assert_eq!(editor.custom_comparisons(), None);
    assert!(!editor.set_custom_comparisons(Vec::new()));

    editor.select(1);
    assert_eq!(editor.custom_comparisons(), Some(&[program_json("A")][..]));
    assert!(editor.set_custom_comparisons(vec![program_json("B")]));
    let layout = editor.close();
    assert_eq!(layout.custom_comparisons().programs()[0].name, "B");
}
