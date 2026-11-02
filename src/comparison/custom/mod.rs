//! Custom comparisons: comparisons the runner builds themselves out of blocks.
//!
//! A custom comparison is a small program, like "the median of my last 10
//! finished runs, never faster than my golds, gaps filled from my PB". It is
//! pure data: a tree of blocks from a fixed set ([`ast`]), stored in a
//! versioned JSON format ([`format`]) that is shared with the desktop
//! LiveSplit component *Custom Comparisons*, so the same file works in both.
//!
//! # How it fits into livesplit-core
//!
//! Each custom comparison becomes one [`CustomComparison`] comparison
//! generator. Like the built-in generators, it recalculates its comparison
//! between attempts, for both timing methods. All the custom comparisons of a
//! run share one [`Library`], because a program may use another custom
//! comparison as an input. Those references are resolved by evaluating the
//! other program directly (with loop detection), never by reading back what
//! another generator wrote, so the order of the generators doesn't matter.
//!
//! Programs may also use the built-in generated comparisons (Best Segments,
//! Average Segments, ...). Those are calculated on a copy of the segments
//! when needed, so they work even if the runner removed those generators
//! from their run, which is exactly what someone relying solely on custom
//! comparisons would do.
//!
//! # Rules
//!
//! The rules are deliberately conservative, because a comparison that
//! silently invents times is worse than one with a visible gap:
//!
//! - Empty segments stay empty unless a fallback block fills them, and once
//!   a segment is empty, every later split time is empty too.
//! - The segment history follows LiveSplit's own rules for skipped segments.
//! - A program that can't be evaluated (a loop, an attempt that doesn't
//!   exist) leaves its comparison empty, and the error can be shown by the
//!   editor through [`preview`].

pub mod ast;
pub mod editor;
pub mod evaluator;
pub mod format;
pub mod run_data;

#[cfg(test)]
mod tests;

pub use self::{
    ast::Program,
    editor::Editor,
    evaluator::{ComparisonResult, Evaluator},
    format::{Document, FormatError},
};

use self::{
    ast::Nanos,
    run_data::{AttemptData, RunData, SegmentData},
};
use super::{ComparisonGenerator, default_generators, personal_best};
use crate::{
    AtomicDateTime, Attempt, Run, Segment, TimeSpan, TimingMethod,
    platform::{Arc, DateTime, prelude::*},
};
use alloc::collections::{BTreeMap, BTreeSet};

/// The comparisons LiveSplit generates itself, which custom comparisons can
/// use but not be named after. Golf HCP is included because the desktop
/// LiveSplit offers it, and a file made there must not clash with it.
pub const RESERVED_NAMES: &[&str] = &[
    personal_best::NAME,
    super::none::NAME,
    super::best_segments::NAME,
    super::best_split_times::NAME,
    super::average_segments::NAME,
    super::median_segments::NAME,
    super::worst_segments::NAME,
    super::balanced_pb::NAME,
    super::latest_run::NAME,
    "Golf HCP",
];

/// Checks whether `name` can be used for a new custom comparison. Returns a
/// message for the runner if it can't. `taken` are the names of the run's
/// other comparisons (leave out the one being renamed).
pub fn check_name<'a>(name: &str, taken: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Some("Give the comparison a name.".into());
    }
    if trimmed.chars().count() > format::MAX_NAME_LENGTH {
        return Some(format!(
            "Keep the name under {} characters.",
            format::MAX_NAME_LENGTH
        ));
    }
    let is_race = trimmed
        .get(..super::RACE_COMPARISON_PREFIX.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(super::RACE_COMPARISON_PREFIX));
    if is_race {
        return Some("Names starting with [Race] are used by LiveSplit's race features.".into());
    }
    if RESERVED_NAMES
        .iter()
        .any(|r| r.eq_ignore_ascii_case(trimmed))
    {
        return Some(format!(
            "'{trimmed}' is one of LiveSplit's own comparisons. Pick another name."
        ));
    }
    if taken.into_iter().any(|t| same_name(t, trimmed)) {
        return Some(format!("There's already a comparison called '{trimmed}'."));
    }
    None
}

/// Whether two comparison names are the same. Like the desktop LiveSplit, this
/// ignores surrounding spaces and case, because two comparisons that only
/// differ in case can't be told apart in the list of comparisons.
pub fn same_name(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    a.eq_ignore_ascii_case(b) || a.to_lowercase() == b.to_lowercase()
}

