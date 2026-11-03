//! Evaluates custom comparison programs against a run.
//!
//! The rules:
//!
//! - Everything is per segment. An empty segment stays empty unless a
//!   fallback fills it, and once a segment is empty every later split time is
//!   empty too. Nothing is ever made up.
//! - The segment history follows LiveSplit's own rules: a skipped segment is
//!   left out, and so is the segment right after it, because its recorded time
//!   covers both segments.
//! - Programs may use each other through the comparison block. Those are
//!   evaluated here (not read back from the run), so the order the generators
//!   run in doesn't matter, and a loop is reported as an error instead of
//!   silently producing stale times.

use super::{
    ast::{AggregateKind, Nanos, PickKind, Program, Samples, Times},
    run_data::RunData,
};
use crate::platform::prelude::*;
use alloc::collections::{BTreeMap, BTreeSet};

/// The outcome of evaluating one program against one run, for one timing
/// method.
#[derive(Clone, Debug, PartialEq)]
pub struct ComparisonResult {
    /// The time of each segment. Empty segments are `None`.
    pub segment_times: Vec<Option<Nanos>>,
    /// The cumulative split times, which is what a comparison stores. Once a
    /// segment is empty, every later split is empty too.
    pub split_times: Vec<Option<Nanos>>,
    /// `None` on success. On failure every time is empty and this says why,
    /// in words meant for the runner.
    pub error: Option<String>,
    /// Only filled in by [`Evaluator::evaluate_with_origins`]: for each
    /// segment, the path from the program's root to the block that produced
    /// its time, through the choices made by fallback, segment range,
    /// faster/slower and keep-between blocks. The steps use the file format's
    /// keys, like `primary.cases.0.times` or `fallback`. An empty path means
    /// the root block itself.
    pub origins: Option<Vec<String>>,
}

impl ComparisonResult {
    /// Whether the evaluation succeeded.
    pub const fn succeeded(&self) -> bool {
        self.error.is_none()
    }
}

/// A problem with a program itself, like a comparison that doesn't exist or
/// a loop between comparisons.
type ProgramError = String;

/// One time (or nothing) for each segment.
type SegmentTimes = Vec<Option<Nanos>>;

/// One recorded segment time and the attempt it came from.
#[derive(Copy, Clone, Debug)]
struct Sample {
    attempt: i32,
    time: Nanos,
}

/// Evaluates a set of programs. Programs may refer to each other by name.
#[derive(Clone, Debug)]
pub struct Evaluator<'a> {
    programs: BTreeMap<&'a str, &'a Program>,
}

impl<'a> Evaluator<'a> {
    /// Creates an evaluator for the given programs. If two programs share a
    /// name, the first one wins; the library makes sure that never happens.
    pub fn new(programs: impl IntoIterator<Item = &'a Program>) -> Self {
        let mut map = BTreeMap::new();
        for program in programs {
            map.entry(program.name.as_str()).or_insert(program);
        }
        Self { programs: map }
    }

    /// Whether a program with this name is part of the evaluator.
    pub fn contains(&self, name: &str) -> bool {
        self.programs.contains_key(name)
    }

    /// Evaluates the program with the given name. Returns `None` if there's
    /// no such program.
    pub fn evaluate(&self, name: &str, run: &RunData) -> Option<ComparisonResult> {
        self.programs.get(name)?;
        Some(Session::new(self, run).evaluate(name))
    }

    /// Like [`evaluate`](Self::evaluate), but also records where each
    /// segment's time came from, for the editor's preview.
    pub fn evaluate_with_origins(&self, name: &str, run: &RunData) -> Option<ComparisonResult> {
        let program = self.programs.get(name)?;
        let mut session = Session::new(self, run);
        let mut result = session.evaluate(name);
        if result.succeeded() {
            match session.trace(&program.root) {
                Ok((_, paths)) => result.origins = Some(paths),
                Err(error) => return Some(failed(run, error)),
            }
        }
        Some(result)
    }
}

fn failed(run: &RunData, error: ProgramError) -> ComparisonResult {
    let empty = vec![None; run.segment_count()];
    ComparisonResult {
        segment_times: empty.clone(),
        split_times: empty,
        error: Some(error),
        origins: None,
    }
}

/// One evaluation pass over one run. Caches results, so a comparison used by
/// several others is only evaluated once.
struct Session<'e, 'a, 'r> {
    owner: &'e Evaluator<'a>,
    run: &'r RunData,
    finished: BTreeMap<String, SegmentTimes>,
    in_progress: Vec<String>,
    history: Option<Vec<Vec<Sample>>>,
}

