use super::Result;
use crate::{
    comparison::custom::settings,
    platform::prelude::*,
    util::xml::{
        Reader,
        helper::{end_tag, parse_base, parse_children, text, text_as_escaped_string_err},
    },
};

/// Reads the custom comparisons stored in a layout file of the original
/// LiveSplit by its Custom Comparisons component, and returns them as the
/// settings of the Custom Comparisons plugin (see
/// [`comparison::custom::settings`](crate::comparison::custom::settings)).
/// LiveSplit One calculates custom comparisons with a comparison plugin, not
/// with a layout component, so this is how they are taken over from such a
/// layout. Every comparison is kept as it is, even one that can't be read.
/// The comparisons of all the layout's Custom Comparisons components are
/// included.
pub fn custom_comparisons_settings(source: &str) -> Result<String> {
    let reader = &mut Reader::new(source);
    let mut comparisons = Vec::new();

    parse_base(reader, "Layout", |reader, _| {
        parse_children(reader, |reader, tag, _| -> Result<()> {
            if tag.name() != "Components" {
                return end_tag(reader);
            }
            parse_children(reader, |reader, _, _| -> Result<()> {
                let mut is_custom_comparisons = false;
                parse_children(reader, |reader, tag, _| -> Result<()> {
                    match tag.name() {
                        "Path" => text_as_escaped_string_err(reader, |path| -> Result<()> {
                            is_custom_comparisons = path == "LiveSplit.CustomComparisons.dll";
                            Ok(())
                        }),
                        // Assumption: Settings always has to come after the Path,
                        // as for every other component.
                        "Settings" if is_custom_comparisons => {
                            parse_children(reader, |reader, tag, _| -> Result<()> {
                                if tag.name() != "Comparisons" {
                                    return end_tag(reader);
                                }
                                parse_children(reader, |reader, tag, _| -> Result<()> {
                                    if tag.name() == "Comparison" {
                                        text(reader, |t| comparisons.push(t.into_owned()))
                                    } else {
                                        end_tag(reader)
                                    }
                                })
                            })
                        }
                        _ => end_tag(reader),
                    }
                })
            })
        })
    })?;

    Ok(settings::write(comparisons))
}
