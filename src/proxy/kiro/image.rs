use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::ImageDecoder;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::Semaphore;

use crate::proxy::request_memory::{
    retained_json_bytes, RequestMemoryBudget, RequestMemoryComponent, RequestMemoryReservation,
};
use crate::proxy::ProxyError;

const MAX_IMAGES: usize = 20;
const MAX_ENCODED_INPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_TOTAL_OUTPUT_BYTES: usize = 20 * 1024 * 1024;
#[cfg(test)]
const RESIZE_THRESHOLD_BYTES: usize = 400_000; // Historical decoded-byte threshold.
const MAX_WIRE_BASE64_CHARS: usize = 400_000; // Local policy, not an official vendor limit.
const MAX_LONG_SIDE: u32 = 1568;
const MAX_DECODE_LONG_SIDE: usize = 16_384;
const MAX_DECODE_PIXELS: usize = 64 * 1024 * 1024;
const JPEG_QUALITY: u8 = 85;
const JPEG_QUALITIES: [u8; 4] = [JPEG_QUALITY, 65, 45, 35];
const MIN_LONG_SIDE: usize = 256;
const MAX_ENCODE_ATTEMPTS_PER_IMAGE: usize = 24;
const MAX_ENCODE_ATTEMPTS_PER_REQUEST: usize = 64;
const MAX_PIXEL_WORK_PER_REQUEST: usize = 256 * 1024 * 1024;
const MAX_BLOCKING_JOBS: usize = 2;
static IMAGE_JOBS: OnceLock<Arc<Semaphore>> = OnceLock::new();

#[derive(Default)]
struct ImageWork {
    cancelled: Option<Arc<AtomicBool>>,
    pixels: usize,
    encodes: usize,
    // Processed fields stay charged until the enclosing body reservation has
    // adopted them, including when later images expand from compact inputs.
    retained_outputs: Vec<RequestMemoryReservation>,
}

impl ImageWork {
    fn checkpoint(&self) -> Result<(), ProxyError> {
        if self
            .cancelled
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
        {
            return Err(ProxyError::conflict("Kiro image preparation cancelled"));
        }
        Ok(())
    }

    fn pixels(&mut self, pixels: usize) -> Result<(), ProxyError> {
        self.checkpoint()?;
        self.pixels = self.pixels.saturating_add(pixels);
        if self.pixels > MAX_PIXEL_WORK_PER_REQUEST {
            return Err(ProxyError::bad_request(
                "Kiro images exceed the local pixel work budget",
            ));
        }
        Ok(())
    }

    fn encode(&mut self, pixels: usize) -> Result<(), ProxyError> {
        self.encodes = self.encodes.saturating_add(1);
        if self.encodes > MAX_ENCODE_ATTEMPTS_PER_REQUEST {
            return Err(ProxyError::bad_request(
                "Kiro images exceed the local encoding work budget",
            ));
        }
        self.pixels(pixels)
    }
}

struct CancelImageWork(Arc<AtomicBool>);
impl Drop for CancelImageWork {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(super) fn has_images(body: &Value) -> bool {
    body.get("system").is_some_and(contains_image_block)
        || body
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|message| message.get("content").is_some_and(contains_image_block))
}

pub(super) async fn prepare_bounded(
    body: &Value,
    budget: Option<&RequestMemoryBudget>,
    memory: Option<RequestMemoryReservation>,
    timeout: Duration,
) -> Result<(Value, Option<RequestMemoryReservation>), ProxyError> {
    let pool = IMAGE_JOBS
        .get_or_init(|| Arc::new(Semaphore::new(MAX_BLOCKING_JOBS)))
        .clone();
    run_bounded(
        body,
        budget,
        memory,
        timeout,
        pool,
        prepare_images_controlled,
    )
    .await
}

