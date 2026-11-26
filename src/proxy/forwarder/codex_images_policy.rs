// Dedicated Images policy is separate from ordinary native Responses/Lite.
// Reference projects are read-only protocol evidence; this code is Server-owned.
fn configure_codex_images_adapter(
    execution: &ProviderExecution,
    prepared: &mut CodexImagesPreparedRequest,
    request: &mut adapters::AdapterRequest,
) -> Result<(), ProxyError> {
    prepared.transport = execution
        .plan
        .driver_options
        .get("codexImageTransport")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|_| ProxyError::bad_request("invalid codexImageTransport"))?
        .unwrap_or_default();
    if prepared.transport == CodexImageTransport::Responses {
        let configured = execution
            .plan
            .driver_options
            .get("codexImageResponsesModel")
            .and_then(Value::as_str);
        let model =
            crate::domain::providers::model::normalize_codex_image_responses_model(configured)
                .map_err(|error| ProxyError::bad_request(error.to_string()))?
                .unwrap_or_else(|| CODEX_IMAGES_RESPONSES_MAIN_MODEL.to_string());
        let mut value: Value = serde_json::from_slice(&request.body)
            .map_err(|error| ProxyError::bad_request(error.to_string()))?;
        value["model"] = json!(model);
        request.body = Bytes::from(
            serde_json::to_vec(&value)
                .map_err(|error| ProxyError::bad_request(error.to_string()))?,
        );
        request.model = Some(model.clone());
        request.actual_model = Some(model);
        // Existing explicit single-model policy still owns the bridge's text model.
        execution.enforce_model_policy(request)?;
        let model = codex_model_from_body(&request.body)
            .ok_or_else(|| ProxyError::bad_request("Codex Images text driver is missing"))?;
        crate::domain::providers::model::normalize_codex_image_responses_model(Some(&model))
            .map_err(|error| ProxyError::bad_request(error.to_string()))?;
        prepared.actual_model = model;
        prepared.actual_model_source = request
            .actual_model_source
            .clone()
            .unwrap_or_else(|| "codex_image_generation_bridge".to_string());
    } else {
        if prepared.intent.requested_reasoning_effort.is_some() {
            return Err(ProxyError::bad_request(
                "direct Images requests do not support reasoning effort; omit reasoning parameters",
            ));
        }
        let value: Value = serde_json::from_slice(&request.body)
            .map_err(|error| ProxyError::bad_request(error.to_string()))?;
        prepared.actual_model = value
            .pointer("/tools/0/model")
            .and_then(Value::as_str)
            .ok_or_else(|| ProxyError::bad_request("Codex Images image model is missing"))?
            .to_string();
        prepared.actual_model_source = "codex_images_direct".to_string();
        // The direct operation's model is an image model. Text single-model policy
        // remains unchanged for text requests and never rewrites an image model.
        request.model = Some(prepared.actual_model.clone());
        request.actual_model = Some(prepared.actual_model.clone());
        request.actual_model_source = Some(prepared.actual_model_source.clone());
    }
    prepared.body = request.body.clone();
    Ok(())
}

fn codex_images_direct_body(
    body: &Bytes,
    operation: CodexImagesOperation,
) -> Result<Bytes, ProxyError> {
    let source: Value =
        serde_json::from_slice(body).map_err(|error| ProxyError::bad_request(error.to_string()))?;
    let tool = source
        .pointer("/tools/0")
        .and_then(Value::as_object)
        .ok_or_else(|| ProxyError::bad_request("Codex Images image tool is missing"))?;
    let mut target = serde_json::Map::new();
    for (key, value) in tool {
        if !matches!(key.as_str(), "type" | "action" | "input_image_mask") {
            target.insert(key.clone(), value.clone());
        }
    }
    let content = source
        .pointer("/input/0/content")
        .and_then(Value::as_array)
        .ok_or_else(|| ProxyError::bad_request("Codex Images input is missing"))?;
    let prompt = content
        .iter()
        .find(|part| part.get("type").and_then(Value::as_str) == Some("input_text"))
        .and_then(|part| part.get("text"))
        .cloned()
        .ok_or_else(|| ProxyError::bad_request("Codex Images prompt is missing"))?;
    target.insert("prompt".to_string(), prompt);
    target.insert("stream".to_string(), json!(true));
    target.insert("response_format".to_string(), json!("b64_json"));
    target.insert("n".to_string(), json!(1));
    if operation == CodexImagesOperation::Edit {
        let images: Vec<Value> = content
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("input_image"))
            .filter_map(|part| part.get("image_url"))
            .map(|url| json!({"image_url":url}))
            .collect();
        if images.is_empty() {
            return Err(ProxyError::bad_request(
                "Codex Images edit input is missing",
            ));
        }
        target.insert("images".to_string(), json!(images));
        if let Some(mask) = tool.get("input_image_mask") {
            target.insert("mask".to_string(), mask.clone());
        }
    }
    serde_json::to_vec(&target)
        .map(Bytes::from)
        .map_err(|error| ProxyError::bad_request(error.to_string()))
}

