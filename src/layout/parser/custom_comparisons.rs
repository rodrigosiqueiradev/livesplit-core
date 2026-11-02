use super::Result;
use crate::util::xml::{
    Reader,
    helper::{end_tag, parse_children, text},
};

pub use crate::component::custom_comparisons::Component;

pub fn settings(reader: &mut Reader, component: &mut Component) -> Result<()> {
    let settings = component.settings_mut();

    parse_children(reader, |reader, tag, _| {
        if tag.name() == "Comparisons" {
            parse_children(reader, |reader, tag, _| {
                if tag.name() == "Comparison" {
                    text(reader, |t| settings.comparisons.push(t.into_owned()))
                } else {
                    end_tag(reader)
                }
            })
        } else {
            // FIXME:
            // Version
            end_tag(reader)
        }
    })
}
