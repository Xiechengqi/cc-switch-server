// Dedicated Zen driver. Reuse shared protocol bridges, quota evidence and
// lifecycle settlement; no external repository is a runtime input.
struct OpencodeZenForwardOptions {
    state: ServerState,
    execution: ProviderExecution,
    stored: StoredProvider,
    route: ProxyRoute,
    body: Bytes,
    request_context: UsageLogContext,
    account_in_flight_guard: Option<AccountInFlightGuard>,
    share_invocation_guard: Option<ShareInFlightGuard>,
    started: Instant,
    timing: TimingSlot,
    request_memory: Option<RequestMemoryBudget>,
}

async fn forward_opencode_zen(options: OpencodeZenForwardOptions) -> Result<Response, ProxyError> {
    #[cfg(test)]
    if let Some(origin) = options
        .execution
        .plan
        .driver_options
        .get("testZenOrigin")
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        return forward_opencode_zen_at(options, &origin).await;
    }
    forward_opencode_zen_at(options, super::opencode_zen::ORIGIN).await
}

// Only the fixed-origin wrapper is called by production routing. Tests pass a
// loopback origin directly; no Provider setting can override the endpoint.
async fn forward_opencode_zen_at(
    options: OpencodeZenForwardOptions,
    origin: &str,
) -> Result<Response, ProxyError> {
    let state = options.state.clone();
    let stored = options.stored.clone();
    let mut context = options.request_context.clone();
    let started = options.started;
    let mut logged = false;
    let result = forward_opencode_zen_inner(options, origin, &mut logged).await;
    if !logged {
        if let Err(error) = &result {
            context.usage_state = Some(UsageState::NotApplicable);
            context.stream_status = Some(
                if error.status.is_client_error() {
                    "client_error"
                } else {
                    "failed"
                }
                .into(),
            );
            context.failure_kind = Some("zen_preflight_rejected".into());
            context.error_message = Some(error.client_message().to_string());
            log_usage(
                &state,
                &stored,
                error.status.as_u16(),
                started.elapsed().as_millis(),
                UsageModelMetadata::default(),
                TokenUsage::default(),
                context,
            )
            .await;
        }
    }
    result
}

