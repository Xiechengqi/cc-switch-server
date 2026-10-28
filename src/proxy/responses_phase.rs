//! Responses phases have no standard Chat/Messages equivalent. Capture their
//! item identity for diagnostics, but never turn public commentary into signed
//! reasoning or invent a marker that would need a private replay protocol.
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MessagePhase {
    Unspecified,
    Commentary,
    FinalAnswer,
    Unrecognized,
}

impl MessagePhase {
    fn from_item(item: &Value) -> Self {
        match item.get("phase") {
            None | Some(Value::Null) => Self::Unspecified,
            Some(Value::String(value)) if value == "commentary" => Self::Commentary,
            Some(Value::String(value)) if value == "final_answer" => Self::FinalAnswer,
            _ => Self::Unrecognized,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Commentary => "commentary",
            Self::FinalAnswer => "final_answer",
            Self::Unrecognized => "unrecognized",
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct MessagePhases {
    items: BTreeMap<String, MessagePhase>,
    reported: u8,
}

impl MessagePhases {
    pub(super) fn observe(
        &mut self,
        item: &Value,
        keys: &BTreeSet<String>,
        target: &'static str,
        direction: &'static str,
    ) {
        if item.get("type").and_then(Value::as_str) != Some("message") {
            return;
        }
        let incoming = MessagePhase::from_item(item);
        let known = keys
            .iter()
            .filter_map(|key| self.items.get(key))
            .copied()
            .filter(|phase| *phase != MessagePhase::Unspecified)
            .reduce(|left, right| {
                if left == right {
                    left
                } else {
                    MessagePhase::Unrecognized
                }
            });
        let phase = match (known, incoming) {
            (Some(known), MessagePhase::Unspecified) => known,
            (Some(known), incoming) if known != incoming => MessagePhase::Unrecognized,
            (_, incoming) => incoming,
        };
        for key in keys {
            self.items.insert(key.clone(), phase);
        }
        if phase != MessagePhase::Unspecified {
            let bit = 1 << phase as u8;
            if self.reported & bit == 0 {
                self.reported |= bit;
                crate::metrics::record_responses_phase_downgrade(target, direction, phase.label());
            }
        }
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.items.keys().fold(0usize, |bytes, key| {
            bytes
                .saturating_add(key.capacity())
                .saturating_add(std::mem::size_of::<(String, MessagePhase)>())
                .saturating_add(std::mem::size_of::<usize>() * 3)
        })
    }

    #[cfg(test)]
    pub(super) fn for_key(&self, key: &str) -> Option<MessagePhase> {
        self.items.get(key).copied()
    }
}

pub(super) fn observe_items(
    input: &Value,
    field: &str,
    target: &'static str,
    direction: &'static str,
) {
    let mut phases = MessagePhases::default();
    let no_keys = BTreeSet::new();
    match input.get(field) {
        Some(Value::Array(items)) => {
            for item in items {
                phases.observe(item, &no_keys, target, direction);
            }
        }
        Some(item @ Value::Object(_)) => phases.observe(item, &no_keys, target, direction),
        _ => {}
    }
}
