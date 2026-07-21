use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub(crate) enum ClientMessage {
    #[serde(rename = "in")]
    Input {
        d: String,
    },
    #[serde(rename = "rs")]
    Resize {
        c: u16,
        r: u16,
    },
    Ping,
}

#[derive(Debug, Serialize)]
#[serde(tag = "t")]
pub(crate) enum ServerMessage {
    #[serde(rename = "rb")]
    ReplayBegin,
    #[serde(rename = "re")]
    ReplayEnd,
    #[serde(rename = "out")]
    Output { d: String },
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "exit")]
    Exit {
        #[serde(skip_serializing_if = "Option::is_none")]
        c: Option<i32>,
    },
    #[serde(rename = "err")]
    Error { m: String },
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

pub(crate) fn decode_client_message(text: &str) -> Result<ClientMessage, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("invalid json: {error}"))?;
    let kind = value
        .get("t")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing t".to_string())?;
    match kind {
        "in" => {
            let data = value
                .get("d")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing d".to_string())?;
            Ok(ClientMessage::Input {
                d: data.to_string(),
            })
        }
        "rs" => {
            let cols = value
                .get("c")
                .and_then(Value::as_u64)
                .ok_or_else(|| "missing c".to_string())?;
            let rows = value
                .get("r")
                .and_then(Value::as_u64)
                .ok_or_else(|| "missing r".to_string())?;
            Ok(ClientMessage::Resize {
                c: cols.min(u16::MAX as u64) as u16,
                r: rows.min(u16::MAX as u64) as u16,
            })
        }
        "ping" => Ok(ClientMessage::Ping),
        other => Err(format!("unknown message type {other}")),
    }
}

pub(crate) fn decode_input_payload(encoded: &str) -> Result<Vec<u8>, String> {
    B64.decode(encoded.as_bytes())
        .map_err(|error| format!("invalid input encoding: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{decode_client_message, decode_input_payload, ClientMessage, ServerMessage};

    #[test]
    fn decodes_resize_and_input() {
        let resize = decode_client_message(r#"{"t":"rs","c":120,"r":40}"#).unwrap();
        assert!(matches!(resize, ClientMessage::Resize { c: 120, r: 40 }));
        let encoded = ServerMessage::output_bytes(b"hi").to_text().unwrap();
        assert!(encoded.contains("out"));
        let input = decode_client_message(r#"{"t":"in","d":"aGk="}"#).unwrap();
        match input {
            ClientMessage::Input { d } => {
                assert_eq!(decode_input_payload(&d).unwrap(), b"hi");
            }
            _ => panic!("expected input"),
        }
    }
}
