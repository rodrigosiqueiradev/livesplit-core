//! The editor for the list of custom comparisons in the settings of the
//! Custom Comparisons plugin.
//!
//! The comparisons are stored as text (see [`settings`](super::settings)), so
//! that one that can't be read, for example because it was made with a
//! newer version, is never lost. The [`Editor`] manages that list with the
//! same rules as the settings of the desktop LiveSplit component *Custom
//! Comparisons*:
//!
//! - Names are unique, ignoring case, and can't be one of LiveSplit's own
//!   comparisons or one the run already has.
//! - A comparison that can't be read stays exactly as it is. It can be
//!   exported, moved and removed, but not renamed.
//! - Renaming a comparison updates the other comparisons that use it, so they
//!   don't stop working.
//!
//! It also provides the state of the list and a preview of a comparison being
//! edited, for a user interface to show.

use super::{
    Document, Library, Program, ast::Times, check_name, format, preview as evaluate, same_name,
};
use crate::{Run, TimingMethod, platform::prelude::*};
use serde_derive::{Deserialize, Serialize};
use serde_json::Value;

/// The block editor's comparison block, inside a stored workspace.
const COMPARISON_BLOCK_TYPE: &str = "cc_comparison";

/// The message for a rename that couldn't update the comparisons using it.
const RENAME_FAILED: &str =
    "Couldn't update the comparisons that use this one, so nothing was renamed.";

/// One stored comparison: the exact text it was loaded from and the
/// comparison read from it, or why it couldn't be read.
#[derive(Clone, Debug)]
struct Entry {
    text: String,
    document: Result<Document, String>,
}

impl Entry {
    fn load(text: String) -> Self {
        let document = format::read(&text).map_err(|e| e.0);
        Self { text, document }
    }

    fn from_document(document: Document) -> Self {
        Self {
            text: format::write_compact(&document),
            document: Ok(document),
        }
    }

    fn program(&self) -> Option<&Program> {
        self.document.as_ref().ok().map(|d| &d.program)
    }

    /// The comparison's name, or a best guess for one that can't be read.
    fn name(&self) -> String {
        match &self.document {
            Ok(document) => document.program.name.clone(),
            Err(_) => guess_name(&self.text).unwrap_or_else(|| "(unreadable comparison)".into()),
        }
    }
}

fn guess_name(text: &str) -> Option<String> {
    let json: Value = serde_json::from_str(text).ok()?;
    let name = json.get("name")?.as_str()?.trim();
    (!name.is_empty()).then(|| name.into())
}

/// Reads the stored comparisons. A comparison named like an earlier one is
/// treated as unreadable, so it's never used, but it's kept as it is.
fn load(texts: impl IntoIterator<Item = String>) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for text in texts {
        let mut entry = Entry::load(text);
        if let Ok(document) = &entry.document {
            let name = &document.program.name;
            if entries
                .iter()
                .filter_map(Entry::program)
                .any(|p| same_name(&p.name, name))
            {
                entry.document = Err(format!(
                    "Another comparison is already called '{name}'. Rename one of them."
                ));
            }
        }
        entries.push(entry);
    }
    entries
}

/// Manages the list of custom comparisons stored in a layout's Custom
/// Comparisons component. See the [module documentation](self) for the rules.
#[derive(Clone, Debug)]
pub struct Editor {
    entries: Vec<Entry>,
    other_comparisons: Vec<String>,
}

/// The state of the list of comparisons, meant to be shown by a user
/// interface.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    /// The comparisons, in the order they are stored and offered in.
    pub comparisons: Vec<ComparisonState>,
}

/// The state of one comparison in the list.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ComparisonState {
    /// The comparison's name. For a comparison that can't be read, this is
    /// the best guess.
    pub name: String,
    /// Whether the comparison could be read. One that can't be read can't be
    /// renamed or edited, but it can be exported, moved and removed.
    pub readable: bool,
    /// Why the comparison doesn't work, in words meant for the runner, or
    /// nothing if it works.
    pub problem: Option<String>,
    /// How many of the other comparisons use this one. They stop working if
    /// it's removed.
    pub used_by: usize,
}