impl<'e, 'a, 'r> Session<'e, 'a, 'r> {
    const fn new(owner: &'e Evaluator<'a>, run: &'r RunData) -> Self {
        Self {
            owner,
            run,
            finished: BTreeMap::new(),
            in_progress: Vec::new(),
            history: None,
        }
    }

    fn evaluate(&mut self, name: &str) -> ComparisonResult {
        match self.evaluate_program(name) {
            Ok(segments) => ComparisonResult {
                split_times: to_splits(&segments),
                segment_times: segments,
                error: None,
                origins: None,
            },
            Err(error) => failed(self.run, error),
        }
    }

    fn evaluate_program(&mut self, name: &str) -> Result<SegmentTimes, ProgramError> {
        if let Some(cached) = self.finished.get(name) {
            return Ok(cached.clone());
        }

        if let Some(start) = self.in_progress.iter().position(|n| n == name) {
            let mut chain: Vec<&str> = self.in_progress[start..]
                .iter()
                .map(String::as_str)
                .collect();
            chain.push(name);
            return Err(format!(
                "These comparisons depend on each other in a loop: {}",
                chain.join(" → ")
            ));
        }

        let program = self.owner.programs[name];
        self.in_progress.push(name.to_string());
        let result = self.times(&program.root);
        self.in_progress.pop();
        let result = result?;
        self.finished.insert(name.to_string(), result.clone());
        Ok(result)
    }

    // ---------------------------------------------------------------- Origins

    /// The values and, per segment, the path to the block that produced each
    /// value. Follows the branch actually taken by fallback, segment range,
    /// faster/slower and keep-between blocks, and looks through blocks that
    /// only adjust a single input (add, scale, stretch, round), since the
    /// runner wants to know which *rule* a time came from.
    fn trace(&mut self, node: &Times) -> Result<(SegmentTimes, Vec<String>), ProgramError> {
        Ok(match node {
            Times::Fallback { primary, fallback } => {
                let (p, p_paths) = self.trace(primary)?;
                let (f, f_paths) = self.trace(fallback)?;
                let mut values = Vec::with_capacity(p.len());
                let mut paths = Vec::with_capacity(p.len());
                for i in 0..p.len() {
                    if p[i].is_some() {
                        values.push(p[i]);
                        paths.push(join("primary", &p_paths[i]));
                    } else {
                        values.push(f[i]);
                        paths.push(join("fallback", &f_paths[i]));
                    }
                }
                (values, paths)
            }
            Times::SegmentRange { cases, otherwise } => {
                let arms = cases
                    .iter()
                    .map(|c| self.trace(&c.times))
                    .collect::<Result<Vec<_>, _>>()?;
                let (o, o_paths) = self.trace(otherwise)?;
                let mut values = Vec::with_capacity(o.len());
                let mut paths = Vec::with_capacity(o.len());
                for i in 0..o.len() {
                    match cases.iter().position(|c| c.contains(i as u32 + 1)) {
                        Some(k) => {
                            values.push(arms[k].0[i]);
                            paths.push(join(&format!("cases.{k}.times"), &arms[k].1[i]));
                        }
                        None => {
                            values.push(o[i]);
                            paths.push(join("otherwise", &o_paths[i]));
                        }
                    }
                }
                (values, paths)
            }
            Times::Pick {
                kind,
                first,
                second,
            } => {
                let (a, a_paths) = self.trace(first)?;
                let (b, b_paths) = self.trace(second)?;
                let values = self.times(node)?;
                let paths = (0..values.len())
                    .map(|i| {
                        if takes_first(*kind, a[i], b[i]) {
                            join("first", &a_paths[i])
                        } else {
                            join("second", &b_paths[i])
                        }
                    })
                    .collect();
                (values, paths)
            }
            Times::Clamp { source, low, high } => {
                // Points at whichever input the time ended up being equal to:
                // one of the limits, or the source itself.
                let (s, s_paths) = self.trace(source)?;
                let (l, l_paths) = self.trace(low)?;
                let (_, h_paths) = self.trace(high)?;
                let values = self.times(node)?;
                let paths = (0..values.len())
                    .map(|i| {
                        if values[i].is_none() || values[i] == s[i] {
                            join("source", &s_paths[i])
                        } else if values[i] == l[i] {
                            join("low", &l_paths[i])
                        } else {
                            join("high", &h_paths[i])
                        }
                    })
                    .collect();
                (values, paths)
            }
            Times::AddTime { source, .. }
            | Times::Scale { source, .. }
            | Times::StretchToTotal { source, .. }
            | Times::Round { source, .. } => {
                let (_, paths) = self.trace(source)?;
                (
                    self.times(node)?,
                    paths.iter().map(|p| join("source", p)).collect(),
                )
            }
            _ => {
                let values = self.times(node)?;
                let paths = vec![String::new(); values.len()];
                (values, paths)
            }
        })
    }

