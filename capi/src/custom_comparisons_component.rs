//! The Custom Comparisons Component doesn't show anything. It stores the
//! runner's custom comparisons in the layout, the same way the desktop
//! LiveSplit component Custom Comparisons does, so they travel with the
//! layout. Edit them with the Custom Comparisons Editor and install them on
//! the timer with Timer_set_custom_comparisons whenever the layout or the run
//! changes.

use crate::component::OwnedComponent;
use livesplit_core::component::custom_comparisons::Component as CustomComparisonsComponent;

/// type
pub type OwnedCustomComparisonsComponent = Box<CustomComparisonsComponent>;

/// Creates a new Custom Comparisons Component without any comparisons.
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsComponent_new() -> OwnedCustomComparisonsComponent {
    Box::new(CustomComparisonsComponent::new())
}

/// drop
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsComponent_drop(this: OwnedCustomComparisonsComponent) {
    drop(this);
}

/// Converts the component into a generic component suitable for using with a
/// layout.
#[unsafe(no_mangle)]
pub extern "C" fn CustomComparisonsComponent_into_generic(
    this: OwnedCustomComparisonsComponent,
) -> OwnedComponent {
    Box::new((*this).into())
}