async fn run_bounded(
    body: &Value,
    budget: Option<&RequestMemoryBudget>,
    memory: Option<RequestMemoryReservation>,
    timeout: Duration,
    pool: Arc<Semaphore>,
    operation: impl FnOnce(&mut Value, Option<&RequestMemoryBudget>, &mut ImageWork) -> Result<(), ProxyError>
        + Send
        + 'static,
) -> Result<(Value, Option<RequestMemoryReservation>), ProxyError> {
    // No waiting queue. The permit belongs to the actual worker, including
    // after cancellation of its asynchronous caller or while Tokio queues it.
    let permit = pool.try_acquire_owned().map_err(|_| ProxyError {
        status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
        message: "Kiro image preparation capacity is full".into(),
    })?;
    if let Some(memory) = memory.as_ref() {
        memory
            .resize(retained_json_bytes(body))
            .map_err(|error| error.into_proxy_error())?;
    }
    let mut body = body.clone();
    let budget = budget.cloned();
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel = CancelImageWork(cancelled.clone());
    let job = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut work = ImageWork {
            cancelled: Some(cancelled),
            ..Default::default()
        };
        work.checkpoint()?;
        operation(&mut body, budget.as_ref(), &mut work)?;
        work.checkpoint()?;
        if let Some(memory) = memory.as_ref() {
            memory
                .resize(retained_json_bytes(&body))
                .map_err(|error| error.into_proxy_error())?;
        }
        // The completed output keeps its reservation even before join is polled.
        Ok((body, memory))
    });
    tokio::time::timeout(timeout, job)
        .await
        .map_err(|_| ProxyError {
            status: axum::http::StatusCode::GATEWAY_TIMEOUT,
            message: "Kiro image preparation exceeded its local deadline".into(),
        })?
        .map_err(|_| ProxyError::bad_gateway("Kiro image worker failed"))?
}

#[derive(Debug, Clone)]
struct PreparedImage {
    media_type: &'static str,
    data: String,
    decoded_bytes: usize,
    memory: Option<RequestMemoryReservation>,
}

#[cfg(test)]
pub(super) fn prepare_anthropic_images(body: &mut Value) -> Result<(), ProxyError> {
    prepare_anthropic_images_with_memory(body, None)
}

pub(super) fn prepare_anthropic_images_with_memory(
    body: &mut Value,
    request_memory: Option<&RequestMemoryBudget>,
) -> Result<(), ProxyError> {
    prepare_images_controlled(body, request_memory, &mut ImageWork::default())
}

fn prepare_images_controlled(
    body: &mut Value,
    request_memory: Option<&RequestMemoryBudget>,
    work: &mut ImageWork,
) -> Result<(), ProxyError> {
    work.checkpoint()?;
    validate_image_placement(body)?;
    let mut hashes = Vec::new();
    visit_image_blocks(body, &mut |block| {
        work.checkpoint()?;
        let data = image_data(block).ok_or_else(|| {
            ProxyError::bad_request("Anthropic image block must contain string base64 source.data")
        })?;
        hashes.push(hash(data));
        Ok(())
    })?;
    if hashes.len() > MAX_IMAGES {
        return Err(ProxyError::bad_request(format!(
            "Kiro request contains {} images; maximum is {MAX_IMAGES}",
            hashes.len()
        )));
    }
    let mut last_occurrence = HashMap::new();
    for (index, digest) in hashes.into_iter().enumerate() {
        last_occurrence.insert(digest, index);
    }

    let mut ordinal = 0usize;
    let mut total_bytes = 0usize;
    visit_image_blocks(body, &mut |block| {
        work.checkpoint()?;
        let data = image_data(block).expect("image blocks were validated in the first pass");
        let digest = hash(data);
        let keep = last_occurrence.get(&digest).copied() == Some(ordinal);
        ordinal = ordinal.saturating_add(1);
        if !keep {
            *block = json!({
                "type": "text",
                "text": "[image omitted: identical to a newer screenshot]"
            });
            return Ok(());
        }
        let prepared = prepare_image_controlled(data, request_memory, work)?;
        if let Some(memory) = prepared.memory.as_ref() {
            memory
                .resize(
                    prepared
                        .data
                        .capacity()
                        .saturating_add(std::mem::size_of::<PreparedImage>()),
                )
                .map_err(|error| error.into_proxy_error())?;
            work.retained_outputs.push(memory.clone());
        }
        total_bytes = total_bytes.saturating_add(prepared.decoded_bytes);
        if total_bytes > MAX_TOTAL_OUTPUT_BYTES {
            return Err(ProxyError::bad_request(format!(
                "Kiro image payload exceeds {} MiB after processing",
                MAX_TOTAL_OUTPUT_BYTES / (1024 * 1024)
            )));
        }
        let Some(source) = block.get_mut("source").and_then(Value::as_object_mut) else {
            return Err(ProxyError::bad_request(
                "Anthropic image block must contain a base64 source",
            ));
        };
        source.insert("type".to_string(), json!("base64"));
        source.insert("media_type".to_string(), json!(prepared.media_type));
        source.insert("data".to_string(), json!(prepared.data));
        Ok(())
    })
}

