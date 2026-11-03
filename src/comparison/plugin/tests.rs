use super::snapshot;
use crate::{Run, Segment, Time, TimeSpan, comparison::best_segments, platform::DateTime};

fn run() -> Run {
    let mut run = Run::new();
    for name in ["A", "B"] {
        run.push_segment(Segment::new(name));
    }
    run.comparison_generators_mut().clear();
    for (index, segment) in run.segments_mut().iter_mut().enumerate() {
        let split = TimeSpan::from_seconds(10.0 * (index + 1) as f64);
        segment.set_personal_best_split_time(Time::new().with_real_time(Some(split)));
        segment.segment_history_mut().insert(
            1,
            Time::new().with_real_time(Some(TimeSpan::from_seconds(10.0))),
        );
        segment.comparison_mut("Mine").real_time = Some(split);
    }
    run
}

#[test]
fn built_ins_are_included_even_when_turned_off() {
    let run = run();
    let snapshot = snapshot(
        run.segments(),
        run.attempt_history(),
        DateTime::UNIX_EPOCH,
        &[],
    );
    let best = &snapshot.comparisons[best_segments::NAME];
    assert_eq!(best.real, [Some(10_000_000_000), Some(20_000_000_000)]);
    assert_eq!(
        snapshot.comparisons["Personal Best"].real,
        [Some(10_000_000_000), Some(20_000_000_000)]
    );
    assert_eq!(snapshot.segments[1].history[0].real, Some(10_000_000_000));
}

#[test]
fn the_plugins_own_comparisons_are_left_out() {
    let run = run();
    let own = ["Mine".to_string()];
    let snapshot = snapshot(
        run.segments(),
        run.attempt_history(),
        DateTime::UNIX_EPOCH,
        &own,
    );
    assert!(!snapshot.comparisons.contains_key("Mine"));
    let all = super::snapshot(
        run.segments(),
        run.attempt_history(),
        DateTime::UNIX_EPOCH,
        &[],
    );
    assert!(all.comparisons.contains_key("Mine"));
}
