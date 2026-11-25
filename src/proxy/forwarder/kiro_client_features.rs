struct KiroFeatureMessage {
    value: Value,
    _memory: Option<RequestMemoryReservation>,
}

struct KiroSearchResult {
    content: Vec<Value>,
    _memory: Option<RequestMemoryReservation>,
}

fn own_kiro_feature_message(
    value: Value,
    memory: Option<RequestMemoryReservation>,
) -> Result<KiroFeatureMessage, ProxyError> {
    if let Some(memory) = &memory {
        memory
            .resize(retained_json_bytes(&value).saturating_mul(2))
            .map_err(|error| error.into_proxy_error())?;
    }
    Ok(KiroFeatureMessage {
        value,
        _memory: memory,
    })
}

fn retain_kiro_feature_output(
    bytes: Bytes,
    memory: Option<RequestMemoryReservation>,
) -> Result<Bytes, ProxyError> {
    match memory {
        Some(memory) => {
            memory
                .resize(bytes.len())
                .map_err(|error| error.into_proxy_error())?;
            Ok(memory.retain_bytes(bytes))
        }
        None => Ok(bytes),
    }
}

async fn forward_kiro_client_features(
    mut options: ClaudeKiroForwardOptions,
    operation: kiro::client_features::Operation,
) -> Result<Response, ProxyError> {
    options.auth_refresh_budget = Some(Arc::new(AtomicBool::new(false)));
    let mut source: Value = serde_json::from_slice(&options.body)
        .map_err(|error| ProxyError::bad_request(error.to_string()))?;
    let search_policy = kiro::client_features::SearchPolicy::from_request(&source)?;
    let streaming = source.get("stream").and_then(Value::as_bool) == Some(true);
    kiro::client_features::prepare_source(&mut source, operation)?;
    let source = Bytes::from(serde_json::to_vec(&source).map_err(ProxyError::bad_gateway)?);
    let mut canonical = kiro_provider::prepare_canonical_request(
        &options.execution,
        &options.stored,
        options.route,
        options.gemini_path.as_deref(),
        &options.headers,
        source,
        &mut options.request_context,
    )?;
    kiro::client_features::prepare_canonical(&mut canonical.request_body, operation)?;
    canonical.stream_requested = false;
    canonical.runtime_request.stream_requested = false;
    let template = canonical.clone();
    if !streaming {
        let message =
            execute_kiro_client_features(&options, canonical, operation, &search_policy).await?;
        let encoded = render_kiro_client_feature_json(&options, &template, operation, message)?;
        let mut response = Response::new(Body::from(encoded));
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        return Ok(response);
    }
    let route = options.route;
    let stream = async_stream::stream! {
        let future = execute_kiro_client_features(&options, canonical, operation, &search_policy);
        tokio::pin!(future);
        let mut ping = tokio::time::interval(Duration::from_secs(10));
        ping.tick().await;
        let result = loop {
            tokio::select! {
                result = &mut future => break result,
                _ = ping.tick() => yield Ok::<Bytes, std::io::Error>(kiro_provider::downstream_keepalive_frame(route)),
            }
        };
        match result.and_then(|message| render_kiro_client_feature_stream(&options, &template, operation, message)) {
            Ok(encoded) => yield Ok::<Bytes, std::io::Error>(encoded),
            Err(error) => if let Some(frame) = stream_terminal_error_frame(route, error.client_message(), error.status.as_u16()) { yield Ok(frame); },
        }
    };
    let mut response = Response::new(Body::from_stream(stream));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok(response)
}