async fn forward_opencode_zen_inner(
    options: OpencodeZenForwardOptions,
    origin: &str,
    logged: &mut bool,
) -> Result<Response, ProxyError> {
    use super::opencode_zen as zen;
    let OpencodeZenForwardOptions {
        state,
        execution,
        stored,
        route,
        body,
        mut request_context,
        account_in_flight_guard,
        share_invocation_guard,
        started,
        timing,
        request_memory,
    } = options;
    let total_deadline = tokio::time::Instant::from_std(started) + execution.request_timeout();
    let first_deadline = total_deadline.min(
        tokio::time::Instant::from_std(started)
            + execution
                .stream_first_byte_timeout()
                .unwrap_or(Duration::from_secs(60)),
    );
    if tokio::time::Instant::now() >= total_deadline {
        return Err(ProxyError {
            status: StatusCode::GATEWAY_TIMEOUT,
            message: "OpenCode Zen request deadline exceeded".into(),
        });
    }
    if !matches!(
        route,
        ProxyRoute::ClaudeMessages | ProxyRoute::CodexChatCompletions | ProxyRoute::CodexResponses
    ) {
        return Err(ProxyError::bad_request(
            "OpenCode Zen supports Messages, Chat and Responses only",
        ));
    }
    let budget = request_memory
        .unwrap_or_else(|| RequestMemoryBudget::new(state.request_body_limits.memory_budget_bytes));
    let _input_memory = budget
        .reserve(
            RequestMemoryComponent::NormalizedBody,
            super::request_memory::json_parse_working_bytes(&body).saturating_mul(3),
        )
        .map_err(|e| e.into_proxy_error())?;
    let mut routed = super::adapters::AdapterRequest {
        body,
        ..Default::default()
    };
    execution.enforce_model_policy(&mut routed)?;
    let mut requested: Value = serde_json::from_slice(&routed.body)
        .map_err(|_| ProxyError::bad_request("OpenCode Zen request must be a JSON object"))?;
    let map = requested
        .as_object_mut()
        .ok_or_else(|| ProxyError::bad_request("OpenCode Zen request must be a JSON object"))?;
    map.entry("stream")
        .or_insert(Value::Bool(request_context.is_streaming));
    if !requested["stream"].is_boolean() {
        return Err(ProxyError::bad_request("stream must be a boolean"));
    }
    let model = requested["model"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ProxyError::bad_request("OpenCode Zen model is required; select a current free model or configure a fixed upstream model"))?
        .to_string();
    // Reject unsupported Responses resource semantics rather than silently
    // dropping history or promising persistence that this driver cannot provide.
    if requested
        .get("previous_response_id")
        .is_some_and(|v| !v.is_null())
        || requested.get("store") == Some(&Value::Bool(true))
    {
        return Err(ProxyError::bad_request(
            "OpenCode Zen requires explicit history and store=false",
        ));
    }
    zen::validate_source_request(&requested)?;
    if zen::tool_choice_rejected(&requested) {
        return Err(ProxyError {
            status: http::StatusCode::FORBIDDEN,
            message: "OpenCode Zen rejects tool_choice none".into(),
        });
    }
    let client = state.http_client().await;
    let catalog = tokio::time::timeout_at(
        total_deadline,
        zen::refresh_model_catalog(&client, &state.config_dir, false),
    )
    .await
    .map_err(|_| ProxyError {
        status: StatusCode::GATEWAY_TIMEOUT,
        message: "OpenCode Zen catalog deadline exceeded".into(),
    })?
    .map_err(ProxyError::bad_gateway)?;
    let admitted = catalog.admit(&model).map_err(ProxyError::bad_request)?;
    let prepared = zen::prepare_for_model(&requested, route, &admitted)?;
    request_context.requested_reasoning_effort =
        zen::requested_effort(&requested).map(str::to_string);
    request_context.effective_reasoning_effort = request_context.requested_reasoning_effort.clone();
    ensure_share_model_available(
        &state,
        &execution,
        request_context.share_id.as_deref(),
        Some(&model),
    )
    .await?;
    // Provision/verify before marking a generation sent or reserving its USD
    // attempt. A runtime download failure is a preflight failure.
    #[cfg(test)]
    let loopback = origin.starts_with("http://127.0.0.1:") || origin.starts_with("http://[::1]:");
    #[cfg(not(test))]
    let loopback = false;
    if !loopback {
        zen::runtime::ensure(&state.config_dir, first_deadline).await?;
    }
    let wire = prepared.wire;
    let tool_context = super::transforms::responses_tool_context(&requested);
    let metadata = routed_model_metadata(
        routed.requested_model.as_deref().unwrap_or(&model),
        zen::base_model_id(&model),
        None,
        routed
            .actual_model_source
            .as_deref()
            .unwrap_or("opencode_zen.request"),
    );
    request_context.is_streaming = prepared.stream;
    request_context.usage_state = Some(UsageState::Pending);
    request_context.stream_status = prepared.stream.then(|| "pending".to_string());
    let payload_memory = budget
        .reserve(
            RequestMemoryComponent::NormalizedBody,
            prepared
                .body
                .len()
                .saturating_mul(2)
                .saturating_add(128 * 1024),
        )
        .map_err(|error| error.into_proxy_error())?;
    let request_id = log_usage(
        &state,
        &stored,
        200,
        started.elapsed().as_millis(),
        metadata,
        TokenUsage::default(),
        request_context.clone(),
    )
    .await;
    *logged = true;
    // Preserve one request identity. Each attempt owns its own evidence; the
    // public aggregate log carries a separate current-generation observation.
    let mut evidence_context = request_context.clone();
    evidence_context.request_id = Some(request_id.clone());
    let series = Arc::new(tokio::sync::Mutex::new(
        super::value_metering::ValueAttemptSeries::default(),
    ));
    let mut guard = ShareStreamInterruptGuard {
        armed: true,
        state: state.clone(),
        stored: stored.clone(),
        request_id: request_id.clone(),
        status_code: 200,
        share_id: request_context.share_id.clone(),
        user_email: request_context.user_email.clone(),
        quota_started_at_ms: request_context.started_at_ms,
        started,
        first_token_ms: None,
        usage: StreamUsageAccumulator::new(wire.semantics()),
    };
    let _owner = timing.owner_guard();

    if let Some(t) = timing.get() {
        budget.set_timing(t);
    }
    let probe_session = format!("probe:{}:{}", zen::WIRE_PROFILE, zen::base_model_id(&model));
    let conversation = if request_context.is_health_check {
        probe_session.as_str()
    } else {
        request_context.session_id.as_deref().unwrap_or(&request_id)
    };
    let seed = serde_json::to_string(&(
        zen::WIRE_PROFILE,
        stored.app.as_str(),
        stored.provider.id.as_str(),
        execution.plan.runtime_fingerprint.as_str(),
        request_context.share_id.as_deref(),
        request_context.user_email.as_deref(),
        conversation,
    ))
    .map_err(|_| ProxyError::bad_request("invalid Zen conversation scope"))?;
    let session = zen::session_for_conversation(&seed);
    let lane = match zen::acquire_lane(origin, &model, first_deadline).await {
        Ok(lane) => lane,
        Err(error) => {
            finish_qoder_stream_failure(&mut guard, route, &error).await;
            return Err(error);
        }
    };
    let result = async {
        if tokio::time::Instant::now() >= first_deadline {
            return Err(ProxyError {
                status: StatusCode::GATEWAY_TIMEOUT,
                message: "OpenCode Zen first-byte deadline exceeded before generation".into(),
            });
        }
        series
            .lock()
            .await
            .begin(
                &state,
                &evidence_context,
                zen::base_model_id(&model),
                &prepared.body,
                &execution.plan.runtime_fingerprint,
            )
            .await?;
        series.lock().await.sent();
        let mut response = zen_post(
            origin,
            &client,
            &state.config_dir,
            total_deadline,
            &prepared,
            &session,
            first_deadline,
            timing.get().as_ref(),
        )
        .await?;
        if !response.status().is_success() {
            if response.status() == StatusCode::UNAUTHORIZED {
                series.lock().await.auth_rejected().await;
            }
            let error = zen_http_error(&mut response, total_deadline).await;
            zen::observe_lane_error(origin, &model, &error);
            return Err(error);
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !content_type.starts_with("text/event-stream") {
            return Err(ProxyError::bad_gateway(
                "OpenCode Zen expected an SSE response",
            ));
        }
        if prepared.stream {
            return Ok((Some(response), None, false));
        }
        let (mut payload, _raw_memory) = zen_read_document(
            response,
            &stored,
            &prepared,
            &budget,
            &execution,
            started,
            total_deadline,
            timing.get(),
            &mut guard.usage,
            &mut guard.first_token_ms,
        )
        .await?;
        if wire == zen::ZenWire::Chat
            && zen::continuation_kind(&payload) == Some(zen::ContinuationKind::PureReasoning)
        {
            let mut candidate = payload.clone();
            candidate["choices"][0]["finish_reason"] = json!("stop");
            zen_validate_json(&candidate, wire)?;
        } else {
            zen_validate_json(&payload, wire)?;
        }
        zen::validate_client_tools(&payload, &prepared.client_tool_names)?;
        zen_observe_generation(&series, &guard.usage.snapshot(), &model).await?;
        if let Some(t) = timing.get() {
            match wire {
                zen::ZenWire::Chat => t.observe_chat_document(&payload),
                zen::ZenWire::Messages => t.observe_anthropic_value(&payload),
                zen::ZenWire::Responses => t.observe_responses_document(
                    &payload,
                    &super::response_semantics::classify_value(&payload),
                ),
            }
        }
        if wire == zen::ZenWire::Chat
            && guard.usage.snapshot().response_model.model.as_deref()
                == Some(zen::base_model_id(&model))
            && !guard.usage.snapshot().response_model.conflict
        {
            if let Some(kind) = zen::continuation_kind(&payload) {
                if let Some((next, continuation)) = zen_continuation_request(
                    &requested,
                    route,
                    &admitted,
                    kind,
                    &payload,
                    total_deadline,
                )? {
                    let continuation_result = async {
                        series
                            .lock()
                            .await
                            .begin(
                                &state,
                                &evidence_context,
                                zen::base_model_id(&model),
                                &continuation.body,
                                &execution.plan.runtime_fingerprint,
                            )
                            .await?;
                        state
                            .update_usage_log(&request_id, |log| {
                                log.attempt_count = 2;
                                log.value_generation_usage = Some(TokenUsage::default());
                                log.reset_router_sync_state();
                            })
                            .await
                            .map_err(|_| {
                                ProxyError::bad_gateway(
                                    "OpenCode Zen continuation evidence could not be persisted",
                                )
                            })?
                            .ok_or_else(|| {
                                ProxyError::bad_gateway(
                                    "OpenCode Zen request log disappeared before continuation",
                                )
                            })?;
                        series.lock().await.sent();
                        let continuation_started = Instant::now();
                        if let Some(t) = timing.begin_for_driver(
                            stored.provider_type,
                            execution.plan.driver_id.as_str(),
                            forward_timing_surface(route, None),
                            continuation_started,
                        ) {
                            budget.set_timing(t);
                        }
                        // Once a second generation starts, its failure is explicit;
                        // the public log still preserves first-generation usage.
                        let mut response = zen_post(
                            origin,
                            &client,
                            &state.config_dir,
                            total_deadline,
                            &continuation,
                            &session,
                            total_deadline.min(
                                tokio::time::Instant::from_std(continuation_started)
                                    + execution
                                        .stream_first_byte_timeout()
                                        .unwrap_or(Duration::from_secs(60)),
                            ),
                            timing.get().as_ref(),
                        )
                        .await?;
                        if !response.status().is_success() {
                            if response.status() == StatusCode::UNAUTHORIZED {
                                series.lock().await.auth_rejected().await;
                            }
                            let error = zen_http_error(&mut response, total_deadline).await;
                            zen::observe_lane_error(origin, &model, &error);
                            return Err(error);
                        }
                        let mut observer = StreamUsageAccumulator::new(wire.semantics());
                        let (mut continued, _memory) = zen_read_document(
                            response,
                            &stored,
                            &continuation,
                            &budget,
                            &execution,
                            continuation_started,
                            total_deadline,
                            timing.get(),
                            &mut observer,
                            &mut None,
                        )
                        .await?;
                        let second = observer.finish_with_status();
                        state
                            .update_usage_log(&request_id, |log| {
                                log.value_generation_usage = Some(second.usage);
                                log.reset_router_sync_state();
                            })
                            .await
                            .map_err(|_| {
                                ProxyError::bad_gateway(
                                    "OpenCode Zen generation evidence could not be persisted",
                                )
                            })?
                            .ok_or_else(|| {
                                ProxyError::bad_gateway(
                                    "OpenCode Zen request log disappeared during continuation",
                                )
                            })?;
                        zen_observe_generation(&series, &second, &model).await?;
                        let first = guard.usage.snapshot();
                        let mut merged = second.clone();
                        merged.usage = zen_add_usage(first.usage, second.usage);
                        merged.parse_error |= first.parse_error;
                        merged.streamed_text_bytes = merged
                            .streamed_text_bytes
                            .saturating_add(first.streamed_text_bytes);
                        guard.usage.replace_observation(merged);
                        zen_validate_json(&continued, wire)?;
                        zen::validate_client_tools(&continued, &continuation.client_tool_names)?;
                        zen_merge_continuation(&payload, &mut continued, kind)?;
                        Ok::<Value, ProxyError>(continued)
                    }
                    .await;
                    match continuation_result {
                        Ok(continued) => payload = continued,
                        Err(error) => {
                            update_terminal_usage_error(&state, &request_id, error.message.clone())
                                .await;
                            // Preserve a valid first segment. A reasoning-only
                            // cut has no valid answer envelope to return.
                            if kind != zen::ContinuationKind::MaxTokens
                                || zen_validate_json(&payload, wire).is_err()
                            {
                                return Err(error);
                            }
                        }
                    }
                    drop(next);
                }
            }
        }
        zen_validate_json(&payload, wire)?;
        let incomplete = zen_json_incomplete(&payload, wire);
        zen::restore_tool_names(&mut payload, &prepared.rename);
        let converted = zen::convert_response(&payload, wire, route, &tool_context)?;
        let encoded = serde_json::to_vec(&converted)
            .map_err(|_| ProxyError::bad_gateway("OpenCode Zen response encoding failed"))?;
        let bytes = budget
            .retain_bytes(
                RequestMemoryComponent::NormalizedEvent,
                Bytes::from(encoded),
            )
            .map_err(|e| e.into_proxy_error())?;
        Ok((None, Some(bytes), incomplete))
    }
    .await;
    let (upstream, json, incomplete) = match result {
        Ok(value) => value,
        Err(error) => {
            finish_qoder_stream_failure(&mut guard, route, &error).await;
            if let Some(t) = timing.get() {
                t.observe_terminal(TimingResult::Failed);
            }
            return Err(error);
        }
    };
    if let Some(json) = json {
        finish_qoder_stream_success(&mut guard).await;
        if incomplete {
            state
                .update_usage_log(&request_id, |log| {
                    log.stream_status = Some("incomplete".into());
                    log.reset_router_sync_state();
                })
                .await
                .ok();
        }
        if let Some(t) = timing.get() {
            t.observe_terminal(if incomplete {
                TimingResult::Incomplete
            } else {
                TimingResult::Complete
            });
        }
        return Ok(super::value_metering::series_response(
            Response::builder()
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(json))
                .map_err(|e| ProxyError::bad_gateway(e.to_string()))?,
            series,
        ));
    }
    let upstream =
        upstream.ok_or_else(|| ProxyError::bad_gateway("OpenCode Zen missing stream"))?;
    let include_usage = requested
        .pointer("/stream_options/include_usage")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut decoder = zen::ZenStream::new(
        &stored,
        route,
        wire,
        prepared.rename,
        tool_context,
        include_usage,
        timing.get(),
    )
    .with_client_tools(prepared.client_tool_names);
    let retained = match budget.reserve(
        RequestMemoryComponent::StreamRetainedState,
        decoder.retained_bytes(),
    ) {
        Ok(memory) => memory,
        Err(error) => {
            let error = error.into_proxy_error();
            finish_qoder_stream_failure(&mut guard, route, &error).await;
            return Err(error);
        }
    };
    let mut upstream = upstream.bytes_stream();
    // The guard is captured before the generator's first poll: even an unread
    // response settles cancellation and releases in-flight ownership.
    let stream = async_stream::stream! {
        let _owner = _owner;
        let _payload_memory = payload_memory;
        let _account = account_in_flight_guard;
        let _share = share_invocation_guard;
        let _lane = lane;
        let _series = series.clone();
        let mut idle_deadline = tokio::time::Instant::now() + execution.stream_idle_timeout().unwrap_or(Duration::from_secs(60));
        let mut heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + Duration::from_secs(15), Duration::from_secs(15));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let deadline = if decoder.semantic { total_deadline } else { total_deadline.min(first_deadline) };
            let read = tokio::select! {
                result = tokio::time::timeout_at(deadline.min(idle_deadline), upstream.next()) => result,
                _ = heartbeat.tick() => {
                    let handoff = tokio::time::Instant::now();
                    yield Ok::<Bytes, std::io::Error>(Bytes::from_static(b": keepalive\n\n"));
                    idle_deadline += handoff.elapsed();
                    continue;
                }
            };
            let result = match read {
                Ok(Some(Ok(chunk))) => {
                    idle_deadline = tokio::time::Instant::now() + execution.stream_idle_timeout().unwrap_or(Duration::from_secs(60));
                    let memory = budget.reserve(RequestMemoryComponent::TransportPending, chunk.len()).map_err(|e| e.into_proxy_error());
                    match memory {
                        Err(error) => Err(error),
                        Ok(_memory) => {
                            retained.resize(decoder.retained_bytes().saturating_add(guard.usage.retained_bytes()).saturating_add(decoder.working_bytes(&chunk)))
                                .map_err(|e| e.into_proxy_error()).and_then(|_| {
                                    guard.usage.push(&chunk);
                                    decoder.push(&chunk).map(|bytes| (bytes, false))
                                })
                        }
                    }
                }
                Ok(None) => decoder.finish().map(|bytes| (bytes, true)),
                Ok(Some(Err(_))) => Err(ProxyError::bad_gateway("OpenCode Zen stream transport failed")),
                Err(_) => Err(ProxyError { status: StatusCode::GATEWAY_TIMEOUT, message: "OpenCode Zen stream deadline exceeded".to_string() }),
            };
            let result = match result {
                Ok((bytes, done)) => retained.resize(decoder.retained_bytes().saturating_add(guard.usage.retained_bytes())).map_err(|e| e.into_proxy_error()).and_then(|_| {
                    budget.retain_bytes(RequestMemoryComponent::NormalizedEvent, bytes).map(|bytes| (bytes, done)).map_err(|e| e.into_proxy_error())
                }),
                Err(error) => Err(error),
            };
            if decoder.semantic && guard.first_token_ms.is_none() { guard.first_token_ms = Some(started.elapsed().as_millis()); }
            let result = match result {
                Ok(value) => match zen_observe_generation(&_series, &guard.usage.snapshot(), &model).await { Ok(()) => Ok(value), Err(error) => Err(error) },
                Err(error) => Err(error),
            };
            match result {
                Err(error) => {
                    if let Some(t) = timing.get() { t.observe_terminal(TimingResult::Failed); }
                    if let Some(frame) = finish_qoder_stream_failure(&mut guard, route, &error).await { yield Ok::<Bytes, std::io::Error>(frame); }
                    return;
                }
                Ok((bytes, done)) => {
                    if done {
                        finish_qoder_stream_success(&mut guard).await;
                        if decoder.incomplete {
                            state.update_usage_log(&request_id, |log| { log.stream_status = Some("incomplete".to_string()); log.reset_router_sync_state(); }).await.ok();
                        }
                        if let Some(t) = timing.get() { t.observe_terminal(if decoder.incomplete { TimingResult::Incomplete } else { TimingResult::Complete }); }
                    }
                    if !bytes.is_empty() {
                        let handoff = tokio::time::Instant::now();
                        yield Ok(bytes);
                        idle_deadline += handoff.elapsed();
                    }
                    if done { return; }
                }
            }
        }
    };
    Response::builder()
        .header(CONTENT_TYPE, "text/event-stream")
        .header(CACHE_CONTROL, "no-cache, no-transform")
        .body(Body::from_stream(stream))
        .map_err(|e| ProxyError::bad_gateway(e.to_string()))
}