fn codex_images_upstream_endpoint(
    responses_url: &str,
    prepared: &CodexImagesPreparedRequest,
) -> Result<String, ProxyError> {
    if prepared.transport == CodexImageTransport::Responses {
        return Ok(responses_url.to_string());
    }
    let mut url = url::Url::parse(responses_url)
        .map_err(|error| ProxyError::bad_request(error.to_string()))?;
    let base = url
        .path()
        .strip_suffix("/responses")
        .ok_or_else(|| ProxyError::bad_request("invalid Codex Images upstream endpoint"))?;
    let path = format!(
        "{base}/images/{}",
        match prepared.operation {
            CodexImagesOperation::Generation => "generations",
            CodexImagesOperation::Edit => "edits",
        }
    );
    url.set_path(&path);
    Ok(url.into())
}

/// A rejected text driver is a request/model issue, not evidence that the image
/// model or all credentials are unavailable. Preserve the upstream error body.
fn codex_images_text_driver_rejected(body: &[u8], prepared: &CodexImagesPreparedRequest) -> bool {
    if prepared.transport != CodexImageTransport::Responses {
        return false;
    }
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    codex_images_value_rejects_text_driver(&value, &prepared.actual_model)
}

fn codex_images_value_rejects_text_driver(value: &Value, model: &str) -> bool {
    let value = value.get("response").unwrap_or(value);
    let structured = value
        .pointer("/error/code")
        .and_then(Value::as_str)
        .is_some_and(|code| matches!(code, "model_not_found" | "model_not_supported"))
        || value.pointer("/error/subreason").and_then(Value::as_str)
            == Some("model_not_in_accessible_catalog");
    let quoted = format!("'{model}'").to_ascii_lowercase();
    let rejected = [
        value.pointer("/error/message"),
        value.get("detail"),
        value.get("message"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .any(|message| {
        let message = message.to_ascii_lowercase();
        message.contains(&quoted) && (structured || message.contains("model is not supported"))
    });
    rejected
}

impl CodexImagesResponseParser {
    fn direct_event_from_value(
        &mut self,
        value: &Value,
    ) -> Result<Option<CodexImagesProtocolEvent>, CodexImagesFailure> {
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if event_type == "error" || value.get("error").is_some() {
            return Ok(Some(CodexImagesProtocolEvent::Failed(
                CodexImagesFailure::upstream(codex_image_failure_message(value)),
            )));
        }
        if let Some(created) = value
            .get("created_at")
            .or_else(|| value.get("created"))
            .and_then(Value::as_i64)
            .filter(|time| *time > 0)
        {
            self.created_at = created;
        }
        let prefix = self.operation.stream_prefix();
        if event_type == format!("{prefix}.partial_image") {
            let result = value
                .get("b64_json")
                .and_then(Value::as_str)
                .filter(|result| !result.is_empty())
                .ok_or_else(|| {
                    CodexImagesFailure::protocol("direct Images partial event has no image")
                })?;
            let index = value
                .get("partial_image_index")
                .and_then(Value::as_i64)
                .filter(|index| (0..CODEX_IMAGES_MAX_PARTIAL_IMAGES).contains(index))
                .ok_or_else(|| {
                    CodexImagesFailure::protocol("direct Images partial index is invalid")
                })?;
            return Ok(Some(CodexImagesProtocolEvent::Partial(CodexImagePartial {
                result: result.to_string(),
                partial_image_index: index,
                created_at: self.created_at,
                meta: codex_image_meta_from_value(value),
            })));
        }
        let results = if event_type == format!("{prefix}.completed") {
            vec![direct_codex_image_result(value)?]
        } else if event_type.is_empty() && !self.decoder.is_sse() {
            value
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    CodexImagesFailure::protocol("direct Images JSON has no data array")
                })?
                .iter()
                .map(direct_codex_image_result)
                .collect::<Result<Vec<_>, _>>()?
        } else {
            return Err(CodexImagesFailure::protocol(
                "unexpected event on direct Images transport",
            ));
        };
        if results.len() != 1 {
            return Err(CodexImagesFailure::protocol(
                "direct Images must return exactly one image",
            ));
        }
        let mut results = results;
        let meta = codex_image_meta_from_value(value);
        for result in &mut results {
            merge_codex_image_meta(result, &meta);
        }
        Ok(Some(CodexImagesProtocolEvent::Completed(
            CodexImagesCompletion {
                results,
                created_at: self.created_at,
                usage: value
                    .get("usage")
                    .filter(|usage| usage.is_object())
                    .cloned(),
            },
        )))
    }
}

fn direct_codex_image_result(value: &Value) -> Result<CodexImageResult, CodexImagesFailure> {
    let result = value
        .get("b64_json")
        .and_then(Value::as_str)
        .filter(|image| !image.is_empty())
        .ok_or_else(|| {
            CodexImagesFailure::protocol("direct Images completed response has no base64 image")
        })?;
    let mut meta = codex_image_meta_from_value(value);
    meta.result = result.to_string();
    Ok(meta)
}