async fn execute_kiro_client_features(
    options: &ClaudeKiroForwardOptions,
    mut canonical: kiro_provider::CanonicalRequest,
    operation: kiro::client_features::Operation,
    search_policy: &kiro::client_features::SearchPolicy,
) -> Result<KiroFeatureMessage, ProxyError> {
    let retained = options
        .request_memory
        .as_ref()
        .map(|budget| {
            budget.reserve(
                RequestMemoryComponent::StreamRetainedState,
                retained_json_bytes(&canonical.request_body).saturating_mul(2),
            )
        })
        .transpose()
        .map_err(|error| error.into_proxy_error())?;
    let mut presentation = Vec::new();
    let mut usage = json!({});
    let mut searches_used = 0;
    for round in 0..=kiro::client_features::MAX_SEARCH_ROUNDS {
        if round == kiro::client_features::MAX_SEARCH_ROUNDS
            || searches_used == search_policy.max_uses
        {
            if let Some(tools) = canonical.request_body["tools"].as_array_mut() {
                tools.retain(|tool| tool["name"] != kiro::client_features::SEARCH_TOOL);
            }
            canonical.request_body["tool_choice"] = json!({"type":"auto"});
        }
        let response = forward_claude_kiro_inner(ClaudeKiroForwardOptions {
            state: options.state.clone(),
            execution: options.execution.clone(),
            stored: options.stored.clone(),
            route: options.route,
            gemini_path: options.gemini_path.clone(),
            headers: options.headers.clone(),
            body: options.body.clone(),
            request_context: options.request_context.clone(),
            account_in_flight_guard: None,
            share_invocation_guard: None,
            started: options.started,
            request_memory: options.request_memory.clone(),
            timing: options.timing.clone(),
            auth_refresh_budget: options.auth_refresh_budget.clone(),
            canonical_override: Some(canonical.clone()),
            canonical_response: true,
        })
        .await?;
        if !response.status().is_success() {
            return Err(ProxyError {
                status: response.status(),
                message: "Kiro model round failed".into(),
            });
        }
        let bytes = axum::body::to_bytes(
            response.into_body(),
            PROXY_BUFFERED_RESPONSE_BODY_LIMIT_BYTES,
        )
        .await
        .map_err(ProxyError::bad_gateway)?;
        let response_memory = options
            .request_memory
            .as_ref()
            .map(|budget| {
                budget.reserve(
                    RequestMemoryComponent::SemanticPrelude,
                    bytes.len().saturating_mul(4),
                )
            })
            .transpose()
            .map_err(|error| error.into_proxy_error())?;
        let mut message: Value = serde_json::from_slice(&bytes).map_err(ProxyError::bad_gateway)?;
        kiro::client_features::append_usage(&mut usage, &message["usage"]);
        if matches!(operation, kiro::client_features::Operation::Compact) {
            return own_kiro_feature_message(message, response_memory);
        }
        let calls = kiro::client_features::search_calls(&message)?;
        if calls.is_empty() || message["stop_reason"] == "refusal" {
            presentation.extend(message["content"].as_array().cloned().unwrap_or_default());
            message["content"] = Value::Array(presentation);
            message["usage"] = usage;
            kiro::client_features::finish_structured(&mut message, &canonical.request_body);
            return own_kiro_feature_message(message, response_memory);
        }
        if round == kiro::client_features::MAX_SEARCH_ROUNDS
            || searches_used + calls.len() > search_policy.max_uses
        {
            return Err(ProxyError::bad_gateway(
                "Kiro exceeded the hosted search round limit",
            ));
        }
        let mut results = Vec::new();
        for block in message["content"].as_array().into_iter().flatten() {
            if block["type"] != "tool_use" || block["name"] != kiro::client_features::SEARCH_TOOL {
                presentation.push(block.clone());
                continue;
            }
            let id = block["id"].as_str().expect("validated search call id");
            let query = calls
                .iter()
                .find(|(call_id, _)| call_id == id)
                .map(|(_, query)| query.as_str())
                .expect("validated query");
            let mut search_result = execute_kiro_hosted_search(options, query).await?;
            searches_used += 1;
            search_policy.filter(&mut search_result.content);
            let found = &search_result.content;
            usage["server_tool_use"] = json!({"web_search_requests":searches_used});
            let server_id = format!("srvtoolu_{id}");
            presentation.push(json!({"type":"server_tool_use","id":server_id,"name":"web_search","input":{"query":query}}));
            presentation.push(
                json!({"type":"web_search_tool_result","tool_use_id":server_id,"content":found}),
            );
            results.push(json!({"type":"tool_result","tool_use_id":id,"content":serde_json::to_string(found).map_err(ProxyError::bad_gateway)?}));
            if let Some(memory) = &retained {
                let presentation_bytes =
                    presentation.iter().map(retained_json_bytes).sum::<usize>();
                let result_bytes = results.iter().map(retained_json_bytes).sum::<usize>();
                memory
                    .resize(
                        retained_json_bytes(&canonical.request_body)
                            .saturating_mul(2)
                            .saturating_add(presentation_bytes)
                            .saturating_add(result_bytes),
                    )
                    .map_err(|error| error.into_proxy_error())?;
            }
        }
        if message["content"].as_array().is_some_and(|blocks| {
            blocks.iter().any(|block| {
                block["type"] == "tool_use" && block["name"] != kiro::client_features::SEARCH_TOOL
            })
        }) {
            message["content"] = Value::Array(presentation);
            message["usage"] = usage;
            return own_kiro_feature_message(message, response_memory);
        }
        let messages = canonical.request_body["messages"]
            .as_array_mut()
            .expect("canonical messages");
        messages.push(json!({"role":"assistant","content":message["content"]}));
        messages.push(json!({"role":"user","content":results}));
        if let Some(memory) = &retained {
            memory
                .resize(
                    retained_json_bytes(&canonical.request_body)
                        .saturating_mul(2)
                        .saturating_add(retained_json_bytes(&json!(presentation))),
                )
                .map_err(|error| error.into_proxy_error())?;
        }
    }
    Err(ProxyError::bad_gateway(
        "Kiro hosted search did not complete",
    ))
}

