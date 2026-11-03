//! Runs the Custom Comparisons plugin through wasmtime and checks that it
//! calculates exactly what the built-in custom comparisons engine does.

#![cfg(feature = "comparison-plugins")]

mod run_files;

use livesplit_core::{
    Run, TimingMethod,
    comparison::{
        custom::{Library, format},
        plugin::{Engine, Plugin, WasmtimeEngine},
    },
    run::parser::livesplit,
};
use std::{path::PathBuf, process::Command, sync::OnceLock};

const ID: &str = "livesplit.custom-comparisons";

/// Builds the plugin once for all the tests.
fn plugin_wasm() -> &'static [u8] {
    static WASM: OnceLock<Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target"));
        let status = Command::new(env!("CARGO"))
            .current_dir(&root)
            .args([
                "build",
                "-p",
                "livesplit-custom-comparisons-plugin",
                "--target",
                "wasm32-unknown-unknown",
                "--release",
            ])
            .status()
            .expect("cargo should run");
        assert!(status.success(), "the plugin should build");
        std::fs::read(
            target.join("wasm32-unknown-unknown/release/livesplit_custom_comparisons_plugin.wasm"),
        )
        .unwrap()
    })
}

fn engine() -> Box<dyn Engine> {
    Box::new(WasmtimeEngine::new(plugin_wasm()).unwrap())
}

const SAFE_PACE: &str = r#"{
  "format": "livesplit-custom-comparison", "version": 1, "name": "Safe Pace",
  "root": {
    "type": "fallback",
    "primary": {
      "type": "segmentRange",
      "cases": [{ "from": 1, "to": 3, "times": {
        "type": "addTime", "seconds": 0.5,
        "source": { "type": "comparison", "name": "Best Segments" } } }],
      "otherwise": { "type": "aggregate", "kind": "median",
        "source": { "type": "lastN", "count": 10,
          "source": { "type": "completedRunsOnly", "source": { "type": "segmentHistory" } } } }
    },
    "fallback": { "type": "comparison", "name": "Personal Best" }
  }
}"#;

const SAFER: &str = r#"{
  "format": "livesplit-custom-comparison", "version": 1, "name": "Safer",
  "root": { "type": "addTime", "seconds": 2,
    "source": { "type": "comparison", "name": "Safe Pace" } }
}"#;

const AVERAGE_OF_ALL: &str = r#"{
  "format": "livesplit-custom-comparison", "version": 1, "name": "Average Of All",
  "root": { "type": "aggregate", "kind": "average", "source": { "type": "segmentHistory" } }
}"#;

const BROKEN: &str = r#"{
  "format": "livesplit-custom-comparison", "version": 1, "name": "Broken",
  "root": { "type": "comparison", "name": "Does Not Exist" }
}"#;

const DOCUMENTS: [&str; 4] = [SAFE_PACE, SAFER, AVERAGE_OF_ALL, BROKEN];

