use std::collections::BTreeMap;

use super::error::WireError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum HeaderValue {
    Bool(bool),
    Byte(i8),
    Short(i16),
    Integer(i32),
    Long(i64),
    ByteArray(Vec<u8>),
    String(String),
    Timestamp(i64),
    Uuid([u8; 16]),
}

impl HeaderValue {
    pub(super) fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Headers(BTreeMap<String, HeaderValue>);

impl Headers {
    pub(super) fn get(&self, name: &str) -> Option<&HeaderValue> {
        self.0.get(name)
    }

    pub(super) fn string(&self, name: &str) -> Option<&str> {
        self.get(name).and_then(HeaderValue::as_str)
    }

    pub(super) fn event_type(&self) -> Option<&str> {
        self.string(":event-type")
    }

    pub(super) fn message_type(&self) -> Option<&str> {
        self.string(":message-type")
    }

    pub(super) fn exception_type(&self) -> Option<&str> {
        self.string(":exception-type")
    }

    pub(super) fn error_code(&self) -> Option<&str> {
        self.string(":error-code")
    }
}

pub(super) fn parse_headers(bytes: &[u8]) -> Result<Headers, WireError> {
    let mut offset = 0usize;
    let mut headers = BTreeMap::new();
    while offset < bytes.len() {
        let name_length = take_u8(bytes, &mut offset)? as usize;
        if name_length == 0 {
            return Err(WireError::InvalidHeaderName);
        }
        let name_bytes = take(bytes, &mut offset, name_length, "header name")?;
        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| WireError::InvalidHeaderName)?
            .to_string();
        let value_type = take_u8(bytes, &mut offset)?;
        let value = match value_type {
            0 => HeaderValue::Bool(true),
            1 => HeaderValue::Bool(false),
            2 => HeaderValue::Byte(take_u8(bytes, &mut offset)? as i8),
            3 => HeaderValue::Short(i16::from_be_bytes(array::<2>(take(
                bytes,
                &mut offset,
                2,
                "short",
            )?))),
            4 => HeaderValue::Integer(i32::from_be_bytes(array::<4>(take(
                bytes,
                &mut offset,
                4,
                "integer",
            )?))),
            5 => HeaderValue::Long(i64::from_be_bytes(array::<8>(take(
                bytes,
                &mut offset,
                8,
                "long",
            )?))),
            6 => {
                let length = u16::from_be_bytes(array::<2>(take(
                    bytes,
                    &mut offset,
                    2,
                    "byte array length",
                )?)) as usize;
                HeaderValue::ByteArray(take(bytes, &mut offset, length, "byte array")?.to_vec())
            }
            7 => {
                let length =
                    u16::from_be_bytes(array::<2>(take(bytes, &mut offset, 2, "string length")?))
                        as usize;
                let value = std::str::from_utf8(take(bytes, &mut offset, length, "string")?)
                    .map_err(|_| WireError::InvalidHeaderValue("UTF-8 string"))?;
                HeaderValue::String(value.to_string())
            }
            8 => HeaderValue::Timestamp(i64::from_be_bytes(array::<8>(take(
                bytes,
                &mut offset,
                8,
                "timestamp",
            )?))),
            9 => HeaderValue::Uuid(array::<16>(take(bytes, &mut offset, 16, "UUID")?)),
            other => return Err(WireError::InvalidHeaderType(other)),
        };
        if headers.insert(name.clone(), value).is_some() {
            return Err(WireError::DuplicateHeader(name));
        }
    }
    Ok(Headers(headers))
}

fn take_u8(bytes: &[u8], offset: &mut usize) -> Result<u8, WireError> {
    Ok(take(bytes, offset, 1, "byte")?[0])
}

fn take<'a>(
    bytes: &'a [u8],
    offset: &mut usize,
    length: usize,
    kind: &'static str,
) -> Result<&'a [u8], WireError> {
    let end = offset
        .checked_add(length)
        .ok_or(WireError::InvalidHeaderValue(kind))?;
    let value = bytes
        .get(*offset..end)
        .ok_or(WireError::InvalidHeaderValue(kind))?;
    *offset = end;
    Ok(value)
}

fn array<const N: usize>(bytes: &[u8]) -> [u8; N] {
    bytes.try_into().expect("header slice length is validated")
}
