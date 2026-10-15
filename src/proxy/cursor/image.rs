//! Image input loader for Cursor AgentService.

use axum::http::StatusCode;
use bytes::Bytes;
use futures_util::{stream, StreamExt, TryStreamExt};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use rand::RngCore;

use super::agent_proto::EncodedImage;
use crate::proxy::request_memory::{RequestMemoryBudget, RequestMemoryComponent};
use crate::proxy::ProxyError;

pub const MAX_IMAGE_BYTES: usize = crate::proxy::remote_image::MAX_IMAGE_BYTES;
pub const MAX_INPUT_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_IMAGES: usize = 12;
const MAX_DECODE_EDGE: usize = 8192;
const MAX_DECODE_PIXELS: usize = 25_000_000;
const MAX_WIRE_EDGE: u32 = 2000;
const SOFT_WIRE_BYTES: usize = 100 * 1024;
const MIN_WIRE_EDGE: u32 = 256;

/// Image references extracted from downstream request bodies.
#[derive(Debug, Clone)]
pub enum ImageRef {
    DataUri(String),
    HttpUrl(String),
    Inline {
        mime: String,
        data: Bytes,
    },
    Detailed {
        source: Box<ImageRef>,
        high_detail: bool,
    },
}

pub async fn load_images(refs: Vec<ImageRef>) -> Result<Vec<EncodedImage>, ProxyError> {
    load_images_with_memory(refs, None).await
}