/// All the custom comparisons of a run. Names are unique.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Library {
    programs: Vec<Program>,
}

impl Library {
    /// Creates a library from programs. Programs whose name is already taken
    /// by an earlier one (ignoring case) are left out.
    pub fn new(programs: impl IntoIterator<Item = Program>) -> Self {
        let mut seen = BTreeSet::new();
        let programs = programs
            .into_iter()
            .filter(|p| seen.insert(p.name.trim().to_lowercase()))
            .collect();
        Self { programs }
    }

    /// The programs, in order.
    pub fn programs(&self) -> &[Program] {
        &self.programs
    }

    /// The program with the given name, if any.
    pub fn get(&self, name: &str) -> Option<&Program> {
        self.programs.iter().find(|p| p.name == name)
    }

    /// Creates one comparison generator per program. Add them to a run's
    /// comparison generators to make the comparisons available.
    pub fn generators(self) -> Vec<Box<dyn ComparisonGenerator>> {
        let library = Arc::new(self);
        library
            .programs
            .iter()
            .map(|p| {
                Box::new(CustomComparison {
                    name: p.name.clone(),
                    library: library.clone(),
                }) as Box<dyn ComparisonGenerator>
            })
            .collect()
    }

    /// Replaces the custom comparisons installed on the run with the ones in
    /// this library and calculates them. A program named like a comparison
    /// the run already has, like a comparison stored in the splits, isn't
    /// installed, just like in the desktop LiveSplit. The names of the
    /// programs left out are returned. Other programs can still use them.
    pub fn install(self, run: &mut Run) -> Vec<String> {
        run.comparison_generators_mut()
            .retain(|g| g.as_custom().is_none());

        let taken: Vec<String> = run.comparisons().map(String::from).collect();
        let mut left_out = Vec::new();
        for generator in self.generators() {
            let name = generator.name();
            if check_name(name, taken.iter().map(String::as_str)).is_some() {
                left_out.push(name.to_string());
            } else {
                run.comparison_generators_mut().push(generator);
            }
        }

        run.regenerate_comparisons();
        left_out
    }

    /// Names of comparisons the programs use that aren't part of the library
    /// themselves, like the Personal Best or Best Segments.
    fn external_references(&self) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        for program in &self.programs {
            collect_references(&program.root, &mut names);
        }
        names.retain(|n| self.get(n).is_none());
        names
    }
}

fn collect_references(node: &ast::Times, names: &mut BTreeSet<String>) {
    use ast::Times::*;
    match node {
        Comparison { name } => {
            names.insert(name.clone());
        }
        Attempt { .. }
        | BestRecentRun { .. }
        | Aggregate { .. }
        | Percentile { .. }
        | WeightedAverage { .. } => {}
        AddTime { source, .. }
        | Scale { source, .. }
        | StretchToTotal { source, .. }
        | Round { source, .. } => collect_references(source, names),
        Blend { from, to, .. } => {
            collect_references(from, names);
            collect_references(to, names);
        }
        Pick { first, second, .. } => {
            collect_references(first, names);
            collect_references(second, names);
        }
        Clamp { source, low, high } => {
            collect_references(source, names);
            collect_references(low, names);
            collect_references(high, names);
        }
        Fallback { primary, fallback } => {
            collect_references(primary, names);
            collect_references(fallback, names);
        }
        SegmentRange { cases, otherwise } => {
            for case in cases {
                collect_references(&case.times, names);
            }
            collect_references(otherwise, names);
        }
    }
}

/// The comparison generator for one custom comparison.
#[derive(Clone, Debug)]
pub struct CustomComparison {
    name: String,
    library: Arc<Library>,
}

impl CustomComparison {
    /// The library this comparison belongs to. It holds all the custom
    /// comparisons installed together with this one.
    pub fn library(&self) -> &Library {
        &self.library
    }
}

impl ComparisonGenerator for CustomComparison {
    fn name(&self) -> &str {
        &self.name
    }

    fn as_custom(&self) -> Option<&CustomComparison> {
        Some(self)
    }