fn zen_status_error(status: StatusCode, headers: &HeaderMap) -> ProxyError {
    // Never quote provider bodies or HTML in logs/client errors. Keep only a
    // bounded request ID for correlation and a validated retry delay hint.
    let request_id = ["x-request-id", "request-id", "cf-ray"]
        .iter()
        .find_map(|name| headers.get(*name).and_then(|v| v.to_str().ok()))
        .filter(|id| {
            id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        });
    let retry = headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.parse::<u64>().ok().or_else(|| {
                httpdate::parse_http_date(v).ok().map(|date| {
                    date.duration_since(std::time::SystemTime::now())
                        .unwrap_or_default()
                        .as_secs()
                })
            })
        })
        .map(|v| v.min(86400));
    let kind = super::opencode_zen::classify_upstream_status(status.as_u16());
    let mut error = ProxyError {
        status: if status == StatusCode::TOO_MANY_REQUESTS
            || status == StatusCode::FORBIDDEN
            || status == StatusCode::UNAUTHORIZED
        {
            status
        } else {
            StatusCode::BAD_GATEWAY
        },
        message: format!(
            "OpenCode Zen upstream {kind}: HTTP {}{}",
            status.as_u16(),
            request_id
                .map(|id| format!("; request_id={id}"))
                .unwrap_or_default()
        ),
    };
    if let Some(seconds) = retry {
        error.message = format!(
            "{}{}]{}",
            ProxyError::RETRY_AFTER_PREFIX,
            seconds,
            error.message
        );
    }
    error
}

