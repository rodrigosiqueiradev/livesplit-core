//! Provides the Custom Comparisons Component and relevant types for using it.
//! The Custom Comparisons Component doesn't show anything. It stores the
//! runner's [custom comparisons](crate::comparison::custom) in the layout, the
//! same way the desktop LiveSplit component *Custom Comparisons* does, so they
//! travel with the layout.
//!
//! Since the layout and the run are separate, the comparisons only show up in
//! the run once they are installed on it with
//! [`Timer::set_custom_comparisons`](crate::Timer::set_custom_comparisons),
//! usually with [`Layout::custom_comparisons`](crate::Layout::custom_comparisons)
//! whenever the layout or the run changes.

use crate::{
    comparison::custom::{self, Document, FormatError, Library},
    localization::{Lang, Text},
    platform::prelude::*,
    settings::{Gradient, SettingsDescription, Value},
};
use serde_derive::{Deserialize, Serialize};

use super::blank_space;

/// The Custom Comparisons Component doesn't show anything. It stores the
/// runner's custom comparisons in the layout.
#[derive(Default, Clone)]
pub struct Component {
    settings: Settings,
}

/// The Settings for this component.
#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The custom comparisons, each one in the JSON format shared with the
    /// desktop LiveSplit component. They are kept as text, so that a
    /// comparison that can't be read, for example because it was made by a
    /// newer version, isn't lost when the layout is saved again.
    pub comparisons: Vec<String>,
}

impl Component {
    /// Creates a new Custom Comparisons Component without any comparisons.
    pub fn new() -> Self {
        Default::default()
    }

    /// Creates a new Custom Comparisons Component with the given settings.
    pub const fn with_settings(settings: Settings) -> Self {
        Self { settings }
    }

    /// Accesses the settings of the component.
    pub const fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Grants mutable access to the settings of the component.
    pub const fn settings_mut(&mut self) -> &mut Settings {
        &mut self.settings
    }

    /// Accesses the name of the component for the specified language.
    pub const fn name(&self, lang: Lang) -> &'static str {
        Text::ComponentCustomComparisons.resolve(lang)
    }

    /// Reads the comparisons, in order. A comparison that can't be read is
    /// returned as an error and stays in the settings as it is.
    pub fn documents(&self) -> impl Iterator<Item = Result<Document, FormatError>> + '_ {
        self.settings
            .comparisons
            .iter()
            .map(|c| custom::format::read(c))
    }

    /// The comparisons that can be read, as a library. Comparisons that can't
    /// be read are left out.
    pub fn library(&self) -> Library {
        Library::new(self.documents().filter_map(|d| Some(d.ok()?.program)))
    }

    /// Updates the component's state. The component doesn't show anything,
    /// so it's an empty space without any size.
    pub const fn update_state(&self, state: &mut blank_space::State) {
        state.background = Gradient::Transparent;
        state.size = 0;
    }

    /// Calculates the component's state. The component doesn't show anything,
    /// so it's an empty space without any size.
    pub const fn state(&self) -> blank_space::State {
        blank_space::State {
            background: Gradient::Transparent,
            size: 0,
        }
    }

    /// Accesses a generic description of the settings available for this
    /// component and their current values for the specified language. The
    /// comparisons can't be described by generic settings, so there are none.
    pub fn settings_description(&self, _lang: Lang) -> SettingsDescription {
        SettingsDescription::default()
    }

    /// Sets a setting's value by its index to the given value.
    ///
    /// # Panics
    ///
    /// This panics if the type of the value to be set is not compatible with
    /// the type of the setting's value. A panic can also occur if the index of
    /// the setting provided is out of bounds.
    pub fn set_value(&mut self, _index: usize, _value: Value) {}
}