async fn execute_kiro_hosted_search(
    options: &ClaudeKiroForwardOptions,
    query: &str,
) -> Result<KiroSearchResult, ProxyError> {
    let (provider_type, account_id, generation) = options
        .execution
        .managed_account_identity_target()
        .ok_or_else(|| ProxyError::bad_request("Kiro search requires a bound account"))?;
    let http = forward_http_client(&options.state, &options.execution).await?;
    let version = kiro_provider::runtime_client_version(&options.state, provider_type).await;
    for attempt in 0..2 {
        let accounts = revalidate_execution_before_account_lease(
            &options.state,
            &options.execution,
            options.request_context.share_id.as_deref(),
            options.route.app(),
        )
        .await?;
        options.execution.materialize_auth(&accounts)?;
        let account = accounts
            .find_for_provider(provider_type, Some(account_id))
            .ok_or_else(|| ProxyError::bad_request("Kiro bound account is unavailable"))?;
        let prepared = kiro::prepare_mcp_request(account, query, &version)?;
        let url = match kiro_provider::api_base_override(&options.stored) {
            Some(base) => kiro_provider::url_with_base_override(&base, &prepared.url)?,
            None => prepared.url.clone(),
        };
        let bytes =
            Bytes::from(serde_json::to_vec(&prepared.body).map_err(ProxyError::bad_gateway)?);
        let headers = prepared
            .headers
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect::<Vec<_>>();
        let request = build_upstream_post_request(
            &http,
            &url,
            bytes,
            &HeaderMap::new(),
            &headers,
            options.execution.request_timeout(),
            false,
        )?;
        let mut response = request.send().await.map_err(ProxyError::bad_gateway)?;
        if response.status() == StatusCode::UNAUTHORIZED
            && attempt == 0
            && options
                .auth_refresh_budget
                .as_ref()
                .is_none_or(|budget| !budget.load(Ordering::SeqCst))
            && account
                .refresh_token
                .as_deref()
                .is_some_and(|token| !token.is_empty())
        {
            drop(response);
            if let Some(budget) = &options.auth_refresh_budget {
                budget.store(true, Ordering::SeqCst);
            }
            options
                .state
                .refresh_managed_account_now_for_generation(provider_type, account_id, generation)
                .await
                .map_err(managed_account_refresh_error_to_proxy_error)?;
            continue;
        }
        let status = response.status();
        let response_headers = response.headers().clone();
        let (bytes, memory) = read_response_body_limited_with_memory(
            &mut response,
            512 * 1024,
            options.request_memory.as_ref(),
        )
        .await
        .map_err(|error| error.into_proxy_error())?;
        let decoded = decode_response_body_with_memory(
            &response_headers,
            bytes,
            memory,
            512 * 1024,
            options.request_memory.as_ref(),
        )?;
        if !status.is_success() {
            maybe_mark_upstream_rate_limited(
                &options.state,
                &options.execution,
                status,
                &response_headers,
                &decoded.body,
                None,
                None,
            )
            .await;
            record_provider_outcome(
                &options.state,
                &options.stored,
                ProviderOutcome::from_status(status.as_u16()),
            )
            .await;
            return Err(ProxyError {
                status,
                message: "Kiro hosted search failed".into(),
            });
        }
        revalidate_execution_before_account_lease(
            &options.state,
            &options.execution,
            options.request_context.share_id.as_deref(),
            options.route.app(),
        )
        .await?;
        let memory = options
            .request_memory
            .as_ref()
            .map(|budget| {
                budget.reserve(
                    RequestMemoryComponent::SemanticPrelude,
                    decoded.body.len().saturating_mul(4),
                )
            })
            .transpose()
            .map_err(|error| error.into_proxy_error())?;
        let content = kiro::client_features::search_results(&decoded.body, &prepared.id)?;
        return Ok(KiroSearchResult {
            content,
            _memory: memory,
        });
    }
    Err(ProxyError::bad_gateway("Kiro search authentication failed"))
}