    fn generate(&mut self, segments: &mut [Segment], attempts: &[Attempt]) {
        let now = AtomicDateTime::now().time;
        let evaluator = Evaluator::new(self.library.programs());
        let references = self.library.external_references();
        let mut built_ins = BuiltIns::default();

        for method in [TimingMethod::RealTime, TimingMethod::GameTime] {
            let run = run_data(segments, attempts, method, now, &references, &mut built_ins);
            let splits = match evaluator.evaluate(&self.name, &run) {
                Some(result) => result.split_times,
                None => vec![None; segments.len()],
            };
            for (segment, split) in segments.iter_mut().zip(splits) {
                segment.comparison_mut(&self.name)[method] = split.map(from_nanos);
            }
        }
    }
}

/// Evaluates a custom comparison for the editor's preview: like the
/// generator, but returns the segment times, the origin of each time and any
/// error instead of storing the split times in the run. `library` should
/// contain the program being edited (under `name`) along with the run's other
/// custom comparisons. Unlike the generator, a comparison that doesn't exist
/// in the run is reported as an error, so the editor can point at it.
pub fn preview(
    library: &Library,
    name: &str,
    segments: &[Segment],
    attempts: &[Attempt],
    comparison_names: &[String],
    method: TimingMethod,
) -> Option<ComparisonResult> {
    let mut references = library.external_references();
    references.retain(|r| {
        comparison_names.iter().any(|c| c == r) || RESERVED_NAMES.contains(&r.as_str())
    });
    let run = run_data(
        segments,
        attempts,
        method,
        AtomicDateTime::now().time,
        &references,
        &mut BuiltIns::default(),
    );
    Evaluator::new(library.programs()).evaluate_with_origins(name, &run)
}

/// The built-in generated comparisons, calculated on a copy of the segments
/// the first time a program needs one. Shared by both timing methods.
#[derive(Default)]
struct BuiltIns {
    segments: Option<Vec<Segment>>,
}

impl BuiltIns {
    fn get(
        &mut self,
        name: &str,
        segments: &[Segment],
        attempts: &[Attempt],
    ) -> Option<&[Segment]> {
        let generator = default_generators()
            .into_iter()
            .find(|g| g.name() == name)?;
        let copy = self.segments.get_or_insert_with(|| segments.to_vec());
        let mut generator = generator;
        generator.generate(copy, attempts);
        Some(copy)
    }
}

fn to_nanos(time: TimeSpan) -> Nanos {
    let duration = time.to_duration();
    duration
        .whole_nanoseconds()
        .clamp(i128::from(Nanos::MIN), i128::from(Nanos::MAX)) as Nanos
}

fn from_nanos(nanos: Nanos) -> TimeSpan {
    TimeSpan::from(crate::platform::Duration::nanoseconds(nanos))
}

/// Takes the snapshot of the run the evaluator works on.
fn run_data(
    segments: &[Segment],
    attempts: &[Attempt],
    method: TimingMethod,
    now: DateTime,
    references: &BTreeSet<String>,
    built_ins: &mut BuiltIns,
) -> RunData {
    let segment_data = segments
        .iter()
        .map(|segment| SegmentData {
            name: segment.name().to_string(),
            // Only the runner's own attempts. Indices below 1 hold times that
            // were made up when segments were added or the history imported.
            history: segment
                .segment_history()
                .iter_actual_runs()
                .map(|&(index, time)| (index, time[method].map(to_nanos)))
                .collect(),
        })
        .collect();

    let attempt_data = attempts
        .iter()
        .map(|attempt| {
            let when = attempt.ended().or(attempt.started()).map(|t| t.time);
            let data = AttemptData {
                when,
                final_time: attempt.time()[method].map(to_nanos),
            };
            (attempt.index(), data)
        })
        .collect();

    let mut comparisons = BTreeMap::new();
    for name in references {
        let splits: Vec<Option<Nanos>> = if name == personal_best::NAME {
            segments
                .iter()
                .map(|s| s.personal_best_split_time()[method].map(to_nanos))
                .collect()
        } else if let Some(generated) = built_ins.get(name, segments, attempts) {
            generated
                .iter()
                .map(|s| s.comparison(name)[method].map(to_nanos))
                .collect()
        } else {
            // Any other comparison stored in the run, like a World Record
            // the runner entered. One that doesn't exist reads as empty.
            segments
                .iter()
                .map(|s| s.comparison(name)[method].map(to_nanos))
                .collect()
        };
        comparisons.insert(name.clone(), splits);
    }

    RunData {
        segments: segment_data,
        attempts: attempt_data,
        comparisons,
        now,
    }
}