/// Only user content arrays can become Kiro user images. The converter also
/// lifts images one level inside a user tool_result; all other image blocks
/// would be validated here and then silently disappear from the wire body.
fn validate_image_placement(body: &Value) -> Result<(), ProxyError> {
    let unsupported =
        || ProxyError::bad_request("Kiro image input has an unsupported content location");
    if body.get("system").is_some_and(contains_image_block) {
        return Err(unsupported());
    }
    for message in body
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(content) = message.get("content") else {
            continue;
        };
        if message.get("role").and_then(Value::as_str) != Some("user") {
            if contains_image_block(content) {
                return Err(unsupported());
            }
            continue;
        }
        match content {
            Value::Array(blocks) => {
                for block in blocks {
                    match block.get("type").and_then(Value::as_str) {
                        Some("image") => {}
                        Some("tool_result") => {
                            if let Some(nested) = block.get("content") {
                                match nested {
                                    Value::Array(parts) => {
                                        if parts.iter().any(|part| {
                                            part.get("type").and_then(Value::as_str)
                                                != Some("image")
                                                && contains_image_block(part)
                                        }) {
                                            return Err(unsupported());
                                        }
                                    }
                                    _ if contains_image_block(nested) => return Err(unsupported()),
                                    _ => {}
                                }
                            }
                        }
                        _ if contains_image_block(block) => return Err(unsupported()),
                        _ => {}
                    }
                }
            }
            _ if contains_image_block(content) => return Err(unsupported()),
            _ => {}
        }
    }
    Ok(())
}

fn contains_image_block(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(contains_image_block),
        Value::Object(_) => match value.get("type").and_then(Value::as_str) {
            Some("image") => true,
            Some("tool_result") => value.get("content").is_some_and(contains_image_block),
            _ => false,
        },
        _ => false,
    }
}

#[cfg(test)]
fn prepare_image(
    data: &str,
    request_memory: Option<&RequestMemoryBudget>,
) -> Result<PreparedImage, ProxyError> {
    prepare_image_controlled(data, request_memory, &mut ImageWork::default())
}