fn render_kiro_client_feature_json(
    options: &ClaudeKiroForwardOptions,
    template: &kiro_provider::CanonicalRequest,
    operation: kiro::client_features::Operation,
    message: KiroFeatureMessage,
) -> Result<Bytes, ProxyError> {
    let KiroFeatureMessage {
        value: message,
        _memory: _message_memory,
    } = message;
    let output_memory = options
        .request_memory
        .as_ref()
        .map(|budget| {
            budget.reserve(
                RequestMemoryComponent::NormalizedEvent,
                retained_json_bytes(&message).saturating_mul(4),
            )
        })
        .transpose()
        .map_err(|error| error.into_proxy_error())?;
    let message = if matches!(operation, kiro::client_features::Operation::Compact) {
        kiro::client_features::compact_response(&message)?
    } else {
        message
    };
    let bytes = Bytes::from(serde_json::to_vec(&message).map_err(ProxyError::bad_gateway)?);
    if matches!(operation, kiro::client_features::Operation::Compact) {
        return retain_kiro_feature_output(bytes, output_memory);
    }
    let bytes = template.adapter.transform_response_for_request(
        bytes,
        &options.stored,
        options.route,
        &template.runtime_request,
    )?;
    retain_kiro_feature_output(bytes, output_memory)
}

fn render_kiro_client_feature_stream(
    options: &ClaudeKiroForwardOptions,
    template: &kiro_provider::CanonicalRequest,
    operation: kiro::client_features::Operation,
    message: KiroFeatureMessage,
) -> Result<Bytes, ProxyError> {
    let KiroFeatureMessage {
        value: message,
        _memory: _message_memory,
    } = message;
    let output_memory = options
        .request_memory
        .as_ref()
        .map(|budget| {
            budget.reserve(
                RequestMemoryComponent::NormalizedEvent,
                retained_json_bytes(&message).saturating_mul(4),
            )
        })
        .transpose()
        .map_err(|error| error.into_proxy_error())?;
    if matches!(operation, kiro::client_features::Operation::Compact) {
        let response = kiro::client_features::compact_response(&message)?;
        let item = &response["output"][0];
        return retain_kiro_feature_output(Bytes::from(format!("event: response.created\ndata: {}\n\nevent: response.output_item.added\ndata: {}\n\nevent: response.output_item.done\ndata: {}\n\nevent: response.completed\ndata: {}\n\n",
            json!({"type":"response.created","sequence_number":0,"response":{"id":response["id"],"object":"response","model":response["model"],"created_at":response["created_at"],"status":"in_progress","output":[]}}),
            json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":item}),
            json!({"type":"response.output_item.done","sequence_number":2,"output_index":0,"item":item}),
            json!({"type":"response.completed","sequence_number":3,"response":response}))), output_memory);
    }
    let mut transform = super::stream_transforms::StreamEventTransformer::new_with_downstream_usage(
        &options.stored,
        options.route,
        template.runtime_request.responses_tool_context.clone(),
        template.runtime_request.downstream_include_usage,
    );
    let mut bytes = transform.push(kiro::client_features::message_sse(&message))?;
    bytes = join_bytes(bytes, transform.finish()?);
    retain_kiro_feature_output(bytes, output_memory)
}