pub(crate) async fn load_images_with_memory(
    refs: Vec<ImageRef>,
    request_memory: Option<&RequestMemoryBudget>,
) -> Result<Vec<EncodedImage>, ProxyError> {
    if refs.len() > MAX_IMAGES {
        return Err(invalid_image(format!(
            "Cursor request exceeds {} image limit",
            MAX_IMAGES
        )));
    }

    let budget = request_memory.cloned();
    let mut images = tokio::time::timeout(
        crate::proxy::remote_image::BATCH_FETCH_TIMEOUT,
        stream::iter(refs.into_iter().map(|reference| {
            let budget = budget.clone();
            async move {
                let (reference, high_detail) = match reference {
                    ImageRef::Detailed {
                        source,
                        high_detail,
                    } => (*source, high_detail),
                    other => (other, false),
                };
                let loaded = match reference {
                    ImageRef::DataUri(uri) => decode_data_uri(&uri),
                    ImageRef::Inline { mime, data } => {
                        let mime = crate::proxy::remote_image::validate_image_bytes(
                            &data,
                            Some(&mime),
                            MAX_INPUT_IMAGE_BYTES,
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
                    ImageRef::Detailed { .. } => {
                        return Err(invalid_image("Cursor image detail wrapper is nested"));
                    }
                }?;
                prepare_image_for_wire(loaded, budget, high_detail).await
            }
        }))
        .buffered(crate::proxy::remote_image::MAX_CONCURRENT_FETCHES)
        .try_collect::<Vec<_>>(),
    )
    .await
    .map_err(|_| ProxyError {
        status: StatusCode::BAD_GATEWAY,
        message: "Cursor image batch timed out".to_string(),
    })??;
    if let Some(budget) = request_memory {
        for image in &mut images {
            image.data = budget
                .retain_bytes(RequestMemoryComponent::DecodedBody, image.data.clone())
                .map_err(|error| error.into_proxy_error())?;
        }
    }
    Ok(images)
}

fn decode_data_uri(uri: &str) -> Result<EncodedImage, ProxyError> {
    let loaded = crate::proxy::remote_image::decode_image_data_uri(uri, MAX_INPUT_IMAGE_BYTES)?;
    Ok(EncodedImage {
        data: loaded.data,
        mime_type: Some(loaded.mime_type),
        width: None,
        height: None,
        uuid: random_uuid_like(),
    })
}

async fn fetch_http(url: &str) -> Result<EncodedImage, ProxyError> {
    let loaded = crate::proxy::remote_image::fetch_remote_image(url, MAX_INPUT_IMAGE_BYTES).await?;
    Ok(EncodedImage {
        data: loaded.data,
        mime_type: Some(loaded.mime_type),
        width: None,
        height: None,
        uuid: random_uuid_like(),
    })
}

async fn prepare_image_for_wire(
    image: EncodedImage,
    budget: Option<RequestMemoryBudget>,
    high_detail: bool,
) -> Result<EncodedImage, ProxyError> {
    tokio::task::spawn_blocking(move || prepare_image_for_wire_sync(image, budget, high_detail))
        .await
        .map_err(|_| ProxyError::bad_gateway("Cursor image preparation stopped"))?
}

fn prepare_image_for_wire_sync(
    mut image: EncodedImage,
    budget: Option<RequestMemoryBudget>,
    high_detail: bool,
) -> Result<EncodedImage, ProxyError> {
    let mime = image.mime_type.as_deref().unwrap_or_default();
    if !matches!(
        mime,
        "image/jpeg" | "image/png" | "image/webp" | "image/gif"
    ) {
        return Err(invalid_image("Cursor image input type is unsupported"));
    }
    let size = imagesize::blob_size(&image.data)
        .map_err(|_| invalid_image("Cursor image dimensions could not be read"))?;
    let pixels = size.width.saturating_mul(size.height);
    if size.width == 0
        || size.height == 0
        || size.width.max(size.height) > MAX_DECODE_EDGE
        || pixels > MAX_DECODE_PIXELS
    {
        return Err(invalid_image("Cursor image dimensions are too large"));
    }

    let initial_edge = size.width.max(size.height) as u32;
    let first_target_pixels = if initial_edge > MAX_WIRE_EDGE {
        (MAX_WIRE_EDGE as usize).saturating_mul(MAX_WIRE_EDGE as usize)
    } else {
        pixels
    };
    let working_bytes = image
        .data
        .len()
        .saturating_add(pixels.saturating_mul(8))
        .saturating_add(first_target_pixels.saturating_mul(8))
        .saturating_add(MAX_IMAGE_BYTES.saturating_mul(2));
    let _working = budget
        .as_ref()
        .map(|budget| budget.reserve(RequestMemoryComponent::DecodedBody, working_bytes))
        .transpose()
        .map_err(|error| error.into_proxy_error())?;
    let source = image::load_from_memory(&image.data)
        .map_err(|_| invalid_image("Cursor image input is undecodable or unsupported"))?;
    let soft_wire_bytes = if high_detail {
        256 * 1024
    } else {
        SOFT_WIRE_BYTES
    };
    let qualities: &[u8] = if high_detail {
        &[90, 80, 65, 50]
    } else {
        &[85, 70, 55, 40]
    };
    if mime == "image/jpeg" && image.data.len() <= soft_wire_bytes {
        image.width = Some(size.width as u32);
        image.height = Some(size.height as u32);
        return Ok(image);
    }

    let mut edge = initial_edge.min(MAX_WIRE_EDGE);
    let mut best: Option<(Vec<u8>, u32, u32)> = None;
    loop {
        let scale = edge as f64 / initial_edge as f64;
        let width = ((size.width as f64 * scale).round() as u32).max(1);
        let height = ((size.height as f64 * scale).round() as u32).max(1);
        let rgb = source
            .resize_exact(width, height, FilterType::Triangle)
            .to_rgb8();
        for &quality in qualities {
            let mut encoded = Vec::new();
            JpegEncoder::new_with_quality(&mut encoded, quality)
                .encode(rgb.as_raw(), width, height, image::ExtendedColorType::Rgb8)
                .map_err(|_| invalid_image("Cursor image could not be prepared"))?;
            let soft_capped = encoded.len() <= soft_wire_bytes;
            if best
                .as_ref()
                .is_none_or(|(previous, _, _)| encoded.len() < previous.len())
            {
                best = Some((encoded, width, height));
            }
            if soft_capped {
                break;
            }
        }
        if best
            .as_ref()
            .is_some_and(|(bytes, _, _)| bytes.len() <= soft_wire_bytes)
            || edge <= MIN_WIRE_EDGE
        {
            break;
        }
        let next = ((edge as f64 * 0.85).round() as u32).max(MIN_WIRE_EDGE);
        if next == edge {
            break;
        }
        edge = next;
    }
    let (encoded, width, height) =
        best.ok_or_else(|| invalid_image("Cursor image could not be prepared"))?;
    if encoded.len() > MAX_IMAGE_BYTES {
        return Err(invalid_image("Cursor image exceeds 1 MiB wire limit"));
    }
    image.data = Bytes::from(encoded);
    image.mime_type = Some("image/jpeg".to_string());
    image.width = Some(width);
    image.height = Some(height);
    Ok(image)
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
            MAX_IMAGES + 1
        ];
        let error = load_images(refs).await.unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(error.message.contains("image limit"));
    }

    #[tokio::test]
    async fn oversized_valid_png_is_prepared_under_the_cursor_wire_limit() {
        let mut pixels = image::RgbImage::new(1024, 1024);
        let mut seed = 0x1234_5678_u32;
        for pixel in pixels.pixels_mut() {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *pixel = image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8]);
        }
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(pixels)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let png = encoded.into_inner();
        assert!(png.len() > MAX_IMAGE_BYTES);
        let images = load_images(vec![ImageRef::Inline {
            mime: "image/png".to_string(),
            data: Bytes::from(png),
        }])
        .await
        .unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime_type.as_deref(), Some("image/jpeg"));
        assert!(images[0].data.len() <= MAX_IMAGE_BYTES);
        assert_eq!(images[0].width, images[0].height);
        assert!(images[0]
            .width
            .is_some_and(|width| (256..=1024).contains(&width)));
        assert!(images[0].data.len() <= SOFT_WIRE_BYTES);
    }

    #[tokio::test]
    async fn malformed_or_decode_bomb_images_fail_before_cursor_wire_encoding() {
        let mut image = image::RgbImage::new(2, 2);
        image.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let png = encoded.into_inner();

        let mut bomb = png.clone();
        bomb[16..20].copy_from_slice(&9000_u32.to_be_bytes());
        let error = load_images(vec![ImageRef::Inline {
            mime: "image/png".to_string(),
            data: Bytes::from(bomb),
        }])
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.message, "Cursor image dimensions are too large");

        let truncated = png[..png.len() - 12].to_vec();
        let error = load_images(vec![ImageRef::Inline {
            mime: "image/png".to_string(),
            data: Bytes::from(truncated),
        }])
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert!(!error.message.contains("IHDR"));
    }

    #[tokio::test]
    async fn image_decode_working_set_obeys_the_request_budget() {
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::new(512, 512))
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let budget = RequestMemoryBudget::new(1024 * 1024);
        let error = load_images_with_memory(
            vec![ImageRef::Inline {
                mime: "image/png".to_string(),
                data: Bytes::from(encoded.into_inner()),
            }],
            Some(&budget),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(budget.is_exhausted());
    }

    #[tokio::test]
    async fn high_detail_uses_a_larger_visual_target_than_default() {
        let mut pixels = image::RgbImage::new(512, 512);
        let mut seed = 0x8765_4321_u32;
        for pixel in pixels.pixels_mut() {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *pixel = image::Rgb([(seed >> 16) as u8, (seed >> 8) as u8, seed as u8]);
        }
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(pixels)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let source = ImageRef::Inline {
            mime: "image/png".to_string(),
            data: Bytes::from(encoded.into_inner()),
        };
        let default = load_images(vec![source.clone()]).await.unwrap();
        let high = load_images(vec![ImageRef::Detailed {
            source: Box::new(source),
            high_detail: true,
        }])
        .await
        .unwrap();
        assert!(high[0].data.len() <= 256 * 1024);
        assert!(high[0].width.unwrap() >= default[0].width.unwrap());
        assert!(high[0].data.len() >= default[0].data.len());
    }
}