fn prepare_image_controlled(
    data: &str,
    request_memory: Option<&RequestMemoryBudget>,
    work: &mut ImageWork,
) -> Result<PreparedImage, ProxyError> {
    work.checkpoint()?;
    if data.len() > MAX_ENCODED_INPUT_BYTES.saturating_mul(4) / 3 + 8 {
        return Err(ProxyError::bad_request(format!(
            "Kiro image exceeds {} MiB encoded input limit",
            MAX_ENCODED_INPUT_BYTES / (1024 * 1024)
        )));
    }
    let decoded_upper_bound = data
        .len()
        .saturating_add(3)
        .checked_div(4)
        .unwrap_or(usize::MAX)
        .saturating_mul(3);
    let initial_output_bound = base64_encoded_len(decoded_upper_bound);
    let working_memory = request_memory
        .map(|budget| {
            budget.reserve(
                RequestMemoryComponent::DecodedBody,
                data.len()
                    .saturating_add(decoded_upper_bound)
                    .saturating_add(initial_output_bound),
            )
        })
        .transpose()
        .map_err(|error| error.into_proxy_error())?;
    let decoded = BASE64
        .decode(data.as_bytes())
        .map_err(|_| ProxyError::bad_request("Anthropic image source is not valid base64"))?;
    if decoded.len() > MAX_ENCODED_INPUT_BYTES {
        return Err(ProxyError::bad_request(format!(
            "Kiro image exceeds {} MiB decoded input limit",
            MAX_ENCODED_INPUT_BYTES / (1024 * 1024)
        )));
    }
    let format = magic_format(&decoded)
        .ok_or_else(|| ProxyError::bad_request("Anthropic image has an unsupported byte format"))?;
    let dimensions = imagesize::blob_size(&decoded)
        .map_err(|_| ProxyError::bad_request("Anthropic image dimensions could not be read"))?;
    if dimensions.width > MAX_DECODE_LONG_SIDE
        || dimensions.height > MAX_DECODE_LONG_SIDE
        || dimensions.width.saturating_mul(dimensions.height) > MAX_DECODE_PIXELS
    {
        return Err(ProxyError::bad_request(
            "Anthropic image dimensions exceed the Kiro decode budget",
        ));
    }
    let long_side = dimensions.width.max(dimensions.height) as u32;
    let output_len = base64_encoded_len(decoded.len());
    if format == "image/gif" && output_len > MAX_WIRE_BASE64_CHARS {
        return Err(ProxyError::bad_request(
            "Kiro animated GIF exceeds the local encoded field budget; animation cannot be flattened",
        ));
    }
    if format == "image/gif" || (output_len <= MAX_WIRE_BASE64_CHARS && long_side <= MAX_LONG_SIDE)
    {
        work.checkpoint()?;
        return Ok(PreparedImage {
            media_type: format,
            data: BASE64.encode(&decoded),
            decoded_bytes: decoded.len(),
            memory: working_memory,
        });
    }

    let source_pixels = dimensions.width.saturating_mul(dimensions.height);
    let (resized_width, resized_height) =
        scaled_dimensions(dimensions.width, dimensions.height, MAX_LONG_SIDE as usize);
    let resized_pixels = resized_width.saturating_mul(resized_height);
    // PNG may decode to RGBA16. Orientation can temporarily own two source
    // rasters; the image crate's separable resize also keeps a RGBA32F
    // intermediate (at most the source pixel count) beside source and output.
    let raw_output_limit = MAX_WIRE_BASE64_CHARS / 4 * 3;
    let image_working_bytes = data
        .len()
        .saturating_add(decoded.len())
        .saturating_add(source_pixels.saturating_mul(24))
        .saturating_add(resized_pixels.saturating_mul(11))
        .saturating_add(1024 * 1024)
        .saturating_add(raw_output_limit)
        .saturating_add(MAX_WIRE_BASE64_CHARS);
    if let Some(memory) = working_memory.as_ref() {
        memory
            .resize(image_working_bytes)
            .map_err(|error| error.into_proxy_error())?;
    }
    work.pixels(source_pixels)?;
    let mut reader = image::ImageReader::new(Cursor::new(&decoded))
        .with_guessed_format()
        .map_err(|_| ProxyError::bad_request("Anthropic image format could not be decoded"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_LONG_SIDE as u32);
    limits.max_image_height = Some(MAX_DECODE_LONG_SIDE as u32);
    limits.max_alloc = Some(source_pixels.saturating_mul(8).saturating_add(1024 * 1024) as u64);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| ProxyError::bad_request("Anthropic image could not be decoded"))?;
    let orientation = decoder
        .orientation()
        .map_err(|_| ProxyError::bad_request("Anthropic image orientation is invalid"))?;
    let mut image = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| ProxyError::bad_request("Anthropic image could not be decoded"))?;
    work.checkpoint()?;
    if orientation != image::metadata::Orientation::NoTransforms {
        work.pixels(source_pixels)?;
        image.apply_orientation(orientation);
    }
    let mut max_long = (image.width().max(image.height()) as usize).min(MAX_LONG_SIDE as usize);
    let mut attempts = 0;
    loop {
        work.checkpoint()?;
        let (width, height) =
            scaled_dimensions(image.width() as usize, image.height() as usize, max_long);
        let pixels = width.saturating_mul(height);
        work.pixels(source_pixels.saturating_add(pixels))?;
        let rgb = image
            .resize_exact(width as u32, height as u32, FilterType::Triangle)
            .to_rgb8();
        for quality in JPEG_QUALITIES {
            if attempts >= MAX_ENCODE_ATTEMPTS_PER_IMAGE {
                return Err(ProxyError::bad_request(
                    "Kiro image exceeded its encoding attempt budget",
                ));
            }
            attempts += 1;
            work.encode(pixels)?;
            let mut output = LimitedImageWriter::new(raw_output_limit);
            let result = JpegEncoder::new_with_quality(&mut output, quality).encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            );
            work.checkpoint()?;
            if result.is_ok() {
                let encoded = BASE64.encode(&output.bytes);
                if encoded.len() > MAX_WIRE_BASE64_CHARS {
                    return Err(ProxyError::bad_request(
                        "Kiro image exceeds the local encoded field budget",
                    ));
                }
                return Ok(PreparedImage {
                    media_type: "image/jpeg",
                    data: encoded,
                    decoded_bytes: output.bytes.len(),
                    memory: working_memory,
                });
            }
            if !output.exhausted {
                return Err(ProxyError::bad_request(
                    "Anthropic image could not be re-encoded",
                ));
            }
        }
        if max_long <= MIN_LONG_SIDE {
            return Err(ProxyError::bad_request(
                "Kiro image cannot fit the local encoded field budget",
            ));
        }
        max_long = (max_long * 3 / 4).max(MIN_LONG_SIDE);
    }
}