    // ---------------------------------------------------------------- Times

    fn times(&mut self, node: &Times) -> Result<SegmentTimes, ProgramError> {
        Ok(match node {
            Times::Comparison { name } => self.comparison_segments(name)?,
            Times::Attempt { number } => self.attempt_segments(*number)?,
            Times::BestRecentRun { days } => {
                let cutoff = self.run.now - time::Duration::days(i64::from(*days));
                let best = self
                    .run
                    .attempts
                    .iter()
                    .filter(|(_, a)| a.when.is_some_and(|w| w >= cutoff))
                    .filter_map(|(&index, a)| Some((a.final_time?, index)))
                    .min();
                match best {
                    Some((_, index)) => self.attempt_segments(index)?,
                    None => vec![None; self.run.segment_count()],
                }
            }
            Times::Aggregate { kind, source } => self
                .samples(source)?
                .iter()
                .map(|s| aggregate(*kind, s))
                .collect(),
            Times::Percentile { percent, source } => self
                .samples(source)?
                .iter()
                .map(|s| percentile(s, *percent))
                .collect(),
            Times::WeightedAverage { half_life, source } => self
                .samples(source)?
                .iter()
                .map(|s| weighted_average(s, *half_life))
                .collect(),
            Times::AddTime { amount, source } => self
                .times(source)?
                .into_iter()
                .map(|t| t.map(|t| (t + amount).max(0)))
                .collect(),
            Times::Scale { percent, source } => self
                .times(source)?
                .into_iter()
                .map(|t| t.map(|t| round_to_nanos(t as f64 * percent / 100.0)))
                .collect(),
            Times::StretchToTotal { total, source } => {
                stretch_to_total(&self.times(source)?, *total)
            }
            Times::Blend { percent, from, to } => {
                let a = self.times(from)?;
                let b = self.times(to)?;
                let share = percent / 100.0;
                a.iter()
                    .zip(&b)
                    .map(|(&a, &b)| Some(a? + round_to_nanos((b? - a?) as f64 * share)))
                    .collect()
            }
            Times::Pick {
                kind,
                first,
                second,
            } => {
                let a = self.times(first)?;
                let b = self.times(second)?;
                a.iter()
                    .zip(&b)
                    .map(|(&a, &b)| if takes_first(*kind, a, b) { a } else { b })
                    .collect()
            }
            Times::Clamp { source, low, high } => {
                let s = self.times(source)?;
                let l = self.times(low)?;
                let h = self.times(high)?;
                (0..s.len())
                    .map(|i| {
                        let mut v = s[i]?;
                        // "Never slower than" first, then "never faster than",
                        // so the latter wins when the limits cross. A target
                        // below gold is the bigger problem for a runner.
                        if let Some(h) = h[i] {
                            v = v.min(h);
                        }
                        if let Some(l) = l[i] {
                            v = v.max(l);
                        }
                        Some(v)
                    })
                    .collect()
            }
            Times::Round { step, source } => self
                .times(source)?
                .into_iter()
                .map(|t| t.map(|t| (t + step / 2).div_euclid(*step) * step))
                .collect(),
            Times::Fallback { primary, fallback } => {
                let p = self.times(primary)?;
                let f = self.times(fallback)?;
                p.iter().zip(&f).map(|(&p, &f)| p.or(f)).collect()
            }
            Times::SegmentRange { cases, otherwise } => {
                // Evaluate each branch once, then pick per segment.
                let arms = cases
                    .iter()
                    .map(|c| self.times(&c.times))
                    .collect::<Result<Vec<_>, _>>()?;
                let o = self.times(otherwise)?;
                (0..o.len())
                    .map(
                        |i| match cases.iter().position(|c| c.contains(i as u32 + 1)) {
                            Some(k) => arms[k][i],
                            None => o[i],
                        },
                    )
                    .collect()
            }
        })
    }