/// A preview of a comparison being edited, for one timing method.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PreviewState {
    /// Why the comparison can't be calculated, in words meant for the runner,
    /// or nothing if it can.
    pub error: Option<String>,
    /// One entry per segment of the run.
    pub segments: Vec<PreviewSegment>,
}

/// The preview of one segment.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PreviewSegment {
    /// The name of the segment.
    pub name: String,
    /// The segment's time in seconds, if there is one.
    pub segment_time: Option<f64>,
    /// The split time in seconds, if there is one. Once a segment is empty,
    /// every later split is empty too.
    pub split_time: Option<f64>,
    /// The block the segment's time came from, as the path from the root
    /// block through the file format's keys, like `primary.cases.0.times`.
    /// An empty path is the root block itself. Nothing if there's no time.
    pub origin: Option<String>,
}

#[cfg(feature = "std")]
impl State {
    /// Encodes the state object's information as JSON.
    pub fn write_json<W>(&self, writer: W) -> serde_json::Result<()>
    where
        W: std::io::Write,
    {
        serde_json::to_writer(writer, self)
    }
}

#[cfg(feature = "std")]
impl PreviewState {
    /// Encodes the preview's information as JSON.
    pub fn write_json<W>(&self, writer: W) -> serde_json::Result<()>
    where
        W: std::io::Write,
    {
        serde_json::to_writer(writer, self)
    }
}

impl Editor {
    /// Creates an editor for the stored comparisons. The run provides the
    /// comparisons the custom comparisons can't be named after, like the ones
    /// stored in the splits.
    pub fn new(comparisons: impl IntoIterator<Item = String>, run: &Run) -> Self {
        let custom: Vec<&str> = run
            .comparison_generators()
            .iter()
            .filter(|g| {
                g.as_custom().is_some()
                    || g.as_plugin()
                        .is_some_and(|p| p.plugin().id() == super::settings::PLUGIN_ID)
            })
            .map(|g| g.name())
            .collect();
        let other_comparisons = run
            .comparisons()
            .filter(|c| !custom.contains(c))
            .map(String::from)
            .collect();
        Self {
            entries: load(comparisons),
            other_comparisons,
        }
    }

    /// Closes the editor and gives back the comparisons to store.
    pub fn close(self) -> Vec<String> {
        self.entries.into_iter().map(|e| e.text).collect()
    }

