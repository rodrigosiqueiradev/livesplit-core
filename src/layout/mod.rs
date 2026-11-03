//! The layout module provides everything necessary for working with a
//! [`Layout`]. A [`Layout`] allows you to combine multiple components together
//! to visualize a variety of information the runner is interested in.

mod component;
mod component_settings;
mod component_state;
pub mod editor;
mod general_settings;
mod layout_direction;
mod layout_settings;
mod layout_state;
pub mod parser;

pub use self::{
    component::Component, component_settings::ComponentSettings, component_state::ComponentState,
    editor::Editor, general_settings::GeneralSettings, layout_direction::LayoutDirection,
    layout_settings::LayoutSettings, layout_state::LayoutState,
};

use crate::{
    comparison::custom::{Library, Program},
    component::{previous_segment, splits, timer, title},
    localization::Lang,
    platform::prelude::*,
    settings::ImageCache,
    timing::Snapshot,
};

/// A Layout allows you to combine multiple components together to visualize a
/// variety of information the runner is interested in.
#[derive(Clone, Default)]
pub struct Layout {
    /// All of the layout's components.
    pub components: Vec<Component>,
    settings: GeneralSettings,
}

impl Layout {
    /// Creates a new empty layout with no components.
    pub fn new() -> Self {
        Default::default()
    }

    /// Creates a new default layout that contains a default set of components
    /// in order to provide a good default layout for runners. Which components
    /// are provided by this and how they are configured may change in the
    /// future.
    pub fn default_layout(lang: Lang) -> Self {
        Self {
            components: vec![
                title::Component::new().into(),
                splits::Component::new(lang).into(),
                timer::Component::new().into(),
                previous_segment::Component::new().into(),
            ],
            settings: GeneralSettings::default(),
        }
    }

    /// Creates a new layout from the layout settings of the whole layout.
    pub fn from_settings(layout_settings: LayoutSettings) -> Self {
        Self {
            components: layout_settings
                .components
                .into_iter()
                .map(Into::into)
                .collect(),
            settings: layout_settings.general,
        }
    }

    /// Accesses the general settings of the layout that apply to all
    /// components.
    pub const fn general_settings(&self) -> &GeneralSettings {
        &self.settings
    }

    /// Grants mutable access to the general settings of the layout that apply
    /// to all components.
    pub const fn general_settings_mut(&mut self) -> &mut GeneralSettings {
        &mut self.settings
    }

    /// Adds a new component to the end of the layout.
    pub fn push<C: Into<Component>>(&mut self, component: C) {
        self.components.push(component.into());
    }

    /// Updates the layout's state based on the timer provided. You can use this
    /// to visualize all of the components of a layout. The [`ImageCache`] is
    /// updated with all the images that are part of the state. The images are
    /// marked as visited in the [`ImageCache`]. You still need to manually run
    /// [`ImageCache::collect`] to ensure unused images are removed from the
    /// cache.
    pub fn update_state(
        &mut self,
        state: &mut LayoutState,
        image_cache: &mut ImageCache,
        timer: &Snapshot,
        lang: Lang,
    ) {
        let settings = &self.settings;

        state.components.truncate(self.components.len());
        let mut components = self.components.iter_mut();
        // First update all the states that we have.
        for (state, component) in state.components.iter_mut().zip(components.by_ref()) {
            component.update_state(state, image_cache, timer, settings, lang);
        }
        // Then add states for all the components that don't have states yet.
        state
            .components
            .extend(components.map(|c| c.state(image_cache, timer, settings, lang)));

        state.timer_font.clone_from(&settings.timer_font);
        state.times_font.clone_from(&settings.times_font);
        state.text_font.clone_from(&settings.text_font);

        state.background = settings.background.cache(image_cache);
        state.thin_separators_color = settings.thin_separators_color;
        state.separators_color = settings.separators_color;
        state.text_color = settings.text_color;
        state.direction = settings.direction;
        state.text_shadow = settings.text_shadow;
        state.selected_component = None;
    }

    /// Calculates the layout's state based on the timer provided. You can use
    /// this to visualize all of the components of a layout. The [`ImageCache`]
    /// is updated with all the images that are part of the state. The images
    /// are marked as visited in the [`ImageCache`]. You still need to manually
    /// run [`ImageCache::collect`] to ensure unused images are removed from the
    /// cache.
    pub fn state(
        &mut self,
        image_cache: &mut ImageCache,
        timer: &Snapshot,
        lang: Lang,
    ) -> LayoutState {
        let mut state = Default::default();
        self.update_state(&mut state, image_cache, timer, lang);
        state
    }

    /// Accesses the settings of the layout.
    pub fn settings(&self) -> LayoutSettings {
        LayoutSettings {
            components: self.components.iter().map(Component::settings).collect(),
            general: self.settings.clone(),
        }
    }

    /// The text of every custom comparison stored in the layout's Custom
    /// Comparisons components, including the ones inside groups and
    /// carousels and the ones that can't be read, as the settings of the
    /// Custom Comparisons plugin. This is how comparisons are taken over from
    /// a layout, like one made with the desktop LiveSplit.
    pub fn custom_comparisons_settings(&self) -> String {
        fn collect<'a>(components: &'a [Component], texts: &mut Vec<&'a str>) {
            for component in components {
                if let Component::CustomComparisons(component) = component {
                    texts.extend(component.settings().comparisons.iter().map(String::as_str));
                } else if let Some(children) = component.children() {
                    collect(children, texts);
                }
            }
        }

        let mut texts = Vec::new();
        collect(&self.components, &mut texts);
        crate::comparison::custom::settings::write(texts)
    }

    /// The custom comparisons stored in the layout's Custom Comparisons
    /// components, including the ones inside groups and carousels. Install
    /// them on the run with [`Timer::set_custom_comparisons`] whenever the
    /// layout or the run changes. If two comparisons share a name, the first
    /// one wins.
    ///
    /// [`Timer::set_custom_comparisons`]: crate::Timer::set_custom_comparisons
    pub fn custom_comparisons(&self) -> Library {
        fn collect(components: &[Component], programs: &mut Vec<Program>) {
            for component in components {
                if let Component::CustomComparisons(component) = component {
                    programs.extend(component.library().programs().iter().cloned());
                } else if let Some(children) = component.children() {
                    collect(children, programs);
                }
            }
        }

        let mut programs = Vec::new();
        collect(&self.components, &mut programs);
        Library::new(programs)
    }

    /// Scrolls up all the components in the layout that can be scrolled up.
    pub fn scroll_up(&mut self) {
        for component in &mut self.components {
            component.scroll_up();
        }
    }

    /// Scrolls down all the components in the layout that can be scrolled down.
    pub fn scroll_down(&mut self) {
        for component in &mut self.components {
            component.scroll_down();
        }
    }
}