    /// An existing comparison as segment times. Other custom programs are
    /// evaluated here, with loop detection, rather than read back from the
    /// run, so it doesn't matter in which order the generators run.
    fn comparison_segments(&mut self, name: &str) -> Result<SegmentTimes, ProgramError> {
        if self.owner.contains(name) {
            return self.evaluate_program(name);
        }

        let splits = self
            .run
            .comparisons
            .get(name)
            .ok_or_else(|| format!("There is no comparison named '{name}'."))?;

        let count = self.run.segment_count();
        Ok((0..count)
            .map(|i| {
                let current = splits.get(i).copied().flatten()?;
                let previous = if i == 0 {
                    0
                } else {
                    splits.get(i - 1).copied().flatten()?
                };
                Some((current - previous).max(0))
            })
            .collect())
    }

    /// One attempt's segment times, with LiveSplit's skip rules applied.
    fn attempt_segments(&mut self, number: i32) -> Result<SegmentTimes, ProgramError> {
        let known = self.run.attempts.contains_key(&number)
            || self
                .run
                .segments
                .iter()
                .any(|s| s.history.contains_key(&number));
        if !known {
            return Err(format!("There's no attempt #{number} in these splits."));
        }

        Ok(self
            .segment_history()
            .iter()
            .map(|samples| samples.iter().find(|s| s.attempt == number).map(|s| s.time))
            .collect())
    }

    // ---------------------------------------------------------------- Samples

    fn samples(&mut self, node: &Samples) -> Result<Vec<Vec<Sample>>, ProgramError> {
        let run = self.run;
        Ok(match node {
            Samples::SegmentHistory => self.segment_history().to_vec(),
            Samples::LastN { count, source } => {
                let mut lists = self.samples(source)?;
                for list in &mut lists {
                    list.sort_by_key(|s| s.attempt);
                    let skip = list.len().saturating_sub(*count as usize);
                    list.drain(..skip);
                }
                lists
            }
            Samples::CompletedRunsOnly { source } => filter(self.samples(source)?, |s| {
                run.attempts.get(&s.attempt).is_some_and(|a| a.completed())
            }),
            Samples::RecentDays { days, source } => {
                let cutoff = run.now - time::Duration::days(i64::from(*days));
                filter(self.samples(source)?, |s| {
                    run.attempts
                        .get(&s.attempt)
                        .and_then(|a| a.when)
                        .is_some_and(|w| w >= cutoff)
                })
            }
            Samples::TrimOutliers { percent, source } => {
                let mut lists = self.samples(source)?;
                for list in &mut lists {
                    let drop = libm::floor(list.len() as f64 * percent / 100.0) as usize;
                    if drop == 0 {
                        continue;
                    }
                    // Decide which to drop by time, but keep the survivors in
                    // their original (attempt) order, so a "last N" after this
                    // still means the most recent ones.
                    let mut order: Vec<usize> = (0..list.len()).collect();
                    order.sort_by_key(|&i| (list[i].time, i));
                    let kept: BTreeSet<usize> =
                        order[drop..list.len() - drop].iter().copied().collect();
                    let mut index = 0;
                    list.retain(|_| {
                        let keep = kept.contains(&index);
                        index += 1;
                        keep
                    });
                }
                lists
            }
            Samples::FinishedUnder { limit, source } => filter(self.samples(source)?, |s| {
                run.attempts
                    .get(&s.attempt)
                    .and_then(|a| a.final_time)
                    .is_some_and(|t| t < *limit)
            }),
            Samples::AtLeast { count, source } => {
                let mut lists = self.samples(source)?;
                for list in &mut lists {
                    if list.len() < *count as usize {
                        list.clear();
                    }
                }
                lists
            }
            Samples::ReachedSegment { segment, source } => {
                let lists = self.samples(source)?;
                let reached = segment
                    .checked_sub(1)
                    .and_then(|i| run.segments.get(i as usize));
                match reached {
                    Some(reached) => filter(lists, |s| reached.history.contains_key(&s.attempt)),
                    // A segment past the end of the run: nobody reached it.
                    None => lists.into_iter().map(|_| Vec::new()).collect(),
                }
            }
        })
    }

    /// Builds the per-segment lists from the segment history, following
    /// LiveSplit's own rules: within an attempt, a skipped segment means the
    /// next recorded time covers two segments, so it's left out too; and an
    /// attempt stops contributing at the first segment it never reached.
    fn segment_history(&mut self) -> &[Vec<Sample>] {
        let run = self.run;
        self.history.get_or_insert_with(|| {
            let mut lists = vec![Vec::new(); run.segment_count()];
            let attempts: BTreeSet<i32> = run
                .segments
                .iter()
                .flat_map(|s| s.history.keys().copied())
                .collect();

            for attempt in attempts {
                let mut previous_was_skipped = false;
                for (i, segment) in run.segments.iter().enumerate() {
                    match segment.history.get(&attempt) {
                        None => break,
                        Some(None) => previous_was_skipped = true,
                        Some(Some(_)) if previous_was_skipped => previous_was_skipped = false,
                        Some(&Some(time)) => lists[i].push(Sample { attempt, time }),
                    }
                }
            }

            lists
        })
    }
}