    /// The comparisons to store, in order.
    pub fn comparisons(&self) -> impl Iterator<Item = &str> + '_ {
        self.entries.iter().map(|e| e.text.as_str())
    }

    /// The number of comparisons.
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no comparisons.
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The name of the comparison at the index. For a comparison that can't
    /// be read, this is the best guess.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds.
    pub fn name(&self, index: usize) -> String {
        self.entries[index].name()
    }

    /// The comparisons that can be read, as a library.
    pub fn library(&self) -> Library {
        Library::new(self.entries.iter().filter_map(Entry::program).cloned())
    }

    /// Checks whether `name` can be used for a comparison. Returns a message
    /// for the runner if it can't. `ignoring` is the index of the comparison
    /// being renamed, whose current name doesn't count as taken.
    pub fn check_name(&self, name: &str, ignoring: Option<usize>) -> Option<String> {
        let names: Vec<String> = self
            .entries
            .iter()
            .enumerate()
            .filter(|&(i, _)| Some(i) != ignoring)
            .map(|(_, e)| e.name())
            .collect();
        check_name(
            name,
            names
                .iter()
                .chain(&self.other_comparisons)
                .map(String::as_str),
        )
    }

    /// A name that isn't taken yet, based on `name`: "Pace", "Pace (2)", ...
    pub fn suggest_name(&self, name: &str) -> String {
        let name = name.trim();
        let base = if name.is_empty() {
            "Custom Comparison"
        } else {
            name
        };
        if self.check_name(base, None).is_none() {
            return base.into();
        }
        (2..1000)
            .map(|i| format!("{base} ({i})"))
            .find(|candidate| self.check_name(candidate, None).is_none())
            .unwrap_or_else(|| format!("{base} ({})", self.entries.len() + 1000))
    }

    /// Adds a comparison from its text, like an imported file or text pasted
    /// from a chat. Returns why it can't be added, for example because its
    /// name is taken. In that case, [`suggest_name`](Self::suggest_name) and
    /// [`add_as`](Self::add_as) can be used to add it under another name.
    pub fn add(&mut self, text: &str) -> Result<(), String> {
        let document = format::read(text).map_err(|e| e.0)?;
        if let Some(problem) = self.check_name(&document.program.name, None) {
            return Err(problem);
        }
        self.entries.push(Entry::from_document(document));
        Ok(())
    }

    /// Adds a comparison from its text under another name. Returns why it
    /// can't be added.
    pub fn add_as(&mut self, text: &str, name: &str) -> Result<(), String> {
        let mut document = format::read(text).map_err(|e| e.0)?;
        if let Some(problem) = self.check_name(name, None) {
            return Err(problem);
        }
        document.program.name = name.trim().into();
        self.entries.push(Entry::from_document(document));
        Ok(())
    }

    /// Replaces the comparison at the index with the one in the text, like
    /// after editing it. This also works for a comparison that can't be read.
    /// The comparisons using the old name are not updated; use
    /// [`rename`](Self::rename) for that. Returns why it can't be replaced.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds.
    pub fn replace(&mut self, index: usize, text: &str) -> Result<(), String> {
        assert!(index < self.entries.len(), "The index is out of bounds.");
        let document = format::read(text).map_err(|e| e.0)?;
        if let Some(problem) = self.check_name(&document.program.name, Some(index)) {
            return Err(problem);
        }
        self.entries[index] = Entry::from_document(document);
        self.reload();
        Ok(())
    }

    /// Renames the comparison at the index and updates the comparisons that
    /// use it. Returns why it can't be renamed. Nothing changes in that case.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds.
    pub fn rename(&mut self, index: usize, name: &str) -> Result<(), String> {
        let Some(program) = self.entries[index].program() else {
            return Err("This comparison can't be read, so it can't be renamed.".into());
        };
        if let Some(problem) = self.check_name(name, Some(index)) {
            return Err(problem);
        }
        let old_name = program.name.clone();
        let new_name = name.trim();
        if new_name == old_name {
            return Ok(());
        }

        // Build the whole list first, so a failure leaves everything as it was.
        let mut texts = Vec::with_capacity(self.entries.len());
        for (i, entry) in self.entries.iter().enumerate() {
            let Ok(document) = &entry.document else {
                // Unreadable comparisons stay exactly as they are.
                texts.push(entry.text.clone());
                continue;
            };
            let mut document = document.clone();
            let changed = if i == index {
                document.program.name = new_name.into();
                true
            } else {
                rewrite(&mut document.program.root, &old_name, new_name)
            };
            if !changed {
                // Neither does a comparison that doesn't use this one.
                texts.push(entry.text.clone());
                continue;
            }
            if let Some(workspace) = &mut document.workspace {
                rewrite_workspace(workspace, &old_name, new_name);
            }
            let text = format::write_compact(&document);
            if format::read(&text).is_err() {
                return Err(RENAME_FAILED.into());
            }
            texts.push(text);
        }

        self.entries = load(texts);
        Ok(())
    }

    /// Removes the comparison at the index. The comparisons using it stop
    /// working (see [`State`]'s `used_by`).
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds.
    pub fn remove(&mut self, index: usize) {
        self.entries.remove(index);
        self.reload();
    }

    /// Moves the comparison at the index up by one, which is also its place in
    /// the list of comparisons. Returns whether it moved.
    pub fn move_up(&mut self, index: usize) -> bool {
        if index == 0 || index >= self.entries.len() {
            return false;
        }
        self.entries.swap(index - 1, index);
        self.reload();
        true
    }

    /// Moves the comparison at the index down by one. Returns whether it
    /// moved.
    pub fn move_down(&mut self, index: usize) -> bool {
        if index + 1 >= self.entries.len() {
            return false;
        }
        self.entries.swap(index, index + 1);
        self.reload();
        true
    }

    /// The comparison at the index as indented JSON, for saving as a file.
    /// A comparison that can't be read is returned exactly as stored, so
    /// nothing is lost.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds.
    pub fn export(&self, index: usize) -> String {
        let entry = &self.entries[index];
        match &entry.document {
            Ok(document) => format::write(document),
            Err(_) => entry.text.clone(),
        }
    }

    /// The comparison at the index as JSON on a single line, for copying it
    /// into a chat. A comparison that can't be read is returned exactly as
    /// stored.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds.
    pub fn export_compact(&self, index: usize) -> String {
        let entry = &self.entries[index];
        match &entry.document {
            Ok(document) => format::write_compact(document),
            Err(_) => entry.text.clone(),
        }
    }

    /// How many of the other comparisons use the one at the index. They stop
    /// working if it's removed.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds.
    pub fn used_by(&self, index: usize) -> usize {
        let name = self.entries[index].name();
        self.entries
            .iter()
            .enumerate()
            .filter(|&(i, e)| i != index && e.program().is_some_and(|p| uses(&p.root, &name)))
            .count()
    }

    /// Calculates the state of the list, for the run the comparisons are
    /// used with. A comparison has a problem if it can't be read, if the run
    /// already has a comparison with its name, or if it can't be calculated
    /// for either timing method.
    pub fn state(&self, run: &Run) -> State {
        let library = self.library();
        let names: Vec<String> = run.comparisons().map(String::from).collect();
        let comparisons =
            self.entries
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    let problem = match &entry.document {
                        Err(error) => Some(error.clone()),
                        Ok(document) => {
                            let name = &document.program.name;
                            check_name(name, self.other_comparisons.iter().map(String::as_str))
                                .or_else(|| {
                                    [TimingMethod::RealTime, TimingMethod::GameTime]
                                        .into_iter()
                                        .find_map(|method| {
                                            evaluate(
                                                &library,
                                                name,
                                                run.segments(),
                                                run.attempt_history(),
                                                &names,
                                                method,
                                            )?
                                            .error
                                        })
                                })
                        }
                    };
                    ComparisonState {
                        name: entry.name(),
                        readable: entry.document.is_ok(),
                        problem,
                        used_by: self.used_by(index),
                    }
                })
                .collect();
        State { comparisons }
    }

    /// Calculates a preview of a comparison being edited, given as text, for
    /// the run and timing method. `index` is the comparison the text replaces,
    /// or [`None`] for a new comparison. The other comparisons can be used by
    /// the one being edited.
    pub fn preview(
        &self,
        text: &str,
        index: Option<usize>,
        run: &Run,
        method: TimingMethod,
    ) -> PreviewState {
        let segments = run.segments();
        let empty = |error: String| PreviewState {
            error: Some(error),
            segments: segments
                .iter()
                .map(|s| PreviewSegment {
                    name: s.name().into(),
                    ..Default::default()
                })
                .collect(),
        };

        let document = match format::read(text) {
            Ok(document) => document,
            Err(error) => return empty(error.0),
        };
        let name = document.program.name.clone();
        let others = self
            .entries
            .iter()
            .enumerate()
            .filter(|&(i, _)| Some(i) != index)
            .filter_map(|(_, e)| e.program())
            .filter(|p| !same_name(&p.name, &name))
            .cloned();
        let library = Library::new(core::iter::once(document.program).chain(others));
        let names: Vec<String> = run.comparisons().map(String::from).collect();

        let Some(result) = evaluate(
            &library,
            &name,
            segments,
            run.attempt_history(),
            &names,
            method,
        ) else {
            return empty("The comparison couldn't be calculated.".into());
        };
        if let Some(error) = result.error {
            return empty(error);
        }

        let seconds = |nanos: Option<i64>| nanos.map(|n| n as f64 / 1_000_000_000.0);
        let mut origins = result.origins.unwrap_or_default().into_iter();
        PreviewState {
            error: None,
            segments: segments
                .iter()
                .zip(result.segment_times.iter().zip(&result.split_times))
                .map(|(segment, (&time, &split))| {
                    let origin = origins.next();
                    PreviewSegment {
                        name: segment.name().into(),
                        segment_time: seconds(time),
                        split_time: seconds(split),
                        origin: time.and(origin),
                    }
                })
                .collect(),
        }
    }

    /// Reads the comparisons again, so that a comparison that was a duplicate
    /// of another one works again once the other one is gone or renamed.
    fn reload(&mut self) {
        let texts: Vec<String> = self.entries.drain(..).map(|e| e.text).collect();
        self.entries = load(texts);
    }
}

