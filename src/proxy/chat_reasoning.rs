//! Plain Chat reasoning aliases only. Each delta contributes at most one text.
//! Priority preserves the existing Server bridge: content, text, then reasoning.
//! Blank aliases cannot hide nonblank text; a whitespace-only fragment retains
//! its exact bytes when no other alias carries nonblank content.
use serde_json::{Map, Value};

pub(super) const ALIASES: [&str; 3] = ["reasoning_content", "reasoning_text", "reasoning"];

pub(super) fn select_fields(payload: &Map<String, Value>) -> Option<(&'static str, &str)> {
    let mut whitespace = None;
    for key in ALIASES {
        let Some(text) = payload
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        else {
            continue;
        };
        if !text.trim().is_empty() {
            return Some((key, text));
        }
        whitespace.get_or_insert((key, text));
    }
    whitespace
}

pub(super) fn text(payload: &Value) -> Option<&str> {
    select_fields(payload.as_object()?).map(|(_, text)| text)
}