#[allow(clippy::too_many_arguments)]
async fn zen_post(
    origin: &str,
    _client: &reqwest::Client,
    config_dir: &std::path::Path,
    total_deadline: tokio::time::Instant,
    prepared: &super::opencode_zen::PreparedUpstream,
    session: &str,
    deadline: tokio::time::Instant,
    timing: Option<&ForwardTiming>,
) -> Result<super::opencode_zen::transport::ZenResponse, ProxyError> {
    if let Some(t) = timing {
        t.mark(Milestone::SendStarted);
    }
    #[cfg(test)]
    if origin.starts_with("http://127.0.0.1:") || origin.starts_with("http://[::1]:") {
        let mut request = _client
            .post(format!("{origin}{}", prepared.path))
            .body(prepared.body.clone());
        for (name, value) in super::opencode_zen::gateway_headers(session, true) {
            request = request.header(name, value);
        }
        let response = tokio::time::timeout_at(deadline, request.send())
            .await
            .map_err(|_| ProxyError {
                status: StatusCode::GATEWAY_TIMEOUT,
                message: "OpenCode Zen header deadline exceeded".into(),
            })?
            .map_err(|_| ProxyError::bad_gateway("OpenCode Zen upstream transport failed"))?;
        if let Some(t) = timing {
            t.mark(Milestone::Headers);
        }
        return Ok(super::opencode_zen::transport::ZenResponse::from_reqwest(
            response,
        ));
    }
    let executable = super::opencode_zen::runtime::ensure(config_dir, deadline).await?;
    let response = super::opencode_zen::transport::post(
        &executable,
        &format!("{origin}{}", prepared.path),
        &prepared.body,
        session,
        true,
        total_deadline,
        deadline,
    )
    .await?;
    if let Some(t) = timing {
        t.mark(Milestone::Headers);
    }
    Ok(response)
}

