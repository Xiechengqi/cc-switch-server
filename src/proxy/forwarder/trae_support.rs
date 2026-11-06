/// Resolve Responses history before the generic adapter removes its response id.
/// The existing bounded process cache owns TTL, capacity and opaque-id validation.
async fn prepare_trae_continuation(
    state: &ServerState,
    execution: &ProviderExecution,
    context: &UsageLogContext,
    body: Bytes,
    memory: Option<&RequestMemoryBudget>,
) -> Result<(Bytes, Option<TraeContinuation>), ProxyError> {
    use sha2::{Digest, Sha256};
    let (mut value, _parsed) = parse_previous_response_working_value(&body, memory)?;
    if value
        .get("previous_response_id")
        .is_some_and(|value| !value.is_null() && !value.is_string())
    {
        return Err(ProxyError::bad_request(
            "previous_response_id must be a string",
        ));
    }
    // The cache injection helper accepts arrays. Normalize shorthand before
    // expanding so a continuation never loses its current input.
    if let Some(Value::String(text)) = value.get("input") {
        value["input"] = json!([{"role":"user","content":[{"type":"input_text","text":text}]}]);
    } else if value.get("input").is_some_and(Value::is_object) {
        let item = value.get_mut("input").unwrap().take();
        value["input"] = json!([item]);
    }
    let (previous, _id) = owned_previous_response_id_with_memory(&value, memory)?;
    let (provider_type, account_id, generation) = execution
        .managed_account_identity_target()
        .ok_or_else(|| ProxyError::bad_request("Trae requires an explicit account binding"))?;
    let (account, incarnation) = state
        .find_account_for_provider_with_incarnation(provider_type, account_id)
        .await
        .ok_or_else(|| ProxyError::not_found("Trae account not found"))?;
    if account.auth_identity_generation != generation {
        return Err(ProxyError::conflict("Trae account identity changed"));
    }
    let enabled = if let Some(share_id) = &context.share_id {
        state
            .shares
            .read()
            .await
            .get(share_id)
            .is_some_and(|share| share.previous_response_cache_enabled)
    } else {
        true
    };
    if !enabled {
        if previous.is_some() {
            return Err(ProxyError::response_context_unavailable());
        }
        return Ok((body, None));
    }
    let profile = crate::domain::trae::TraeAccountProfile::parse(account.profile.as_ref())
        .map_err(ProxyError::bad_request)?;
    let scope = PreviousResponseCacheScope {
        share_id: context
            .share_id
            .clone()
            .unwrap_or_else(|| "trae:local".into()),
        principal: context
            .user_email
            .as_deref()
            .unwrap_or("server-local")
            .trim()
            .to_ascii_lowercase(),
        provider_id: execution.stored.provider.id.clone(),
        provider_revision: execution.plan.provider_revision,
        runtime_fingerprint: execution.plan.runtime_fingerprint.clone(),
        account_id: account_id.into(),
        account_incarnation: incarnation,
        auth_identity_generation: generation,
        // A legitimate refresh does not change the conversation's verified owner.
        // Credentials are never replayed from this cache. Rebinding/recreation does.
        token_refresh_generation: 0,
        credential_fingerprint: Sha256::digest(
            serde_json::to_vec(&(provider_type, &profile)).map_err(ProxyError::bad_gateway)?,
        )
        .into(),
        workspace_id: format!("trae:{}:{}", profile.uid, profile.enterprise_id),
    };
    if previous
        .as_deref()
        .is_some_and(|id| id.trim() != id || id.is_empty() || id.len() > 512)
    {
        return Err(ProxyError::bad_request(
            "previous_response_id must be an exact nonempty opaque id",
        ));
    }
    // Codex's general cache only replays tool pairs. Trae has no upstream
    // conversation store, so it must retain the complete message history.
    let mut injection_memory = None;
    if let Some(previous) = previous.as_deref() {
        let current_vector_bytes = value
            .get("input")
            .and_then(Value::as_array)
            .map_or(0, |items| {
                items.len().saturating_mul(std::mem::size_of::<Value>() * 2)
            });
        let lookup = state
            .previous_response_context_lookup_with(
                &scope,
                previous,
                crate::infra::time::now_ms().min(i64::MAX as u128) as i64,
                |cached| {
                    let reservation = trae_reserve(
                        memory,
                        RequestMemoryComponent::NormalizedBody,
                        cached.iter().fold(
                            8192usize.saturating_add(current_vector_bytes),
                            |bytes, item| {
                                bytes.saturating_add(retained_json_bytes(item).saturating_mul(2))
                            },
                        ),
                    )?;
                    Ok::<_, ProxyError>((cached.to_vec(), reservation))
                },
            )
            .items
            .transpose()?;
        let Some((mut cached, reservation)) = lookup else {
            state.record_previous_response_context_unavailable();
            return Err(ProxyError::response_context_unavailable());
        };
        let current = value
            .get_mut("input")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                ProxyError::bad_request("Trae Responses input must be text, an item, or an array")
            })?;
        cached.append(current);
        value["input"] = Value::Array(cached);
        injection_memory = reservation;
    }
    // Instructions apply to this response only, as in the Responses API.
    if let Some(object) = value.as_object_mut() {
        object.remove("previous_response_id");
    }
    let context_memory = trae_reserve(
        memory,
        RequestMemoryComponent::NormalizedBody,
        retained_json_bytes(&value)
            .saturating_mul(2)
            .saturating_add(8192),
    )?;
    let items = value
        .get("input")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let (encoded, encoded_memory) = encode_previous_response_working_value(&value, memory)?;
    let encoded = if let Some(budget) = memory {
        budget
            .retain_bytes(RequestMemoryComponent::NormalizedBody, Bytes::from(encoded))
            .map_err(|error| error.into_proxy_error())
    } else {
        Ok(Bytes::from(encoded))
    }?;
    drop(encoded_memory);
    drop(injection_memory);
    Ok((
        encoded,
        Some(TraeContinuation {
            state: state.clone(),
            scope,
            items,
            request_memory: memory.cloned(),
            _memory: context_memory,
        }),
    ))
}

