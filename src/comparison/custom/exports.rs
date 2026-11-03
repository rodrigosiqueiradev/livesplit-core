//! What the Custom Comparisons plugin does when the host calls it.
//!
//! The plugin itself (the crate `livesplit-custom-comparisons-plugin`) only
//! turns these functions into WebAssembly exports. Keeping them here means the
//! plugin's behaviour is tested together with the rest of custom comparisons,
//! without having to build a WebAssembly module first.

use super::{
    Evaluator, Library, format,
    run_data::{AttemptData, RunData, SegmentData},
    settings,
};
use crate::{
    comparison::plugin::abi::{
        DescribeInput, DescribeOutput, GenerateInput, GenerateOutput, Snapshot, SplitTimes,
    },
    platform::{DateTime, prelude::*},
};

/// Reads the settings into a library of programs. Comparisons that can't be
/// read are left out: the editor shows what is wrong with them.
pub fn library(settings: &str) -> Result<Library, String> {
    let texts = settings::read(settings)?;
    Ok(Library::new(
        texts.iter().filter_map(|text| format::read(text).ok()),
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

/// A generate output that only carries an error.
pub fn failed(error: String) -> GenerateOutput {
    GenerateOutput {
        error: Some(error),
        ..Default::default()
    }
}

fn date(millis: i64) -> Option<DateTime> {
    DateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000).ok()
}

fn pick(times: &SplitTimes, real_time: bool) -> Vec<Option<i64>> {
    if real_time {
        times.real.clone()
    } else {
        times.game.clone()
    }
}

/// Turns the snapshot a plugin gets into what the evaluator works on, for one
/// timing method.
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
        now: date(run.now).unwrap_or(DateTime::UNIX_EPOCH),
    }
}