// -------------------------------------------------------------------- Helpers

fn filter(mut lists: Vec<Vec<Sample>>, keep: impl Fn(&Sample) -> bool) -> Vec<Vec<Sample>> {
    for list in &mut lists {
        list.retain(&keep);
    }
    lists
}

fn join(step: &str, rest: &str) -> String {
    if rest.is_empty() {
        step.to_string()
    } else {
        format!("{step}.{rest}")
    }
}

/// The same rule as the faster/slower block: ties and an empty second side go
/// to the first.
const fn takes_first(kind: PickKind, a: Option<Nanos>, b: Option<Nanos>) -> bool {
    match (a, b) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(a), Some(b)) => match kind {
            PickKind::Faster => a <= b,
            PickKind::Slower => a >= b,
        },
    }
}

fn round_to_nanos(value: f64) -> Nanos {
    libm::round(value) as Nanos
}

fn to_splits(segments: &[Option<Nanos>]) -> SegmentTimes {
    let mut total = Some(0);
    segments
        .iter()
        .map(|&segment| {
            total = total.zip(segment).map(|(t, s)| t + s);
            total
        })
        .collect()
}

fn sorted_times(samples: &[Sample]) -> Vec<Nanos> {
    let mut times: Vec<Nanos> = samples.iter().map(|s| s.time).collect();
    times.sort_unstable();
    times
}

fn aggregate(kind: AggregateKind, samples: &[Sample]) -> Option<Nanos> {
    if samples.is_empty() {
        return None;
    }
    let times = sorted_times(samples);
    Some(match kind {
        AggregateKind::Min => times[0],
        AggregateKind::Max => times[times.len() - 1],
        AggregateKind::Average => {
            // Summing in i128 can't overflow, whatever the history holds.
            let sum: i128 = times.iter().map(|&t| i128::from(t)).sum();
            round_to_nanos(sum as f64 / times.len() as f64)
        }
        AggregateKind::Median => {
            let mid = times.len() / 2;
            if times.len() % 2 == 1 {
                times[mid]
            } else {
                (times[mid - 1] + times[mid]) / 2
            }
        }
    })
}

/// Linear interpolation between the closest ranks, so 50 is exactly the
/// median.
fn percentile(samples: &[Sample], percent: f64) -> Option<Nanos> {
    if samples.is_empty() {
        return None;
    }
    let times = sorted_times(samples);
    let rank = percent / 100.0 * (times.len() - 1) as f64;
    let lower = libm::floor(rank) as usize;
    let upper = libm::ceil(rank) as usize;
    let fraction = rank - lower as f64;
    Some(times[lower] + round_to_nanos((times[upper] - times[lower]) as f64 * fraction))
}

/// The newest time weighs 1, and the weight halves every `half_life` older
/// times.
fn weighted_average(samples: &[Sample], half_life: u32) -> Option<Nanos> {
    if samples.is_empty() {
        return None;
    }
    let mut newest_first: Vec<Sample> = samples.to_vec();
    newest_first.sort_by_key(|s| core::cmp::Reverse(s.attempt));
    let (mut total, mut weights) = (0.0, 0.0);
    for (age, sample) in newest_first.iter().enumerate() {
        let weight = libm::pow(0.5, age as f64 / f64::from(half_life));
        total += sample.time as f64 * weight;
        weights += weight;
    }
    Some(round_to_nanos(total / weights))
}

/// Scales every segment by one factor so the total is exactly `total`.
/// Rounding leftovers go to the last segment, so the sum is exact.
fn stretch_to_total(segments: &[Option<Nanos>], total: Nanos) -> SegmentTimes {
    let empty = vec![None; segments.len()];
    let Some(times) = segments.iter().copied().collect::<Option<Vec<Nanos>>>() else {
        return empty;
    };
    let sum: i128 = times.iter().map(|&t| i128::from(t)).sum();
    if times.is_empty() || sum <= 0 {
        return empty;
    }

    let factor = total as f64 / sum as f64;
    let mut assigned = 0;
    let mut result: SegmentTimes = times[..times.len() - 1]
        .iter()
        .map(|&t| {
            let scaled = round_to_nanos(t as f64 * factor);
            assigned += scaled;
            Some(scaled)
        })
        .collect();
    result.push(Some((total - assigned).max(0)));
    result
}
