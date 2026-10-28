//! Non-authoritative observations of Grok media output. SSE inspection borrows
//! raw JSON fields instead of building another tree containing image/base64
//! payloads. Existing JSON paths pass the value they have already decoded.

use serde::de::{DeserializeSeed, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::{value::RawValue, Value};

use super::request_memory::RequestMemoryBudget;
use super::request_timing::{ForwardTiming, Milestone, TimingResult};

#[derive(Default)]
struct Observation {
    content: bool,
    failed: bool,
    complete: bool,
}

impl Observation {
    fn merge(&mut self, other: Self) {
        self.content |= other.content;
        self.failed |= other.failed;
        self.complete |= other.complete;
    }

    fn apply(self, timing: &ForwardTiming) {
        if self.failed {
            timing.observe_terminal(TimingResult::Failed);
        } else {
            if self.content {
                timing.mark(Milestone::FirstSemantic);
            }
            if self.complete {
                timing.observe_media_eof();
            }
        }
    }
}

#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(borrow)]
    url: Option<&'a RawValue>,
    b64_json: Option<&'a RawValue>,
    result: Option<&'a RawValue>,
    data: Option<&'a RawValue>,
    images: Option<&'a RawValue>,
    image: Option<&'a RawValue>,
    video: Option<&'a RawValue>,
    error: Option<&'a RawValue>,
    status: Option<&'a RawValue>,
    r#type: Option<&'a RawValue>,
}

struct MediaArray(usize);

impl<'de> DeserializeSeed<'de> for MediaArray {
    type Value = Observation;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for MediaArray {
    type Value = Observation;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an array of media results")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Self::Value, A::Error> {
        let mut observation = Observation::default();
        while let Some(raw) = items.next_element::<&RawValue>()? {
            observation.merge(raw_observation(raw.get(), self.0));
        }
        Ok(observation)
    }
}

struct StringPredicate(fn(&str) -> bool);

impl<'de> DeserializeSeed<'de> for StringPredicate {
    type Value = bool;

    fn deserialize<D: serde::Deserializer<'de>>(self, decoder: D) -> Result<bool, D::Error> {
        decoder.deserialize_str(self)
    }
}

impl Visitor<'_> for StringPredicate {
    type Value = bool;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a media string")
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<bool, E> {
        Ok((self.0)(value))
    }
}

fn raw_string(raw: Option<&RawValue>, predicate: fn(&str) -> bool) -> bool {
    raw.is_some_and(|raw| {
        StringPredicate(predicate)
            .deserialize(&mut serde_json::Deserializer::from_str(raw.get()))
            .unwrap_or(false)
    })
}

fn raw_error(raw: &RawValue) -> bool {
    let raw_text = raw.get().trim();
    match raw_text.as_bytes().first() {
        Some(b'n' | b'f') => false,
        Some(b'"') => raw_string(Some(raw), |text| !text.trim().is_empty()),
        Some(b'{') => !raw_text[1..].trim_start().starts_with('}'),
        Some(b'[') => !raw_text[1..].trim_start().starts_with(']'),
        _ => true,
    }
}

fn result_status(status: Option<&str>, kind: Option<&str>) -> (bool, bool) {
    let failed = matches!(status, Some("failed" | "error" | "cancelled" | "canceled"))
        || kind.is_some_and(|kind| kind == "error" || kind.ends_with(".failed"));
    let complete = matches!(
        status,
        Some("completed" | "succeeded" | "success" | "ready" | "done")
    ) || kind.is_some_and(|kind| kind.ends_with(".completed"));
    (failed, complete)
}

fn raw_observation(raw: &str, depth: usize) -> Observation {
    // Only reviewed media envelopes are traversed, with no result-sized Vec or
    // Value allocation. Unknown fields (including progress) stay unobserved.
    if depth > 4 {
        return Observation::default();
    }
    if raw.starts_with('[') {
        return MediaArray(depth + 1)
            .deserialize(&mut serde_json::Deserializer::from_str(raw))
            .unwrap_or_default();
    }
    let Ok(envelope) = serde_json::from_str::<Envelope<'_>>(raw) else {
        return Observation::default();
    };
    let failed = raw_string(envelope.status, |status| {
        result_status(Some(status), None).0
    }) || raw_string(envelope.r#type, |kind| result_status(None, Some(kind)).0);
    let complete = raw_string(envelope.status, |status| {
        result_status(Some(status), None).1
    }) || raw_string(envelope.r#type, |kind| result_status(None, Some(kind)).1);
    let mut observation = Observation {
        content: [envelope.url, envelope.b64_json, envelope.result]
            .into_iter()
            .flatten()
            .any(|raw| raw_string(Some(raw), |text| !text.trim().is_empty())),
        failed: failed || envelope.error.is_some_and(raw_error),
        complete,
    };
    for raw in [
        envelope.data,
        envelope.images,
        envelope.image,
        envelope.video,
        envelope.result,
    ]
    .into_iter()
    .flatten()
    {
        observation.merge(raw_observation(raw.get(), depth + 1));
    }
    observation
}

pub(super) fn observe_value(timing: &ForwardTiming, value: &Value) {
    fn observation(value: &Value, depth: usize) -> Observation {
        if depth > 4 {
            return Observation::default();
        }
        if let Some(items) = value.as_array() {
            let mut result = Observation::default();
            for item in items {
                result.merge(observation(item, depth + 1));
            }
            return result;
        }
        let (failed, complete) = result_status(
            value.get("status").and_then(Value::as_str),
            value.get("type").and_then(Value::as_str),
        );
        let mut result = Observation {
            content: ["url", "b64_json", "result"].into_iter().any(|field| {
                value
                    .get(field)
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())
            }),
            failed: failed
                || value
                    .get("error")
                    .is_some_and(super::response_semantics::error_value_is_substantive),
            complete,
        };
        for field in ["data", "images", "image", "video", "result"] {
            if let Some(child) = value.get(field) {
                result.merge(observation(child, depth + 1));
            }
        }
        result
    }
    observation(value, 0).apply(timing);
}

pub(super) fn observe_sse(timing: &ForwardTiming, frame: &[u8], budget: &RequestMemoryBudget) {
    let Ok(frame) = std::str::from_utf8(frame) else {
        return;
    };
    let mut lines = frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start));
    let Some(first) = lines.next() else { return };
    let second = lines.next();
    if second.is_none() && first.trim() == "[DONE]" {
        timing.observe_media_eof();
        return;
    }
    // Charge parser scratch (escaped strings/keys) and, for multiline SSE,
    // one payload join. Refusal cannot exhaust the forwarding budget.
    let Some(_scratch) =
        budget.reserve_observation_scratch(frame.len().saturating_mul(if second.is_some() {
            2
        } else {
            1
        }))
    else {
        return;
    };
    let Some(second) = second else {
        raw_observation(first, 0).apply(timing);
        return;
    };
    let mut payload = String::with_capacity(frame.len());
    payload.push_str(first);
    payload.push('\n');
    payload.push_str(second);
    for line in lines {
        payload.push('\n');
        payload.push_str(line);
    }
    raw_observation(&payload, 0).apply(timing);
}