#[allow(clippy::too_many_arguments)]
async fn zen_read_document(
    response: super::opencode_zen::transport::ZenResponse,
    stored: &StoredProvider,
    prepared: &super::opencode_zen::PreparedUpstream,
    budget: &RequestMemoryBudget,
    execution: &ProviderExecution,
    started: Instant,
    deadline: tokio::time::Instant,
    timing: Option<ForwardTiming>,
    usage: &mut StreamUsageAccumulator,
    first_token_ms: &mut Option<u128>,
) -> Result<(Value, RequestMemoryReservation), ProxyError> {
    use super::opencode_zen as zen;
    if !response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"))
    {
        return Err(ProxyError::bad_gateway(
            "OpenCode Zen expected an SSE response",
        ));
    }
    let mut decoder = zen::ZenStream::new(
        stored,
        ProxyRoute::CodexChatCompletions,
        prepared.wire,
        Default::default(),
        Default::default(),
        true,
        timing,
    )
    .with_client_tools(prepared.client_tool_names.clone())
    .with_aggregation();
    let retained = budget
        .reserve(
            RequestMemoryComponent::StreamRetainedState,
            decoder.retained_bytes(),
        )
        .map_err(|e| e.into_proxy_error())?;
    let first_deadline = tokio::time::Instant::from_std(started)
        + execution
            .stream_first_byte_timeout()
            .unwrap_or(Duration::from_secs(60));
    let idle = execution
        .stream_idle_timeout()
        .unwrap_or(Duration::from_secs(60));
    let mut idle_deadline = tokio::time::Instant::now() + idle;
    let mut stream = response.bytes_stream();
    loop {
        let read_deadline = if decoder.semantic {
            deadline
        } else {
            deadline.min(first_deadline)
        };
        let chunk = tokio::time::timeout_at(read_deadline.min(idle_deadline), stream.next())
            .await
            .map_err(|_| ProxyError {
                status: StatusCode::GATEWAY_TIMEOUT,
                message: "OpenCode Zen stream deadline exceeded".into(),
            })?;
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk?;
        idle_deadline = tokio::time::Instant::now() + idle;
        let _pending = budget
            .reserve(RequestMemoryComponent::TransportPending, chunk.len())
            .map_err(|e| e.into_proxy_error())?;
        retained
            .resize(
                decoder
                    .retained_bytes()
                    .saturating_add(usage.retained_bytes())
                    .saturating_add(decoder.working_bytes(&chunk)),
            )
            .map_err(|e| e.into_proxy_error())?;
        usage.push(&chunk);
        decoder.push(&chunk)?;
        if decoder.semantic && first_token_ms.is_none() {
            *first_token_ms = Some(started.elapsed().as_millis());
        }
        retained
            .resize(
                decoder
                    .retained_bytes()
                    .saturating_add(usage.retained_bytes()),
            )
            .map_err(|e| e.into_proxy_error())?;
    }
    let payload = decoder.finish_document()?;
    let argument_working = payload
        .pointer("/choices/0/message/tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|call| call.pointer("/function/arguments").and_then(Value::as_str))
        .map(|args| super::request_memory::json_parse_working_bytes(args.as_bytes()))
        .sum::<usize>();
    retained
        .resize(
            super::request_memory::retained_json_bytes(&payload)
                .saturating_mul(4)
                .saturating_add(argument_working),
        )
        .map_err(|e| e.into_proxy_error())?;
    Ok((payload, retained))
}

fn zen_validate_json(
    payload: &Value,
    wire: super::opencode_zen::ZenWire,
) -> Result<(), ProxyError> {
    super::opencode_zen::validate_json(payload, wire)
}

fn zen_continuation_request(
    requested: &Value,
    route: ProxyRoute,
    admitted: &super::opencode_zen::ZenAdmission,
    kind: super::opencode_zen::ContinuationKind,
    payload: &Value,
    deadline: tokio::time::Instant,
) -> Result<Option<(Value, super::opencode_zen::PreparedUpstream)>, ProxyError> {
    use super::opencode_zen as zen;
    if deadline.saturating_duration_since(tokio::time::Instant::now()) < Duration::from_secs(1) {
        return Ok(None);
    }
    // A new schema-constrained document or a user stop sequence is not a
    // suffix of a truncated answer. Preserve the original incomplete result.
    if [
        requested.get("response_format"),
        requested.pointer("/text/format"),
        requested.pointer("/output_config/format"),
    ]
    .into_iter()
    .flatten()
    .any(|f| f["type"] != "text")
        || requested.get("stop").is_some()
        || requested.get("stop_sequences").is_some()
        || requested.get("tool_choice").is_some_and(|c| {
            c.as_str() != Some("auto") && c.get("type").and_then(Value::as_str) != Some("auto")
        })
    {
        return Ok(None);
    }
    let canonical = match route {
        ProxyRoute::ClaudeMessages => super::transforms::anthropic_to_openai_chat(requested),
        ProxyRoute::CodexResponses => super::transforms::openai_responses_to_chat(requested),
        _ => Ok(requested.clone()),
    }
    .map_err(|e| ProxyError::bad_request(e.to_string()))?;
    let total = zen::output_limit(requested, Some(admitted))?;
    let used = payload
        .pointer("/usage/completion_tokens")
        .and_then(Value::as_u64);
    // Without an actual usage receipt, no second generation is authorized by
    // an invented remaining token budget.
    let Some(used) = used else {
        return Ok(None);
    };
    let remaining = total.saturating_sub(used).min(8192);
    if remaining < 512 {
        return Ok(None);
    }
    let Some(messages) = zen::continuation_messages(&canonical, kind, payload) else {
        return Ok(None);
    };
    // Local conservative context guard; do not claim an unmeasured model window.
    if serde_json::to_vec(&messages).map_or(true, |bytes| bytes.len() > 128 * 1024) {
        return Ok(None);
    }
    let mut next = canonical;
    let map = next
        .as_object_mut()
        .ok_or_else(|| ProxyError::bad_request("invalid Zen continuation request"))?;
    for key in ["max_tokens", "max_output_tokens", "max_completion_tokens"] {
        map.remove(key);
    }
    map.insert("model".into(), requested["model"].clone());
    map.insert("stream".into(), Value::Bool(false));
    map.insert("messages".into(), messages);
    map.insert("max_completion_tokens".into(), remaining.into());
    // Preserve reviewed controls from the ingress, then run the same capability
    // gate and fixed endpoint as generation one. No second catalog read.
    for key in zen::control_keys() {
        next.as_object_mut().unwrap().remove(*key);
    }
    for key in zen::control_keys() {
        if let Some(value) = requested.get(*key) {
            next[*key] = value.clone();
        }
    }
    let Ok(prepared) = zen::prepare_for_model(&next, ProxyRoute::CodexChatCompletions, admitted)
    else {
        // The optional continuation may no longer satisfy a thinking/output
        // budget. No second call has started; keep the valid first segment.
        return Ok(None);
    };
    Ok(Some((next, prepared)))
}

fn zen_merge_continuation(
    first: &Value,
    next: &mut Value,
    kind: super::opencode_zen::ContinuationKind,
) -> Result<(), ProxyError> {
    if next
        .pointer("/choices/0/message/tool_calls")
        .and_then(Value::as_array)
        .is_some_and(|calls| !calls.is_empty())
    {
        return Err(ProxyError::bad_gateway(
            "OpenCode Zen continuation unexpectedly called a tool",
        ));
    }
    if next
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .is_none_or(|text| text.trim().is_empty())
    {
        return Err(ProxyError::bad_gateway(
            "OpenCode Zen continuation produced no answer",
        ));
    }
    if kind == super::opencode_zen::ContinuationKind::MaxTokens {
        let prefix = first
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .unwrap_or("");
        let suffix = next
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .unwrap_or("");
        next["choices"][0]["message"]["content"] = Value::String(format!("{prefix}{suffix}"));
    }
    // Client usage covers both generations. Ledger evidence is observed
    // separately per attempt before this presentation-only merge.
    if let (Some(first), Some(next)) = (
        first.get("usage").and_then(Value::as_object),
        next.get_mut("usage").and_then(Value::as_object_mut),
    ) {
        fn add(first: &serde_json::Map<String, Value>, next: &mut serde_json::Map<String, Value>) {
            for (key, value) in first {
                if let Some(n) = value.as_u64() {
                    next.insert(
                        key.clone(),
                        Value::from(
                            n.saturating_add(next.get(key).and_then(Value::as_u64).unwrap_or(0)),
                        ),
                    );
                } else if let (Some(a), Some(b)) = (
                    value.as_object(),
                    next.get_mut(key).and_then(Value::as_object_mut),
                ) {
                    add(a, b);
                }
            }
        }
        add(first, next);
    }
    Ok(())
}

fn zen_add_usage(a: TokenUsage, b: TokenUsage) -> TokenUsage {
    fn add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
        match (a, b) {
            (None, None) => None,
            _ => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
        }
    }
    TokenUsage {
        raw_input_tokens: add(a.raw_input_tokens, b.raw_input_tokens),
        input_tokens: add(a.input_tokens, b.input_tokens),
        output_tokens: add(a.output_tokens, b.output_tokens),
        cache_read_tokens: add(a.cache_read_tokens, b.cache_read_tokens),
        cache_creation_tokens: add(a.cache_creation_tokens, b.cache_creation_tokens),
        cache_creation_details: match (a.cache_creation_details, b.cache_creation_details) {
            (Some(a), Some(b)) => Some(crate::domain::usage::store::CacheCreationDetails {
                five_minutes: a.five_minutes.saturating_add(b.five_minutes),
                one_hour: a.one_hour.saturating_add(b.one_hour),
            }),
            (None, None) => None,
            _ => None,
        },
        total_tokens: add(a.total_tokens, b.total_tokens),
        credit_usage: match (a.credit_usage, b.credit_usage) {
            (None, None) => None,
            _ => Some(a.credit_usage.unwrap_or(0.0) + b.credit_usage.unwrap_or(0.0)),
        },
    }
}