/// Whether the block uses the comparison with the given name.
fn uses(node: &Times, name: &str) -> bool {
    let mut found = false;
    visit(node, &mut |n| {
        if let Times::Comparison { name: n } = n {
            found |= same_name(n, name);
        }
    });
    found
}

fn visit(node: &Times, f: &mut impl FnMut(&Times)) {
    use Times::*;
    f(node);
    match node {
        Comparison { .. }
        | Attempt { .. }
        | BestRecentRun { .. }
        | Aggregate { .. }
        | Percentile { .. }
        | WeightedAverage { .. } => {}
        AddTime { source, .. }
        | Scale { source, .. }
        | StretchToTotal { source, .. }
        | Round { source, .. } => visit(source, f),
        Blend { from, to, .. } => {
            visit(from, f);
            visit(to, f);
        }
        Pick { first, second, .. } => {
            visit(first, f);
            visit(second, f);
        }
        Clamp { source, low, high } => {
            visit(source, f);
            visit(low, f);
            visit(high, f);
        }
        Fallback { primary, fallback } => {
            visit(primary, f);
            visit(fallback, f);
        }
        SegmentRange { cases, otherwise } => {
            for case in cases {
                visit(&case.times, f);
            }
            visit(otherwise, f);
        }
    }
}

/// Points every comparison block using `old` to `new` instead. Returns
/// whether anything changed.
fn rewrite(node: &mut Times, old: &str, new: &str) -> bool {
    use Times::*;
    match node {
        Comparison { name } => {
            let matches = same_name(name, old);
            if matches {
                *name = new.into();
            }
            matches
        }
        Attempt { .. }
        | BestRecentRun { .. }
        | Aggregate { .. }
        | Percentile { .. }
        | WeightedAverage { .. } => false,
        AddTime { source, .. }
        | Scale { source, .. }
        | StretchToTotal { source, .. }
        | Round { source, .. } => rewrite(source, old, new),
        Blend { from, to, .. } => rewrite(from, old, new) | rewrite(to, old, new),
        Pick { first, second, .. } => rewrite(first, old, new) | rewrite(second, old, new),
        Clamp { source, low, high } => {
            rewrite(source, old, new) | rewrite(low, old, new) | rewrite(high, old, new)
        }
        Fallback { primary, fallback } => rewrite(primary, old, new) | rewrite(fallback, old, new),
        SegmentRange { cases, otherwise } => {
            let mut changed = false;
            for case in cases {
                changed |= rewrite(&mut case.times, old, new);
            }
            changed | rewrite(otherwise, old, new)
        }
    }
}

/// Updates the names in the comparison blocks of the desktop block editor's
/// layout, like the desktop component does. Everything else stays.
fn rewrite_workspace(value: &mut Value, old: &str, new: &str) {
    match value {
        Value::Object(object) => {
            let is_comparison =
                object.get("type").and_then(Value::as_str) == Some(COMPARISON_BLOCK_TYPE);
            for (key, member) in object.iter_mut() {
                if let (true, "fields", Value::Object(fields)) =
                    (is_comparison, key.as_str(), &mut *member)
                {
                    for (field, value) in fields.iter_mut() {
                        if field == "NAME" && value.as_str().is_some_and(|n| same_name(n, old)) {
                            *value = new.into();
                        } else {
                            rewrite_workspace(value, old, new);
                        }
                    }
                } else {
                    rewrite_workspace(member, old, new);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                rewrite_workspace(item, old, new);
            }
        }
        _ => {}
    }
}
