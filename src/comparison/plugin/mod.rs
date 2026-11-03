//! Comparison plugins: comparisons calculated by WebAssembly modules.
//!
//! A comparison plugin is a sandboxed WebAssembly module that turns the run's
//! segment history into one or more comparisons. It is a pure function: it
//! imports nothing, gets a [`Snapshot`](abi::Snapshot) of the run and returns
//! split times. The [`abi`] module describes the exact interface.
//!
//! # How it fits into livesplit-core
//!
//! Each comparison a plugin provides becomes one [`PluginComparison`]
//! comparison generator, so it is recalculated between attempts like the
//! built-in comparisons, and everything that works with comparisons works
//! with it.
//!
//! Where the plugin and its settings (for example the comparisons a runner
//! built in an editor) are kept is up to the application, as is the
//! [`Engine`] that runs the module: livesplit-core comes with one based on
//! wasmtime ([`WasmtimeEngine`], feature `comparison-plugins`), while a
//! browser can use its own WebAssembly engine. Installing a plugin on a run
//! doesn't change what is saved in the splits, just like the built-in
//! comparison generators.
//!
//! A plugin that fails (it traps, runs out of fuel, returns garbage or an
//! error) leaves its comparison empty. It can never affect the timer.

pub mod abi;
#[cfg(feature = "comparison-plugins")]
mod wasmtime_engine;
#[cfg(all(
    feature = "web-comparison-plugins",
    target_family = "wasm",
    not(any(target_os = "wasi", target_os = "emscripten"))
))]
mod web_engine;

#[cfg(feature = "comparison-plugins")]
pub use self::wasmtime_engine::WasmtimeEngine;
#[cfg(all(
    feature = "web-comparison-plugins",
    target_family = "wasm",
    not(any(target_os = "wasi", target_os = "emscripten"))
))]
pub use self::web_engine::WebEngine;

use self::abi::{
    DescribeInput, DescribeOutput, GenerateInput, GenerateOutput, HistoryEntry, Snapshot,
    SnapshotAttempt, SnapshotSegment, SplitTimes,
};
use super::{ComparisonGenerator, custom::check_name, default_generators, personal_best};
use crate::{
    AtomicDateTime, Attempt, Run, Segment, TimeSpan,
    platform::{Arc, DateTime, prelude::*},
};
use core::fmt;

/// Something went wrong while loading or running a plugin. The message is
/// meant for the runner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl core::error::Error for Error {}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self(message.into())
    }
}

/// Runs the functions a plugin exports. The input and output are the JSON
/// documents described in [`abi`]. An engine is free to hold on to state
/// between calls, so it has to synchronize itself.
pub trait Engine: fmt::Debug + Send + Sync {
    /// Calls `lscp_describe`.
    fn describe(&self, input: &[u8]) -> Result<Vec<u8>, Error>;
    /// Calls `lscp_generate`.
    fn generate(&self, input: &[u8]) -> Result<Vec<u8>, Error>;
}

/// Creates an engine for the module with the best engine this build of
/// livesplit-core has: the browser's own engine on the web, otherwise
/// wasmtime. Fails if neither is enabled.
pub fn engine_for(module: &[u8]) -> Result<Box<dyn Engine>, Error> {
    cfg_select! {
        all(
            feature = "web-comparison-plugins",
            target_family = "wasm",
            not(any(target_os = "wasi", target_os = "emscripten"))
        ) => {
            Ok(Box::new(WebEngine::new(module)?))
        }
        feature = "comparison-plugins" => {
            Ok(Box::new(WasmtimeEngine::new(module)?))
        }
        _ => {
            let _ = module;
            Err("This version of LiveSplit can't run comparison plugins.".into())
        }
    }
}

/// A loaded plugin with its settings.
#[derive(Clone, Debug)]
pub struct Plugin {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    id: String,
    settings: String,
    engine: Box<dyn Engine>,
    comparisons: Vec<String>,
}

impl Plugin {
    /// Loads a plugin and asks it which comparisons it provides with these
    /// settings.
    pub fn new(
        id: impl Into<String>,
        settings: impl Into<String>,
        engine: Box<dyn Engine>,
    ) -> Result<Self, Error> {
        let settings = settings.into();
        let input = to_json(&DescribeInput {
            settings: settings.clone(),
        })?;
        let output: DescribeOutput = from_json(&engine.describe(&input)?)?;
        if let Some(error) = output.error {
            return Err(Error(error));
        }
        Ok(Self {
            shared: Arc::new(Shared {
                id: id.into(),
                settings,
                engine,
                comparisons: output.comparisons,
            }),
        })
    }

    /// The plugin's id.
    pub fn id(&self) -> &str {
        &self.shared.id
    }

    /// The plugin's settings.
    pub fn settings(&self) -> &str {
        &self.shared.settings
    }

    /// The comparisons the plugin provides.
    pub fn comparisons(&self) -> &[String] {
        &self.shared.comparisons
    }

    /// Calculates one of the plugin's comparisons without storing it
    /// anywhere, for example for an editor's preview.
    pub fn generate(
        &self,
        comparison: &str,
        segments: &[Segment],
        attempts: &[Attempt],
    ) -> Result<GenerateOutput, Error> {
        let run = snapshot(
            segments,
            attempts,
            AtomicDateTime::now().time,
            &self.shared.comparisons,
        );
        let input = to_json(&GenerateInput {
            settings: self.shared.settings.clone(),
            comparison: comparison.into(),
            run,
        })?;
        let output: GenerateOutput = from_json(&self.shared.engine.generate(&input)?)?;
        if let Some(error) = output.error {
            return Err(Error(error));
        }
        Ok(output)
    }

