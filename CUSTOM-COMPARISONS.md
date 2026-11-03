# Custom comparisons and comparison plugins

This document describes how custom comparisons are built: the block language
and its file format, the generic comparison plugin interface that runs them,
the engines that host plugins, the C API, and how LiveSplit One stores and
installs them. It is written for people working on livesplit-core or on an
application that embeds it.

The runner's side (the editor, every block, import and export) is described
in LiveSplit One's
[`docs/custom-comparisons.md`](https://github.com/rodrigosiqueiradev/LiveSplitOne/blob/comparison-plugins/docs/custom-comparisons.md).

## Contents

1. [Overview](#1-overview)
2. [The block language](#2-the-block-language)
3. [The file format](#3-the-file-format)
4. [The evaluator](#4-the-evaluator)
5. [Comparison plugins](#5-comparison-plugins)
6. [The plugin ABI, version 1](#6-the-plugin-abi-version-1)
7. [Engines](#7-engines)
8. [The Custom Comparisons plugin](#8-the-custom-comparisons-plugin)
9. [Settings, the editor and desktop layouts](#9-settings-the-editor-and-desktop-layouts)
10. [C API](#10-c-api)
11. [LiveSplit One integration](#11-livesplit-one-integration)
12. [Building and testing](#12-building-and-testing)
13. [Design decisions](#13-design-decisions)

## 1. Overview

A custom comparison is a small program, a tree of blocks from a fixed set,
that turns the run's history into one time per segment. The format and the
semantics are those of the desktop LiveSplit component *Custom Comparisons*,
so a comparison file gives the same times in both.

The pieces, from the bottom up:

```
src/comparison/custom/          the language
  ast.rs                        block tree: Program, Times, Samples
  format.rs                     JSON format, versions 1 to 4, validation
  evaluator.rs                  evaluates programs against a RunData
  run_data.rs                   timing method specific snapshot of a run
  settings.rs                   {"comparisons": [...]}: the plugin's settings
  editor.rs                     list editor: names, renames, preview, state
  mod.rs                        Library, CustomComparison generator, preview

src/comparison/plugin/          the generic plugin interface
  abi.rs                        JSON documents that cross the boundary
  mod.rs                        Engine trait, Plugin, PluginComparison, snapshot
  wasmtime_engine.rs            native engine (feature comparison-plugins)
  web_engine.rs                 browser engine (feature web-comparison-plugins)

crates/livesplit-custom-comparisons-plugin/
                                the Custom Comparisons plugin (wasm32 cdylib)

src/layout/parser/custom_comparisons.rs
                                reads the comparisons out of a desktop .lsl

capi/src/comparison_plugin.rs   C API: ComparisonPlugin_*
capi/src/custom_comparisons_editor.rs
                                C API: CustomComparisonsEditor_*
```

At run time, an application:

1. keeps the plugin's **settings** (for custom comparisons, the list of
   comparisons as `{"comparisons": [...]}`) wherever it likes;
2. loads the **plugin module** with those settings into a `Plugin`, which
   asks the module which comparisons it provides;
3. **installs** the plugin on the timer's run. That adds one comparison
   generator per comparison, and they are recalculated between attempts like
   the built-in generators.

Nothing about custom comparisons or plugins is saved in the splits (`.lss`)
or the layout. The application owns the settings.

## 2. The block language

`ast.rs` has two block types, matching the two kinds of blocks the editor
shows:

- `Samples`: a **list of times** per segment. Sources and filters.
- `Times`: **one time** (or nothing) per segment. Everything else.

Keeping them as separate Rust types means an ill-typed program (a median of a
single time, say) can't be constructed. Only the JSON reader has to report
such mistakes.

All durations are whole nanoseconds (`Nanos = i64`), which keeps the
arithmetic exact. For example "stretch to total" puts the rounding leftovers
on the last segment so the total matches to the nanosecond. Everything is per
segment; times are summed into split times only at the very end.

| Kind | Variant | JSON `type` | Fields (JSON) | Since |
| --- | --- | --- | --- | --- |
| Samples | `SegmentHistory` | `segmentHistory` | | 1 |
| Samples | `LastN` | `lastN` | `count` 1..10000, `source` | 1 |
| Samples | `CompletedRunsOnly` | `completedRunsOnly` | `source` | 1 |
| Samples | `RecentDays` | `recentDays` | `days` 1..36500, `source` | 2 |
| Samples | `TrimOutliers` | `trimOutliers` | `percent` 0..45, `source` | 3 |
| Samples | `FinishedUnder` | `finishedUnder` | `totalSeconds` 0.001..360000, `source` | 3 |
| Samples | `AtLeast` | `atLeast` | `count` 1..10000, `source` | 3 |
| Samples | `ReachedSegment` | `reachedSegment` | `segment` 1..10000, `source` | 4 |
| Times | `Comparison` | `comparison` | `name` | 1 |
| Times | `Attempt` | `attempt` | `number` ≥ 1 | 3 |
| Times | `BestRecentRun` | `bestRecentRun` | `days` 1..36500 | 4 |
| Times | `Aggregate` | `aggregate` | `kind` (`min`, `max`, `average`, `median`), `source` (Samples) | 1 |
| Times | `Percentile` | `percentile` | `percent` 0..100, `source` (Samples) | 2 |
| Times | `WeightedAverage` | `weightedAverage` | `halfLife` 1..10000, `source` (Samples) | 4 |
| Times | `AddTime` | `addTime` | `seconds` -3600..3600, `source` | 1 |
| Times | `Scale` | `scale` | `percent` 1..1000, `source` | 1 |
| Times | `StretchToTotal` | `stretchToTotal` | `totalSeconds` 0.001..360000, `source` | 2 |
| Times | `Round` | `round` | `step` 0.001..3600 (seconds), `source` | 4 |
| Times | `Blend` | `blend` | `percent` 0..100, `from`, `to` | 2 |
| Times | `Pick` | `pick` | `kind` (`faster`, `slower`), `first`, `second` | 2 |
| Times | `Clamp` | `clamp` | `source`, `low`, `high` | 4 |
| Times | `Fallback` | `fallback` | `primary`, `fallback` | 1 |
| Times | `SegmentRange` | `segmentRange` | `cases` (≤ 100 of `{from, to, times}`), `otherwise` | 1 |

The limits are the constants at the top of `format.rs` (`MAX_COUNT`,
`MAX_DAYS`, `MAX_TOTAL_SECONDS`, ...).

## 3. The file format

One comparison is one JSON document:

```json
{
  "format": "livesplit-custom-comparison",
  "version": 1,
  "name": "Safe Pace",
  "root": {
    "type": "fallback",
    "primary": {
      "type": "aggregate",
      "kind": "median",
      "source": {
        "type": "lastN",
        "count": 10,
        "source": {
          "type": "completedRunsOnly",
          "source": { "type": "segmentHistory" }
        }
      }
    },
    "fallback": { "type": "comparison", "name": "Personal Best" }
  },
  "workspace": { }
}
```

- `format` must be `livesplit-custom-comparison`.
- `version` is 1 to `CURRENT_VERSION` (4). A newer version is rejected with
  a message asking the runner to update.
- `name` is trimmed, non-empty and at most 64 characters.
- `root` is a `Times` block.
- `workspace` is the desktop Blockly editor's layout (block positions and
  so on). It is kept as is in `Document::workspace` and never evaluated.
  LiveSplit One's editor writes it back only if the blocks didn't change, so
  the desktop editor can lay out a changed comparison itself.

### Versions 1 to 4

The version is not a schema switch: every version is a superset of the one
before, and the only difference is which block types exist.

| Version | Blocks added |
| --- | --- |
| 1 | `segmentHistory`, `lastN`, `completedRunsOnly`, `comparison`, `aggregate`, `addTime`, `scale`, `fallback`, `segmentRange` |
| 2 | `percentile`, `recentDays`, `stretchToTotal`, `blend`, `pick` |
| 3 | `trimOutliers`, `finishedUnder`, `atLeast`, `attempt` |
| 4 | `reachedSegment`, `weightedAverage`, `clamp`, `round`, `bestRecentRun` |

Reading accepts any version up to 4 and doesn't check that a file only uses
blocks of its declared version. Writing (`format::write`,
`format::write_compact`) always picks the **lowest** version that holds every
block (`required_version`), so a comparison that only uses old blocks still
opens in old desktop releases. Times are written in seconds rounded to
milliseconds and whole numbers without a fraction, like the desktop.

### Errors

`format::read` validates every value and reports the first problem with its
path, for example:

```
root.source.count: must be a whole number from 1 to 10000.
root.primary: "lastN" gives a list of times per segment. Put it inside a min, max, average, median, percentile or weighted average block.
root.cases[2].to: must not be before "from".
```

The messages are shown to runners and match the desktop component word for
word.

## 4. The evaluator

`Evaluator` evaluates a set of programs (a `Library`) against a `RunData`:
one timing method's view of the run (segments with their history by attempt,
attempts with date and final time, the comparisons the programs refer to,
and "now"). The rules, shared with the desktop:

- **Per segment.** An empty segment stays empty unless a fallback fills it.
  Once a segment is empty, every later split time is empty.
- **Segment history** follows LiveSplit's skip rules: a skipped segment is
  left out, and so is the segment right after it, because its recorded time
  covers both. An attempt stops contributing at the first segment it never
  reached. Only attempt ids ≥ 1 count; lower ids hold times that were made up
  when segments were added or history imported.
- **References.** A `comparison` block that names another program evaluates
  that program directly (cached, with loop detection) instead of reading what
  its generator stored. Generator order therefore doesn't matter, and a loop
  is an error instead of stale times. Other names are read from
  `RunData::comparisons` as split times and converted to segment times.
- **Built-in comparisons** (Best Segments, Average Segments, ...) are
  generated on a copy of the segments when needed, so they work even if the
  runner removed those generators.
- **Errors** (a loop, a missing comparison, an attempt that doesn't exist)
  leave the whole comparison empty and are returned as a message.
- `evaluate_with_origins` additionally returns, per segment, the path to the
  block that produced the time (like `primary.cases.0.times`), following the
  branch taken by fallback, segment range, pick and clamp. The editor's
  preview uses it to highlight the block.

Notable details: `lastN` sorts by attempt id and keeps the newest;
`trimOutliers` drops `floor(len * percent / 100)` from each end but keeps the
survivors in attempt order; `percentile` interpolates linearly between ranks;
`weightedAverage` gives the k-th newest time the weight `0.5^(k / halfLife)`;
`round` rounds halves up; `clamp` applies "never slower" first, then "never
faster", so the low limit wins when they cross; `pick` gives ties and an
empty second side to the first.

`CustomComparison` (in `mod.rs`) is the `ComparisonGenerator` for one program
when the language runs **inside** livesplit-core. `comparison::custom::preview`
evaluates a program for the editor without touching the run. Both still
exist, but applications install custom comparisons through the plugin
described next.

## 5. Comparison plugins

A comparison plugin is a sandboxed WebAssembly module that turns the run's
history into one or more comparisons. It's a pure function: it imports
nothing, gets a snapshot of the run and returns split times. The Custom
Comparisons plugin is one such plugin; the interface is generic.

```rust
use livesplit_core::comparison::plugin::{self, Plugin};

let engine = plugin::engine_for(&module_bytes)?;           // best engine for this build
let plugin = Plugin::new("livesplit.custom-comparisons", settings, engine)?;
let left_out = timer.install_comparison_plugin(&plugin);  // names already taken
// later
timer.remove_comparison_plugin("livesplit.custom-comparisons");
```

- `Plugin::new` calls `describe` once and keeps the list of comparison names.
  A plugin with new settings is a new `Plugin`.
- `Plugin::install(&mut Run)` / `Timer::install_comparison_plugin` remove the
  generators of any earlier plugin **with the same id**, add one
  `PluginComparison` generator per comparison, and regenerate comparisons. A
  name that `custom::check_name` rejects (empty, reserved, `[Race]` prefix,
  or a comparison the run already has, like one stored in the splits) is left
  out and returned.
- `Timer::remove_comparison_plugin(id)` removes them again. Both timer
  methods switch the current comparison to the Personal Best if it no longer
  exists, and keep the current attempt.
- `PluginComparison::generate` calls the plugin and writes the split times
  into the segments. Any failure (trap, fuel, garbage, an `error`) leaves the
  comparison empty. A plugin can never affect the timer.
- `Plugin::generate` runs one comparison without storing it, for previews.
- `ComparisonGenerator::as_plugin` finds the generators a plugin installed.

Installing a plugin changes nothing that is saved in the splits, just like
the built-in generators.

## 6. The plugin ABI, version 1

A plugin module **imports nothing** and exports:

```text
memory
lscp_abi_version() -> i32                 // must return 1
lscp_alloc(len: i32) -> i32               // a buffer the host writes input to
lscp_describe(ptr: i32, len: i32) -> i64  // DescribeInput -> DescribeOutput
lscp_generate(ptr: i32, len: i32) -> i64  // GenerateInput -> GenerateOutput
```

The calling convention for `describe` and `generate`:

1. The host calls `lscp_alloc(len)` and writes the UTF-8 JSON input into
   linear memory at the returned pointer. The buffer then belongs to the
   plugin, which frees it.
2. The host calls the function with `(ptr, len)`.
3. The function returns `(out_ptr << 32) | out_len`, pointing at UTF-8 JSON
   output. It only needs to stay valid until the next call into the plugin.

Times are whole **nanoseconds**, dates **milliseconds since the Unix epoch**
(UTC). The types live in `comparison::plugin::abi`.

### describe

```json
// input
{ "settings": "<the plugin's settings, as the application keeps them>" }
// output
{ "comparisons": ["Safe Pace", "Goal Pace"] }
// or
{ "comparisons": [], "error": "The custom comparisons couldn't be read: ..." }
```

An `error` makes `Plugin::new` fail with that message.

### generate

```json
// input
{
  "settings": "...",
  "comparison": "Safe Pace",
  "run": {
    "now": 1762178400000,
    "segments": [
      { "name": "Forsaken City",
        "history": [ { "attempt": 1, "real": 123000000000, "game": null }, ... ] }
    ],
    "attempts": [
      { "id": 1, "started": 1762170000000, "ended": 1762171800000,
        "real": 1800000000000, "game": null }
    ],
    "comparisons": {
      "Personal Best": { "real": [123000000000, null], "game": [null, null] },
      "Best Segments": { "real": [...], "game": [...] }
    }
  }
}
// output
{ "real": [123000000000, 250000000000], "game": [null, null] }
```

- `segments[].history` lists every recorded segment time by attempt id,
  including ids below 1 (made-up history). An attempt that never reached the
  segment isn't listed; a `null` time means the segment was skipped.
- `attempts[]` has the final time per timing method if the attempt finished.
- `comparisons` has every comparison stored in the segments (the Personal
  Best, ones the runner entered like a World Record, whatever other
  generators produced) **plus all built-in generated comparisons**, generated
  on a copy so they're present even if the runner turned them off. The
  plugin's own comparisons are left out.
- The output has **cumulative split times**, one per segment and timing
  method. Missing entries are empty. An `error` leaves the comparison empty;
  the custom comparisons plugin also returns the times it could calculate
  together with an error, but the host currently treats any error as empty.

### Versioning

`lscp_abi_version` is checked when the plugin is instantiated. Anything other
than `ABI_VERSION` (1) is rejected with a message naming both versions. New
optional JSON fields can be added without a new ABI version, since unknown
fields are ignored and missing ones default; a change in the calling
convention or the meaning of a field needs a new version.

## 7. Engines

`Engine` is a small trait (`describe(&[u8])`, `generate(&[u8])`, JSON in and
out) so the host can pick the WebAssembly runtime. `plugin::engine_for`
picks the best one the build has:

| Engine | Feature | Where | Notes |
| --- | --- | --- | --- |
| `WebEngine` | `web-comparison-plugins` (implies `wasm-web`) | `wasm32` in the browser | Uses the browser's own `WebAssembly` through `js-sys`. `new` compiles synchronously, which browsers only allow for small modules on the main thread (Chrome: up to 8 MB); `from_module` takes a module compiled asynchronously. Calls can't be interrupted, so only run plugins the runner chose. |
| `WasmtimeEngine` | `comparison-plugins` (implies `std`, wasmtime with cranelift) | native | Each call gets fuel for roughly 5 billion instructions (about a second or two) and memory is limited to 256 MiB. |

`engine_for` prefers the web engine when both could apply, and returns an
error ("This version of LiveSplit can't run comparison plugins.") if neither
feature is on.

Both engines:

- reject modules that import anything;
- instantiate once in `new` to check the ABI version and the exports, so a
  broken module fails at load time with a readable message;
- keep the instance between calls, and **drop it after any failed call**, so
  the next call starts from a fresh instance instead of a plugin left in an
  unknown state.

LiveSplit One's web build can't use wasmtime (and doesn't ship the
auto-splitting runtime), which is why the browser engine exists rather than
reusing livesplit-auto-splitting.

## 8. The Custom Comparisons plugin

`crates/livesplit-custom-comparisons-plugin` is a `cdylib` + `rlib` that
depends on livesplit-core with no default features and reuses the language
from `comparison::custom`:

- id `livesplit.custom-comparisons` (`settings::PLUGIN_ID`);
- `describe`: reads the settings into a `Library` and returns the program
  names. Entries that can't be read are skipped, like on the desktop; the
  editor shows what's wrong with them. Duplicate names keep the first one.
- `generate`: evaluates the comparison for Real Time and Game Time from the
  snapshot, dropping history with attempt ids below 1 and using an attempt's
  end date (or start date) for the day-based blocks;
- the export functions are thin wrappers around public Rust functions, so the
  plugin can be tested without a WebAssembly runtime.

Build it with:

```sh
cargo build -p livesplit-custom-comparisons-plugin --target wasm32-unknown-unknown --release
```

## 9. Settings, the editor and desktop layouts

### Settings

The plugin's settings are the runner's list of comparisons, and the same
document is the "Export All" file:

```json
{ "comparisons": [ { "format": "livesplit-custom-comparison", "version": 4, ... }, ... ] }
```

`comparison::custom::settings::read` returns each entry as text. Empty
settings are an empty list. An entry that isn't a JSON object is kept as a
JSON string, so a comparison that couldn't even be parsed is never lost;
`settings::write` turns each text back into an object when it parses, a
string otherwise.

### Editor

`comparison::custom::Editor` manages that list of texts with the desktop
settings' rules:

- names are unique (ignoring case and surrounding spaces), can't be reserved
  (`RESERVED_NAMES`: Personal Best, None, Best Segments, Best Split Times,
  Average Segments, Median Segments, Worst Segments, Balanced PB, Latest Run,
  Golf HCP), can't start with `[Race]`, and can't be a comparison the run
  already has;
- an entry that can't be read stays byte for byte as it is: it can be
  exported, moved and removed, not renamed or edited;
- `rename` also updates the `comparison` blocks of the other comparisons
  (and the `cc_comparison` blocks in their desktop workspace), and fails
  without changing anything if it can't;
- `state(run)` reports each entry's name, whether it's readable, its problem
  for this run, and how many others use it; `preview(...)` evaluates an
  edited text with origins for the preview table.

The editor is created from settings and turns back into settings when
closed, so the settings stay the only place the comparisons live.

### Desktop layouts

`layout::parser::custom_comparisons_settings(lsl)` reads a desktop `.lsl`
and collects the `<Comparison>` texts of every component whose `<Path>` is
`LiveSplit.CustomComparisons.dll`, from its
`<Settings><Comparisons>...</Comparisons></Settings>`. It returns them as
plugin settings, every entry as it is. This replaces the earlier Custom
Comparisons layout component, which has been removed from livesplit-core.

## 10. C API

Two classes, generated into every binding by `capi/bind_gen`.

### ComparisonPlugin (feature `comparison-plugins` or `web-comparison-plugins` of the capi crate)

| Function | What it does |
| --- | --- |
| `ComparisonPlugin_new(id, settings, module, length)` | Loads the module with the best engine and describes it. Returns `NULL` on failure. |
| `ComparisonPlugin.fromArray(id, settings, Uint8Array)` | JavaScript/TypeScript helper around `new` (wasm-bindgen bindings only). |
| `ComparisonPlugin_load_error()` | Why the last `new` on this thread returned `NULL`; empty otherwise. |
| `ComparisonPlugin_id(this)` | The plugin's id. |
| `ComparisonPlugin_comparisons(this)` | JSON array of comparison names. |
| `ComparisonPlugin_install(this, timer)` | Installs on the timer's run, replacing the same id's comparisons. Returns a JSON array of the names left out. The plugin can be dropped afterwards. |
| `ComparisonPlugin_uninstall(id, timer)` | Removes the comparisons of the plugin with that id. |
| `ComparisonPlugin_drop(this)` | |

### CustomComparisonsEditor (part of `run-editing`)

| Function | What it does |
| --- | --- |
| `from_settings(settings, run)` | Opens the editor; `NULL` if the settings can't be read. |
| `into_settings(this)` | Closes it and returns the settings (also the Export All file). |
| `read_settings(settings)` | JSON array of each comparison's text, or `null`. |
| `read_layout(lsl)` | The comparisons of a desktop `.lsl` as settings; empty string if it isn't a layout. |
| `state_as_json(this, run)` | Name, readability, problem and use count of every comparison. |
| `len`, `name(index)` | |
| `check_name(name)`, `check_rename(index, name)`, `suggest_name(name)` | Empty string when allowed, otherwise the reason; a free name like "Pace (2)". |
| `add(text)`, `add_as(text, name)`, `replace(index, text)`, `rename(index, name)` | Empty string on success, otherwise the reason. |
| `remove(index)`, `move_up(index)`, `move_down(index)`, `used_by(index)` | |
| `export(index)`, `export_compact(index)` | Indented JSON for a file, one line for chat. |
| `preview_as_json(this, text, index, run, method)` | Per segment: name, segment and split time in seconds, origin path; or the error. |

The editor used to be part of `layout-editing`; it moved to `run-editing`
when the layout component was removed.

## 11. LiveSplit One integration

LiveSplit One (branch `comparison-plugins` of the fork) uses all of the
above:

- **Build.** `buildCore.ts` enables `web-comparison-plugins` and also builds
  `livesplit-custom-comparisons-plugin` for `wasm32-unknown-unknown` in
  release mode, copying it to `src/livesplit-core/custom_comparisons_plugin.wasm`.
- **Loading.** `src/util/CustomComparisonsPlugin.ts` fetches that module once
  at start-up (Vite `?url` import), so creating a plugin for new settings
  later is synchronous through `ComparisonPlugin.fromArray`. If the fetch
  fails, LiveSplit One still works and the Custom Comparisons view shows why.
- **Storage.** The settings are one global list in IndexedDB, store
  `settings`, key `customComparisons` (`storeCustomComparisons` /
  `loadCustomComparisons` in `src/storage/index.ts`). `undefined` means
  "never stored", which is different from an empty list.
- **Installing.** `LSOCommandSink.setCustomComparisons(plugin)` owns the
  current plugin, installs it, and re-installs it in `setRun`, so every way of
  loading splits gets the custom comparisons. `null` uninstalls by id. Each
  change in the view stores the settings, creates a new plugin and installs
  it.
- **Start-up order.** The plugin is installed before the stored current
  comparison is restored, since that may be a custom comparison.
- **Migration.** `takeCustomComparisonsOutOfLayout` strips
  `CustomComparisons` components (also inside `Group` and `Carousel`) from a
  JSON layout. At start-up, if no list was ever stored, the comparisons of
  the stored layout's components become the list. Imported `.ls1l` layouts
  are stripped too, so they still parse.
- **UI.** `src/ui/views/CustomComparisonsView.tsx` (opened from the timer
  sidebar) hosts the list and editor in `src/ui/views/CustomComparisons/`.
  `library.ts` opens a `CustomComparisonsEditor` per action and closes it
  right away, so no wasm object outlives an action. Import accepts an Export
  All list, a single comparison, or an `.lsl` (via `readLayout`).

## 12. Building and testing

```sh
# The language and the plugin interface
cargo test --lib comparison

# Runs the real plugin through wasmtime and checks it matches the built-in
# evaluator on real splits. Needs the wasm32-unknown-unknown target, as the
# test builds the plugin with cargo.
rustup target add wasm32-unknown-unknown
cargo test --features comparison-plugins --test comparison_plugins

# The desktop .lsl reader
cargo test --test layout_parsing
```

`src/comparison/custom/tests.rs` covers the format and the evaluator rules;
`src/comparison/plugin/tests.rs` covers the snapshot and installation.

## 13. Design decisions

- **A plugin, not a layout component.** LiveSplit One's components are
  strictly visual, so a component that draws nothing and only feeds
  comparisons doesn't fit. A generic comparison plugin keeps livesplit-core's
  comparison code free of the block language's storage and lets other
  comparison ideas use the same mechanism.
- **WebAssembly with no imports and JSON in and out.** A plugin is a pure
  function of the run, which makes it safe to run anywhere, easy to test,
  and independent of livesplit-core's internal types. JSON costs a little
  speed, but comparisons are only calculated between attempts.
- **Two engines.** wasmtime gives fuel and memory limits natively; the
  browser can't run wasmtime, so it uses its own engine.
- **Settings owned by the application.** One global list in LiveSplit One's
  storage, applied to every run, with export and import. No change to the
  `.lss` or layout formats.
- **Same format and messages as the desktop.** A comparison file can be
  shared between the desktop component and LiveSplit One, and gives the same
  times in both.
