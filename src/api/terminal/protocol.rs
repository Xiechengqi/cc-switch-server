use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(tag = "t")]
pub(crate) enum ServerMessage {
    #[serde(rename = "rb")]
    ReplayBegin,
    #[serde(rename = "re")]
    ReplayEnd,
    #[serde(rename = "out")]
    Output { d: String },
    #[serde(rename = "exit")]
    Exit {
        #[serde(skip_serializing_if = "Option::is_none")]
        c: Option<i32>,
    },
}

impl ServerMessage {
    pub(crate) fn output_bytes(data: &[u8]) -> Self {
        Self::Output {
            d: B64.encode(data),
        }
    }

    pub(crate) fn to_text(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

pub(crate) fn decode_input_payload(encoded: &str) -> Result<Vec<u8>, String> {
    B64.decode(encoded.as_bytes())
        .map_err(|error| format!("invalid input encoding: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{decode_input_payload, ServerMessage};

    #[test]
    fn encodes_output_and_decodes_input() {
        let encoded = ServerMessage::output_bytes(b"hi").to_text().unwrap();
        assert!(encoded.contains("out"));
        assert_eq!(decode_input_payload("aGk=").unwrap(), b"hi");
    }
}
