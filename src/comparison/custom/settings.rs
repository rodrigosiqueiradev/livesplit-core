//! The settings of the Custom Comparisons plugin: the list of custom
//! comparisons a runner keeps, which is also the file they import and export.
//!
//! ```json
//! { "comparisons": [ { "format": "livesplit-custom-comparison", ... }, ... ] }
//! ```
//!
//! Each entry is a comparison in the custom comparison format
//! ([`format`](super::format)).
//! A comparison that isn't even valid JSON is kept as a string, so nothing
//! the runner made is ever lost.

use crate::platform::prelude::*;
use serde_json::{Map, Value};

/// The id of the Custom Comparisons plugin.
pub const PLUGIN_ID: &str = "livesplit.custom-comparisons";

/// Reads the settings into the text of each comparison, in order. Empty
/// settings are an empty list.
pub fn read(settings: &str) -> Result<Vec<String>, String> {
    if settings.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_str(settings)
        .map_err(|e| format!("The custom comparisons couldn't be read: {e}"))?;
    let comparisons = match value {
        Value::Object(mut object) => object.remove("comparisons").unwrap_or(Value::Null),
        _ => Value::Null,
    };
    let Value::Array(comparisons) = comparisons else {
        return Err(
            "The custom comparisons couldn't be read: there is no list of comparisons.".into(),
        );
    };
    Ok(comparisons
        .into_iter()
        .map(|c| match c {
            Value::String(text) => text,
            other => other.to_string(),
        })
        .collect())
}

/// Writes the text of each comparison as settings.
pub fn write(comparisons: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let comparisons = comparisons
        .into_iter()
        .map(|text| {
            let text = text.as_ref();
            match serde_json::from_str::<Value>(text) {
                Ok(value @ Value::Object(_)) => value,
                _ => Value::String(text.into()),
            }
        })
        .collect();
    let mut object = Map::new();
    object.insert("comparisons".into(), Value::Array(comparisons));
    Value::Object(object).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_keeps_broken_comparisons() {
        let texts = [r#"{"name":"A","version":1}"#, "not json {"];
        let settings = write(texts);
        assert_eq!(
            settings,
            r#"{"comparisons":[{"name":"A","version":1},"not json {"]}"#
        );
        assert_eq!(read(&settings).unwrap(), texts);
    }

    #[test]
    fn empty_and_bad_settings() {
        assert!(read("  ").unwrap().is_empty());
        assert!(read("[]").is_err());
        assert!(read("{ nope").is_err());
    }
}
