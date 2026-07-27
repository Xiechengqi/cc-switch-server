//! Image input loader for Cursor AgentService.

use axum::http::StatusCode;
use bytes::Bytes;
use futures_util::{stream, StreamExt, TryStreamExt};
use rand::RngCore;

use super::agent_proto::EncodedImage;
use crate::proxy::ProxyError;

pub const MAX_IMAGE_BYTES: usize = crate::proxy::remote_image::MAX_IMAGE_BYTES;

/// Image references extracted from downstream request bodies.
#[derive(Debug, Clone)]
pub enum ImageRef {
    DataUri(String),
    HttpUrl(String),
    Inline { mime: String, data: Bytes },
}

pub async fn load_images(refs: Vec<ImageRef>) -> Result<Vec<EncodedImage>, ProxyError> {
    if refs.len() > crate::proxy::remote_image::MAX_IMAGES_PER_REQUEST {
        return Err(invalid_image(format!(
            "Cursor request exceeds {} image limit",
            crate::proxy::remote_image::MAX_IMAGES_PER_REQUEST
        )));
    }

    tokio::time::timeout(
        crate::proxy::remote_image::BATCH_FETCH_TIMEOUT,
        stream::iter(refs.into_iter().map(|reference| async move {
            match reference {
                ImageRef::DataUri(uri) => decode_data_uri(&uri),
                ImageRef::Inline { mime, data } => {
                    let mime = crate::proxy::remote_image::validate_image_bytes(
                        &data,
                        Some(&mime),
                        MAX_IMAGE_BYTES,
                    )?;
                    Ok(EncodedImage {
                        data,
                        mime_type: Some(mime),
                        width: None,
                        height: None,
                        uuid: random_uuid_like(),
                    })
                }
                ImageRef::HttpUrl(url) => fetch_http(&url).await,
            }
        }))
        .buffered(crate::proxy::remote_image::MAX_CONCURRENT_FETCHES)
        .try_collect::<Vec<_>>(),
    )
    .await
    .map_err(|_| ProxyError {
        status: StatusCode::BAD_GATEWAY,
        message: "Cursor image batch timed out".to_string(),
    })?
}

fn decode_data_uri(uri: &str) -> Result<EncodedImage, ProxyError> {
    let loaded = crate::proxy::remote_image::decode_image_data_uri(uri, MAX_IMAGE_BYTES)?;
    Ok(EncodedImage {
        data: loaded.data,
        mime_type: Some(loaded.mime_type),
        width: None,
        height: None,
        uuid: random_uuid_like(),
    })
}

async fn fetch_http(url: &str) -> Result<EncodedImage, ProxyError> {
    let loaded = crate::proxy::remote_image::fetch_remote_image(url, MAX_IMAGE_BYTES).await?;
    Ok(EncodedImage {
        data: loaded.data,
        mime_type: Some(loaded.mime_type),
        width: None,
        height: None,
        uuid: random_uuid_like(),
    })
}

fn invalid_image(message: impl Into<String>) -> ProxyError {
    ProxyError {
        status: StatusCode::BAD_REQUEST,
        message: message.into(),
    }
}

fn random_uuid_like() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_uri_decodes_base64_image() {
        let image = decode_data_uri("data:image/png;base64,iVBORw0KGgo=").unwrap();
        assert_eq!(image.data, Bytes::from_static(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(image.mime_type.as_deref(), Some("image/png"));
    }

    #[test]
    fn rejects_non_base64_data_uri() {
        let error = decode_data_uri("data:image/png,hello").unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn rejects_data_uri_with_spoofed_mime() {
        let error = decode_data_uri("data:image/jpeg;base64,iVBORw0KGgo=").unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rejects_excessive_image_count_before_decoding() {
        let refs = vec![
            ImageRef::DataUri("data:image/png;base64,iVBORw0KGgo=".to_string());
            crate::proxy::remote_image::MAX_IMAGES_PER_REQUEST + 1
        ];
        let error = load_images(refs).await.unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("image limit"));
    }
}