struct TraeContinuation {
    state: ServerState,
    scope: PreviousResponseCacheScope,
    items: Vec<Value>,
    request_memory: Option<RequestMemoryBudget>,
    _memory: Option<RequestMemoryReservation>,
}
impl TraeContinuation {
    fn store(&self, canonical: &Value) -> Result<(), ProxyError> {
        let _working = trae_reserve(
            self.request_memory.as_ref(),
            RequestMemoryComponent::NormalizedEvent,
            self.items.iter().fold(
                retained_json_bytes(canonical)
                    .saturating_mul(8)
                    .saturating_add(8192),
                |bytes, item| bytes.saturating_add(retained_json_bytes(item).saturating_mul(2)),
            ),
        )?;
        let response = super::transforms::openai_chat_response_to_responses(canonical)
            .map_err(ProxyError::bad_gateway)?;
        if response.get("status").and_then(Value::as_str) != Some("completed") {
            return Ok(());
        }
        let id = response.get("id").and_then(Value::as_str).ok_or_else(|| {
            ProxyError::bad_gateway("Trae Responses completion is missing its id")
        })?;
        let mut items = self.items.clone();
        for item in response
            .get("output")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let mut item = item.clone();
            if let Some(object) = item.as_object_mut() {
                object.remove("id");
                object.remove("encrypted_content");
                object.remove("encryptedContent");
            }
            items.push(item);
        }
        self.state.cache_previous_response_context(
            self.scope.clone(),
            id,
            items,
            crate::infra::time::now_ms().min(i64::MAX as u128) as i64,
        );
        Ok(())
    }
}

fn trae_reserve(
    memory: Option<&RequestMemoryBudget>,
    component: RequestMemoryComponent,
    bytes: usize,
) -> Result<Option<RequestMemoryReservation>, ProxyError> {
    memory
        .map(|budget| {
            budget
                .reserve(component, bytes)
                .map_err(|error| error.into_proxy_error())
        })
        .transpose()
}

fn trae_transform(
    transformer: &mut super::stream_transforms::StreamEventTransformer,
    chunk: Bytes,
    terminal: bool,
    memory: Option<&RequestMemoryBudget>,
    resident: &Option<RequestMemoryReservation>,
) -> Result<Bytes, ProxyError> {
    let _working = trae_reserve(
        memory,
        RequestMemoryComponent::NormalizedEvent,
        chunk
            .len()
            .saturating_mul(64)
            .saturating_add(transformer.retained_bytes().saturating_mul(4))
            .saturating_add(8192),
    )?;
    let mut output = transformer.push(chunk)?;
    if terminal {
        output = join_bytes(output, transformer.finish()?);
    }
    if let Some(resident) = resident {
        resident
            .resize(transformer.retained_bytes())
            .map_err(|error| error.into_proxy_error())?;
    }
    memory.map_or(Ok(output.clone()), |budget| {
        budget
            .retain_bytes(RequestMemoryComponent::NormalizedEvent, output)
            .map_err(|error| error.into_proxy_error())
    })
}

fn trae_store_continuation(
    context: Option<&TraeContinuation>,
    decoder: &super::trae::TraeSseDecoder,
    memory: Option<&RequestMemoryBudget>,
) -> Result<(), ProxyError> {
    if let Some(context) = context {
        let _working = trae_reserve(
            memory,
            RequestMemoryComponent::NormalizedEvent,
            decoder.retained_bytes().saturating_mul(8),
        )?;
        context.store(&decoder.snapshot_completion()?)?;
    }
    Ok(())
}

fn trae_keepalive_frame(route: ProxyRoute) -> Bytes {
    if route == ProxyRoute::Gemini {
        // SSE comments are transport-only for Gemini as for OpenAI.
        Bytes::from_static(b": keepalive\n\n")
    } else {
        kiro_provider::downstream_keepalive_frame(route)
    }
}