/// Oversized candidates stop writing at the raw byte bound instead of first
/// allocating an arbitrarily large JPEG and discovering its size afterwards.
struct LimitedImageWriter {
    bytes: Vec<u8>,
    limit: usize,
    exhausted: bool,
}
impl LimitedImageWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit),
            limit,
            exhausted: false,
        }
    }
}
impl Write for LimitedImageWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exhausted = true;
            return Err(std::io::Error::other(
                "encoded image exceeds the local field budget",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn base64_encoded_len(bytes: usize) -> usize {
    bytes
        .saturating_add(2)
        .checked_div(3)
        .unwrap_or(usize::MAX)
        .saturating_mul(4)
}

fn scaled_dimensions(width: usize, height: usize, max_long_side: usize) -> (usize, usize) {
    let long_side = width.max(height);
    if long_side <= max_long_side || long_side == 0 {
        return (width, height);
    }
    let scaled_width = width
        .saturating_mul(max_long_side)
        .saturating_add(long_side - 1)
        / long_side;
    let scaled_height = height
        .saturating_mul(max_long_side)
        .saturating_add(long_side - 1)
        / long_side;
    (scaled_width.max(1), scaled_height.max(1))
}

fn magic_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn hash(data: &str) -> [u8; 32] {
    Sha256::digest(data.as_bytes()).into()
}

fn image_data(block: &Value) -> Option<&str> {
    (block.get("type").and_then(Value::as_str) == Some("image"))
        .then(|| block.pointer("/source/data").and_then(Value::as_str))
        .flatten()
}

fn visit_image_blocks(
    body: &mut Value,
    visitor: &mut impl FnMut(&mut Value) -> Result<(), ProxyError>,
) -> Result<(), ProxyError> {
    if let Some(system) = body.get_mut("system") {
        visit_content_blocks(system, visitor)?;
    }
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    for message in messages {
        if let Some(content) = message.get_mut("content") {
            visit_content_blocks(content, visitor)?;
        }
    }
    Ok(())
}

fn visit_content_blocks(
    value: &mut Value,
    visitor: &mut impl FnMut(&mut Value) -> Result<(), ProxyError>,
) -> Result<(), ProxyError> {
    match value {
        Value::Array(items) => {
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("image") => visitor(item)?,
                    Some("tool_result") => {
                        if let Some(content) = item.get_mut("content") {
                            visit_content_blocks(content, visitor)?;
                        }
                    }
                    _ => {}
                }
            }
        }
        Value::Object(_) => match value.get("type").and_then(Value::as_str) {
            Some("image") => visitor(value)?,
            Some("tool_result") => {
                if let Some(content) = value.get_mut("content") {
                    visit_content_blocks(content, visitor)?;
                }
            }
            _ => {}
        },
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn kiro_r5_cancelled_blocking_work_keeps_its_permit_and_memory_until_exit() {
        abandoned_worker_retains_resources(false).await;
    }

    #[tokio::test]
    async fn kiro_r5_timed_out_blocking_work_keeps_its_permit_and_memory_until_exit() {
        abandoned_worker_retains_resources(true).await;
    }

    async fn abandoned_worker_retains_resources(timeout: bool) {
        let pool = Arc::new(Semaphore::new(1));
        let budget = RequestMemoryBudget::new(1024 * 1024);
        let body = json!({"payload":"worker-owned input"});
        let memory = budget
            .reserve(
                RequestMemoryComponent::NormalizedBody,
                retained_json_bytes(&body),
            )
            .unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker_pool = pool.clone();
        let worker_budget = budget.clone();
        let task = tokio::spawn(async move {
            run_bounded(
                &body,
                Some(&worker_budget),
                Some(memory),
                if timeout {
                    Duration::from_millis(50)
                } else {
                    Duration::from_secs(10)
                },
                worker_pool,
                move |_, _, _| {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                },
            )
            .await
        });
        started_rx.await.unwrap();
        if timeout {
            assert_eq!(
                task.await.unwrap().unwrap_err().status,
                axum::http::StatusCode::GATEWAY_TIMEOUT
            );
        } else {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        }
        assert_eq!(pool.available_permits(), 0);
        assert!(budget.snapshot().used_bytes > 0);
        let rejected = run_bounded(
            &json!({}),
            None,
            None,
            Duration::from_secs(1),
            pool.clone(),
            |_, _, _| Ok(()),
        )
        .await
        .unwrap_err();
        assert_eq!(rejected.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
        assert!(!budget.is_exhausted());
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while pool.available_permits() != 1 || budget.snapshot().used_bytes != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn kiro_r5_processed_fields_keep_reservations_until_body_adopts_them() {
        let budget = RequestMemoryBudget::new(16 * 1024 * 1024);
        let mut bytes = Cursor::new(Vec::new());
        image::RgbImage::from_pixel(8192, 1, image::Rgb([255, 0, 0]))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let mut body = json!({"messages":[{"role":"user","content":[{
            "type":"image","source":{"type":"base64","data":BASE64.encode(bytes.into_inner())}
        }]}]});
        let mut work = ImageWork::default();
        prepare_images_controlled(&mut body, Some(&budget), &mut work).unwrap();
        let field = body["messages"][0]["content"][0]["source"]["data"]
            .as_str()
            .unwrap();
        assert!(budget.snapshot().used_bytes >= field.len());
        assert!(
            budget.snapshot().used_bytes < 100_000,
            "decoded rasters must already be released"
        );
        drop(work);
        assert_eq!(budget.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn kiro_r5_unpolled_completed_blocking_result_retains_output_memory() {
        let pool = Arc::new(Semaphore::new(1));
        let budget = RequestMemoryBudget::new(1024 * 1024);
        let body = json!({"payload":"completed output"});
        let memory = budget
            .reserve(
                RequestMemoryComponent::NormalizedBody,
                retained_json_bytes(&body),
            )
            .unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut task = Box::pin(run_bounded(
            &body,
            Some(&budget),
            Some(memory),
            Duration::from_secs(10),
            pool.clone(),
            move |_, _, _| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            },
        ));
        assert!(futures_util::poll!(task.as_mut()).is_pending());
        started_rx.await.unwrap();
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while pool.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(budget.snapshot().used_bytes > 0);
        drop(task);
        // The worker releases its permit just before Tokio publishes the join
        // result; detached completion may drop that result on the next turn.
        tokio::time::timeout(Duration::from_secs(5), async {
            while budget.snapshot().used_bytes != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn kiro_r5_runtime_limits_match_the_separate_local_policy_fixture() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../assets/contract/kiro-image-budget.json"
        ))
        .unwrap();
        let policy = &fixture["localPolicy"];
        assert_eq!(fixture["liveState"], "live_pending");
        assert_eq!(fixture["reference"]["officialVendorLimitProven"], false);
        assert_eq!(
            policy["maxWireBase64CharactersPerImage"],
            MAX_WIRE_BASE64_CHARS
        );
        assert_eq!(policy["jpegQualities"], json!(JPEG_QUALITIES));
        assert_eq!(
            policy["maxEncodeAttemptsPerImage"],
            MAX_ENCODE_ATTEMPTS_PER_IMAGE
        );
        assert_eq!(
            policy["maxEncodeAttemptsPerRequest"],
            MAX_ENCODE_ATTEMPTS_PER_REQUEST
        );
        assert_eq!(policy["maxPixelWorkPerRequest"], MAX_PIXEL_WORK_PER_REQUEST);
        assert_eq!(policy["maxConcurrentBlockingJobs"], MAX_BLOCKING_JOBS);
        assert_eq!(policy["minimumLongSide"], MIN_LONG_SIDE);
    }

    #[test]
    fn kiro_r5_twenty_images_extreme_aspect_and_request_work_budgets() {
        let mut blocks = Vec::new();
        for color in 0..MAX_IMAGES {
            let mut bytes = Cursor::new(Vec::new());
            image::RgbImage::from_pixel(1, 1, image::Rgb([color as u8, 0, 0]))
                .write_to(&mut bytes, image::ImageFormat::Png)
                .unwrap();
            blocks.push(json!({"type":"image","source":{"type":"base64","data":BASE64.encode(bytes.into_inner())}}));
        }
        let mut body = json!({"messages":[{"role":"user","content":blocks}]});
        prepare_anthropic_images(&mut body).unwrap();
        assert_eq!(body["messages"][0]["content"].as_array().unwrap().len(), 20);
        let first = body["messages"][0]["content"][0].clone();
        body["messages"][0]["content"]
            .as_array_mut()
            .unwrap()
            .push(first);
        assert!(prepare_anthropic_images(&mut body)
            .unwrap_err()
            .message
            .contains("maximum is 20"));

        let mut bytes = Cursor::new(Vec::new());
        noise(8192, 1)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let input = BASE64.encode(bytes.into_inner());
        let prepared = prepare_image(&input, None).unwrap();
        let output = image::load_from_memory(&BASE64.decode(&prepared.data).unwrap()).unwrap();
        assert_eq!((output.width(), output.height()), (1568, 1));
        let mut pixels = ImageWork {
            pixels: MAX_PIXEL_WORK_PER_REQUEST,
            ..Default::default()
        };
        assert!(prepare_image_controlled(&input, None, &mut pixels)
            .unwrap_err()
            .message
            .contains("pixel work budget"));
        let mut encodes = ImageWork {
            encodes: MAX_ENCODE_ATTEMPTS_PER_REQUEST,
            ..Default::default()
        };
        assert!(prepare_image_controlled(&input, None, &mut encodes)
            .unwrap_err()
            .message
            .contains("encoding work budget"));
    }

    fn noise(width: u32, height: u32) -> image::RgbImage {
        let mut state = 0x1234_5678_u32;
        image::RgbImage::from_fn(width, height, |_, _| {
            let mut channels = [0; 3];
            for value in &mut channels {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *value = state as u8;
            }
            image::Rgb(channels)
        })
    }

    #[test]
    fn kiro_r5_final_base64_budget_catches_already_compressed_input() {
        let mut input = None;
        for side in (400..=800).step_by(50) {
            let rgb = noise(side, side);
            let mut bytes = Vec::new();
            JpegEncoder::new_with_quality(&mut bytes, 70)
                .encode(rgb.as_raw(), side, side, image::ExtendedColorType::Rgb8)
                .unwrap();
            if bytes.len() > 300_000 && bytes.len() <= RESIZE_THRESHOLD_BYTES {
                input = Some(BASE64.encode(bytes));
                break;
            }
        }
        let input = input
            .expect("fixture must cross the base64 field limit below the old decoded threshold");
        let prepared = prepare_image(&input, None).unwrap();
        assert!(
            prepared.data.len() <= 400_000,
            "final base64 field has {} characters",
            prepared.data.len()
        );
        image::load_from_memory(&BASE64.decode(&prepared.data).unwrap()).unwrap();
    }

    #[test]
    fn kiro_r5_high_entropy_image_converges_to_a_decodable_wire_field() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        noise(1568, 1568)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let prepared = prepare_image(&BASE64.encode(bytes.into_inner()), None).unwrap();
        assert!(
            prepared.data.len() <= 400_000,
            "oversized final field: {}",
            prepared.data.len()
        );
        let decoded = image::load_from_memory(&BASE64.decode(&prepared.data).unwrap()).unwrap();
        assert!(decoded.width().max(decoded.height()) <= MAX_LONG_SIDE);
    }

    fn animated_gif() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            encoder
                .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                    2,
                    2,
                    image::Rgba([255, 0, 0, 255]),
                )))
                .unwrap();
            encoder
                .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                    2,
                    2,
                    image::Rgba([0, 0, 255, 255]),
                )))
                .unwrap();
        }
        bytes
    }

    #[test]
    fn kiro_r5_animated_gif_is_preserved_or_explicitly_rejected_when_oversized() {
        let mut bytes = animated_gif();
        let encoded = BASE64.encode(&bytes);
        let prepared = prepare_image(&encoded, None).unwrap();
        assert_eq!(prepared.media_type, "image/gif");
        assert_eq!(prepared.data, encoded);
        assert_eq!(bytes.pop(), Some(0x3b));
        bytes.extend_from_slice(&[0x21, 0xfe]);
        for _ in 0..1250 {
            bytes.push(255);
            bytes.extend_from_slice(&[b'x'; 255]);
        }
        bytes.extend_from_slice(&[0, 0x3b]);
        assert!(
            prepare_image(&BASE64.encode(bytes), None).is_err(),
            "oversized GIF must not bypass the final field budget"
        );
    }

    #[test]
    fn kiro_r5_reencoding_applies_exif_orientation() {
        let rgb = image::RgbImage::from_pixel(2000, 100, image::Rgb([80, 90, 100]));
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, 85)
            .encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        let exif = b"Exif\0\0II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
        let mut oriented = bytes[..2].to_vec();
        oriented.extend_from_slice(&[0xff, 0xe1]);
        oriented.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        oriented.extend_from_slice(exif);
        oriented.extend_from_slice(&bytes[2..]);
        let prepared = prepare_image(&BASE64.encode(oriented), None).unwrap();
        let image = image::load_from_memory(&BASE64.decode(prepared.data).unwrap()).unwrap();
        assert!(
            image.height() > image.width(),
            "portrait orientation was lost"
        );
    }

    const WIRE_PROTOCOL_JSON: &str =
        include_str!("../../../assets/contract/kiro-wire-protocol.json");

    const PNG_1X1: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

    #[test]
    fn corrects_mime_and_keeps_only_newest_duplicate() {
        let mut body = json!({
            "messages": [
                {"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":PNG_1X1}}]},
                {"role":"assistant","content":"seen"},
                {"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":PNG_1X1}}]}
            ]
        });
        prepare_anthropic_images(&mut body).unwrap();
        assert_eq!(
            body.pointer("/messages/0/content/0/type"),
            Some(&json!("text"))
        );
        assert_eq!(
            body.pointer("/messages/2/content/0/source/media_type"),
            Some(&json!("image/png"))
        );
    }

    #[test]
    fn rejects_invalid_base64_before_forwarding() {
        let mut body = json!({
            "messages": [{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"not base64"}}]}]
        });
        assert!(prepare_anthropic_images(&mut body).is_err());
    }

    #[test]
    fn image_decode_working_set_obeys_request_memory_budget() {
        let mut body = json!({
            "messages": [{
                "role":"user",
                "content":[{
                    "type":"image",
                    "source":{
                        "type":"base64",
                        "media_type":"image/png",
                        "data":PNG_1X1
                    }
                }]
            }]
        });
        let budget = RequestMemoryBudget::new(PNG_1X1.len().saturating_add(1));

        let error = prepare_anthropic_images_with_memory(&mut body, Some(&budget)).unwrap_err();

        assert!(error.is_request_memory_exhausted(), "{error:?}");
        assert!(budget.is_exhausted());
    }

    #[test]
    fn rejects_image_without_string_base64_data_before_forwarding() {
        for source in [json!({"type": "base64"}), json!({"data": 42})] {
            let mut body = json!({
                "messages": [{"role":"user","content":[{"type":"image","source":source}]}]
            });
            let error = prepare_anthropic_images(&mut body).unwrap_err();
            assert!(error.message.contains("source.data"));
        }
    }

    #[test]
    fn does_not_treat_tool_input_type_fields_as_content_images() {
        let mut body = json!({
            "messages": [
                {
                    "role": "assistant",
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_1",
                        "name": "classify",
                        "input": {"type": "image", "label": "diagram"}
                    }]
                },
                {"role": "user", "content": "continue"}
            ]
        });
        prepare_anthropic_images(&mut body).unwrap();
        assert_eq!(
            body.pointer("/messages/0/content/0/input/type"),
            Some(&json!("image"))
        );
    }

    #[test]
    fn image_budgets_match_the_wire_protocol_fixture() {
        let fixture: Value = serde_json::from_str(WIRE_PROTOCOL_JSON).unwrap();
        let images = &fixture["images"];
        assert_eq!(images["maxImages"], MAX_IMAGES);
        assert_eq!(
            images["maxDecodedInputBytesPerImage"],
            MAX_ENCODED_INPUT_BYTES
        );
        assert_eq!(images["maxTotalOutputBytes"], MAX_TOTAL_OUTPUT_BYTES);
        assert_eq!(images["resizeThresholdBytes"], RESIZE_THRESHOLD_BYTES);
        assert_eq!(images["maxLongSide"], MAX_LONG_SIDE);
        assert_eq!(images["maxDecodeLongSide"], MAX_DECODE_LONG_SIDE);
        assert_eq!(images["maxDecodePixels"], MAX_DECODE_PIXELS);
        assert_eq!(images["jpegQuality"], JPEG_QUALITY);
        assert_eq!(images["mimeFromMagicBytes"], true);
        assert_eq!(images["duplicatePolicy"], "keep_newest");
    }
}