fn settings() -> String {
    format!(r#"{{"comparisons":[{}]}}"#, DOCUMENTS.join(","))
}

fn celeste() -> Run {
    livesplit::parse(run_files::CELESTE).unwrap()
}

fn splits(run: &Run, name: &str) -> Vec<(Option<i64>, Option<i64>)> {
    let nanos =
        |t: Option<livesplit_core::TimeSpan>| t.map(|t| t.to_duration().whole_nanoseconds() as i64);
    run.segments()
        .iter()
        .map(|s| {
            let time = s.comparison(name);
            (
                nanos(time[TimingMethod::RealTime]),
                nanos(time[TimingMethod::GameTime]),
            )
        })
        .collect()
}

#[test]
fn describes_the_comparisons() {
    let plugin = Plugin::new(ID, settings(), engine()).unwrap();
    assert_eq!(
        plugin.comparisons(),
        ["Safe Pace", "Safer", "Average Of All", "Broken"]
    );
}

#[test]
fn matches_the_built_in_engine() {
    let mut expected = celeste();
    let programs = DOCUMENTS.iter().map(|d| format::read(d).unwrap().program);
    Library::new(programs).install(&mut expected);

    let mut actual = celeste();
    let plugin = Plugin::new(ID, settings(), engine()).unwrap();
    assert!(plugin.install(&mut actual).is_empty());

    for name in ["Safe Pace", "Safer", "Average Of All", "Broken"] {
        assert!(
            actual.comparisons().any(|c| c == name),
            "{name} is installed"
        );
        assert_eq!(splits(&actual, name), splits(&expected, name), "{name}");
    }
    assert!(
        splits(&actual, "Safe Pace")
            .iter()
            .all(|(r, _)| r.is_some())
    );
    assert!(splits(&actual, "Broken").iter().all(|&t| t == (None, None)));
}

#[test]
fn reports_errors_for_the_preview() {
    let run = celeste();
    let plugin = Plugin::new(ID, settings(), engine()).unwrap();
    let error = plugin
        .generate("Broken", run.segments(), run.attempt_history())
        .unwrap_err();
    assert_eq!(error.0, "There is no comparison named 'Does Not Exist'.");
}

#[test]
fn bad_settings_are_reported() {
    let error = Plugin::new(ID, "{ nope", engine()).unwrap_err();
    assert!(
        error
            .0
            .starts_with("The custom comparisons couldn't be read")
    );
}

#[test]
fn reinstalling_replaces_the_old_comparisons() {
    let mut run = celeste();
    Plugin::new(ID, settings(), engine())
        .unwrap()
        .install(&mut run);
    let fewer = format!(r#"{{"comparisons":[{AVERAGE_OF_ALL}]}}"#);
    Plugin::new(ID, fewer, engine()).unwrap().install(&mut run);

    assert!(!run.comparisons().any(|c| c == "Safe Pace"));
    assert!(run.comparisons().any(|c| c == "Average Of All"));
}

#[test]
fn names_already_taken_are_left_out() {
    let mut run = celeste();
    let clash = r#"{"comparisons":[{"format":"livesplit-custom-comparison","version":1,
        "name":"best segments","root":{"type":"aggregate","kind":"average",
        "source":{"type":"segmentHistory"}}}]}"#;
    let left_out = Plugin::new(ID, clash, engine()).unwrap().install(&mut run);
    assert_eq!(left_out, ["best segments"]);
}

#[test]
fn rejects_modules_that_arent_plugins() {
    assert!(WasmtimeEngine::new(b"not wasm").is_err());
    // A valid but empty module: no exports.
    let empty = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    let error = WasmtimeEngine::new(&empty).unwrap_err();
    assert!(
        error.0.starts_with("This isn't a comparison plugin"),
        "{error}"
    );
}

#[test]
fn timer_installs_and_removes_the_plugin() {
    use livesplit_core::{Timer, comparison::personal_best};

    let mut timer = Timer::new(celeste()).unwrap();
    let plugin = Plugin::new(ID, settings(), engine()).unwrap();
    assert!(timer.install_comparison_plugin(&plugin).is_empty());
    timer.set_current_comparison("Safe Pace").unwrap();

    timer.remove_comparison_plugin(ID);
    assert!(!timer.run().comparisons().any(|c| c == "Safe Pace"));
    assert_eq!(timer.current_comparison(), personal_best::NAME);
}

#[test]
fn the_editor_can_rename_comparisons_the_plugin_installed() {
    use livesplit_core::comparison::custom::{Editor, settings};

    let mut run = celeste();
    Plugin::new(ID, settings(), engine())
        .unwrap()
        .install(&mut run);
    let texts = settings::read(&settings()).unwrap();
    let editor = Editor::new(texts, &run);
    // The plugin's own comparisons don't block their names.
    assert_eq!(editor.check_name("Safe Pace", Some(0)), None);
    assert!(editor.check_name("Best Segments", None).is_some());
    assert_eq!(
        settings::read(&settings::write(editor.close()))
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn comparisons_are_taken_over_from_a_layout() {
    use livesplit_core::{Layout, comparison::custom::settings, component::custom_comparisons};

    let mut layout = Layout::new();
    let mut component = custom_comparisons::Component::new();
    component.settings_mut().comparisons = vec![SAFE_PACE.into(), "broken {".into()];
    layout.push(component);

    let texts = settings::read(&layout.custom_comparisons_settings()).unwrap();
    assert_eq!(texts.len(), 2);
    assert_eq!(format::read(&texts[0]).unwrap().program.name, "Safe Pace");
    assert_eq!(texts[1], "broken {");
}