    /// Creates one comparison generator per comparison the plugin provides.
    pub fn generators(&self) -> Vec<Box<dyn ComparisonGenerator>> {
        self.shared
            .comparisons
            .iter()
            .map(|name| {
                Box::new(PluginComparison {
                    name: name.clone(),
                    plugin: self.clone(),
                }) as Box<dyn ComparisonGenerator>
            })
            .collect()
    }

    /// Installs the plugin on the run, replacing the comparisons of an
    /// earlier version of the same plugin, and calculates its comparisons. A comparison named like one the run
    /// already has isn't installed. Their names are returned.
    pub fn install(&self, run: &mut Run) -> Vec<String> {
        run.comparison_generators_mut()
            .retain(|g| g.as_plugin().is_none_or(|p| p.plugin().id() != self.id()));

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
}

/// The comparison generator for one comparison of a plugin.
#[derive(Clone, Debug)]
pub struct PluginComparison {
    name: String,
    plugin: Plugin,
}

impl PluginComparison {
    /// The plugin that provides this comparison.
    pub const fn plugin(&self) -> &Plugin {
        &self.plugin
    }
}

impl ComparisonGenerator for PluginComparison {
    fn name(&self) -> &str {
        &self.name
    }

    fn as_plugin(&self) -> Option<&PluginComparison> {
        Some(self)
    }

    fn generate(&mut self, segments: &mut [Segment], attempts: &[Attempt]) {
        let output = self
            .plugin
            .generate(&self.name, segments, attempts)
            .unwrap_or_default();
        for (index, segment) in segments.iter_mut().enumerate() {
            let time = segment.comparison_mut(&self.name);
            time.real_time = split(&output.real, index);
            time.game_time = split(&output.game, index);
        }
    }
}

fn split(splits: &[Option<i64>], index: usize) -> Option<TimeSpan> {
    let nanos = splits.get(index).copied().flatten()?;
    Some(TimeSpan::from(crate::platform::Duration::nanoseconds(
        nanos,
    )))
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    serde_json::to_vec(value).map_err(|_| Error::from("The plugin's input couldn't be written."))
}

fn from_json<'a, T: serde::Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, Error> {
    serde_json::from_slice(bytes)
        .map_err(|e| Error(format!("The plugin returned something unreadable: {e}")))
}

fn nanos(time: TimeSpan) -> i64 {
    time.to_duration()
        .whole_nanoseconds()
        .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

const fn millis(time: DateTime) -> i64 {
    (time.unix_timestamp_nanos() / 1_000_000) as i64
}

fn split_times(segments: &[Segment], name: &str) -> SplitTimes {
    SplitTimes {
        real: segments
            .iter()
            .map(|s| s.comparison(name).real_time.map(nanos))
            .collect(),
        game: segments
            .iter()
            .map(|s| s.comparison(name).game_time.map(nanos))
            .collect(),
    }
}

/// Takes the snapshot of the run that a plugin gets. The comparisons named in
/// `own` (the plugin's own) are left out.
pub fn snapshot(
    segments: &[Segment],
    attempts: &[Attempt],
    now: DateTime,
    own: &[String],
) -> Snapshot {
    let snapshot_segments = segments
        .iter()
        .map(|segment| SnapshotSegment {
            name: segment.name().into(),
            history: segment
                .segment_history()
                .iter()
                .map(|&(attempt, time)| HistoryEntry {
                    attempt,
                    real: time.real_time.map(nanos),
                    game: time.game_time.map(nanos),
                })
                .collect(),
        })
        .collect();

    let snapshot_attempts = attempts
        .iter()
        .map(|attempt| SnapshotAttempt {
            id: attempt.index(),
            started: attempt.started().map(|t| millis(t.time)),
            ended: attempt.ended().map(|t| millis(t.time)),
            real: attempt.time().real_time.map(nanos),
            game: attempt.time().game_time.map(nanos),
        })
        .collect();

    let mut comparisons = alloc::collections::BTreeMap::new();
    let is_own = |name: &str| own.iter().any(|o| o == name);

    // Every comparison stored in the segments: the Personal Best, ones the
    // runner entered, and whatever other generators calculated.
    for segment in segments {
        for (name, _) in segment.comparisons().iter() {
            if !is_own(name) && !comparisons.contains_key(&**name) {
                comparisons.insert(String::from(&**name), split_times(segments, name));
            }
        }
    }
    comparisons.insert(
        personal_best::NAME.into(),
        split_times(segments, personal_best::NAME),
    );

    // The built-in generated comparisons, calculated on a copy, so a plugin
    // can rely on them even if the runner turned them off.
    let mut copy = segments.to_vec();
    for mut generator in default_generators() {
        generator.generate(&mut copy, attempts);
        let name = generator.name();
        if !is_own(name) {
            comparisons.insert(name.into(), split_times(&copy, name));
        }
    }

    Snapshot {
        now: millis(now),
        segments: snapshot_segments,
        attempts: snapshot_attempts,
        comparisons,
    }
}

#[cfg(test)]
mod tests;