fn zen_json_incomplete(payload: &Value, wire: super::opencode_zen::ZenWire) -> bool {
    match wire {
        super::opencode_zen::ZenWire::Chat => payload
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
            .is_some_and(|s| matches!(s, "length" | "content_filter")),
        super::opencode_zen::ZenWire::Responses => payload["status"] == "incomplete",
        super::opencode_zen::ZenWire::Messages => payload
            .get("stop_reason")
            .and_then(Value::as_str)
            .is_some_and(|s| matches!(s, "max_tokens" | "pause_turn")),
    }
}

async fn zen_observe_generation(
    series: &Arc<tokio::sync::Mutex<super::value_metering::ValueAttemptSeries>>,
    observed: &super::streaming::StreamUsageResult,
    requested_model: &str,
) -> Result<(), ProxyError> {
    if observed.parse_error
        || observed.response_model.conflict
        || observed.response_model.service_tier_conflict
        || observed
            .response_model
            .model
            .as_deref()
            .is_none_or(|model| model != super::opencode_zen::base_model_id(requested_model))
    {
        // A missing/conflicting actual model is not proof of a cheaper result.
        return Ok(());
    }
    series
        .lock()
        .await
        .observe(observed.usage, observed.usage_estimated)
        .await
}

async fn zen_http_error(
    response: &mut super::opencode_zen::transport::ZenResponse,
    deadline: tokio::time::Instant,
) -> ProxyError {
    let mut error = zen_status_error(response.status(), response.headers());
    if let Ok(Ok(body)) = tokio::time::timeout_at(deadline, response.read_limited(16 * 1024)).await
    {
        let lower = String::from_utf8_lossy(&body).to_ascii_lowercase();
        let category = if lower.contains("freeusagelimiterror") {
            Some("free_usage_limit")
        } else if lower.contains("freetiererror") {
            Some("free_tier_gate")
        } else if lower.trim_start().starts_with("<!doctype html")
            || lower.trim_start().starts_with("<html")
        {
            Some("front_proxy_refused")
        } else if lower.contains("regionerror")
            || lower.contains("unsupported_country")
            || lower.contains("region_not_supported")
        {
            Some("region_rejected")
        } else {
            None
        };
        let structured = serde_json::from_slice::<Value>(&body).ok();
        let code = structured
            .as_ref()
            .and_then(|v| {
                v.pointer("/error/code")
                    .or_else(|| v.pointer("/error/type"))
            })
            .and_then(Value::as_str);
        let client_category = match code {
            Some("context_length_exceeded" | "context_window_exceeded" | "max_tokens_exceeded") => {
                Some("context_limit")
            }
            Some("unsupported_parameter" | "invalid_parameter" | "invalid_request_error") => {
                Some("parameter_rejected")
            }
            _ => None,
        };
        if category.is_none()
            && matches!(response.status().as_u16(), 400 | 413 | 422)
            && client_category.is_some()
        {
            error.status = StatusCode::BAD_REQUEST;
        }
        if let Some(category) = category.or(client_category) {
            error.message.push_str(&format!("; category={category}"));
        }
    }
    error
}
