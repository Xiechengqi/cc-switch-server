//! Tenant-only conversation over an existing workspace Share. Use the normal
//! inference path so provider selection, user grants and quota accounting agree.
use super::*;
use serde::Serialize;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Turn {
    share_id: String,
    app: AppKind,
    model: String,
    prompt: String,
    reply: String,
}

fn read_history(state: &ServerState) -> Result<Vec<Turn>, ApiError> {
    match std::fs::read(state.config_dir.join("workspace-chat.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(ApiError::internal),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(ApiError::internal(e)),
    }
}
pub(in crate::api) async fn history(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    super::tenant(&state, &headers).await?;
    let options=state.shares.read().await.shares.iter().filter(|s|s.enabled).map(|s|
        json!({"id":s.id,"name":s.display_name.as_deref().unwrap_or(&s.id),"apps":s.bindings.iter().map(|b|b.app).collect::<Vec<_>>()})
    ).collect::<Vec<_>>();
    Ok(Json(
        json!({"turns":read_history(&state)?,"shares":options}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::api) struct Message {
    share_id: String,
    app: AppKind,
    model: String,
    prompt: String,
}

pub(in crate::api) async fn send(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<Message>,
) -> Result<Json<Value>, ApiError> {
    let grant = super::tenant(&state, &headers).await?;
    if input.prompt.trim().is_empty()
        || input.prompt.len() > 8192
        || input.model.is_empty()
        || input.model.len() > 200
        || !input
            .model
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-._:/".contains(&c))
    {
        return Err(ApiError::bad_request("invalid chat message or model"));
    }
    let _chat = state
        .market_workspace
        .chat
        .try_lock()
        .map_err(|_| ApiError::conflict("chat request in progress"))?;
    let allowed = state
        .shares
        .read()
        .await
        .get(&input.share_id)
        .is_some_and(|s| s.enabled && s.bindings.iter().any(|b| b.app == input.app));
    if !allowed {
        return Err(ApiError::conflict(
            "select an enabled workspace Share binding",
        ));
    }
    let mut turns = read_history(&state)?;
    let mut messages = Vec::new();
    for turn in turns
        .iter()
        .filter(|t| t.share_id == input.share_id && t.app == input.app && t.model == input.model)
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        messages.push(json!({"role":"user","content":turn.prompt}));
        messages.push(json!({"role":"assistant","content":turn.reply}));
    }
    messages.push(json!({"role":"user","content":input.prompt}));
    let (route, path, body) = match input.app {
        AppKind::Claude => (
            ProxyRoute::ClaudeMessages,
            None,
            json!({"model":input.model,"max_tokens":2048,"messages":messages}),
        ),
        AppKind::Codex => (
            ProxyRoute::CodexChatCompletions,
            None,
            json!({"model":input.model,"messages":messages,"stream":false}),
        ),
        AppKind::Gemini => {
            let model = input.model.strip_prefix("models/").unwrap_or(&input.model);
            if model.contains(['/', ':']) {
                return Err(ApiError::bad_request("invalid Gemini model"));
            }
            let contents=messages.iter().map(|m|json!({"role":if m["role"]=="assistant" {"model"}else{"user"},"parts":[{"text":m["content"]}]})).collect::<Vec<_>>();
            (
                ProxyRoute::Gemini,
                Some(format!("models/{model}:generateContent")),
                json!({"contents":contents}),
            )
        }
    };
    let mut upstream = HeaderMap::new();
    upstream.insert(
        "x-cc-switch-user-email",
        grant.tenant_email.parse().map_err(ApiError::internal)?,
    );
    upstream.insert(
        "x-cc-switch-share-id",
        input.share_id.parse().map_err(ApiError::bad_request)?,
    );
    upstream.insert("content-type", HeaderValue::from_static("application/json"));
    let response = proxy::forward(
        state.clone(),
        route,
        path,
        upstream,
        Bytes::from(body.to_string()),
    )
    .await
    .map_err(|_| ApiError::conflict("workspace Share inference failed"))?;
    if !response.status().is_success() {
        return Err(ApiError::conflict("workspace Share returned an error"));
    }
    let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .map_err(ApiError::internal)?;
    let result: Value =
        serde_json::from_slice(&bytes).map_err(|_| ApiError::conflict("invalid chat response"))?;
    let reply = match input.app {
        AppKind::Codex => result
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        _ => {
            let parts = if input.app == AppKind::Claude {
                result.get("content")
            } else {
                result.pointer("/candidates/0/content/parts")
            };
            parts
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|v| v["text"].as_str())
                .collect::<String>()
        }
    };
    if reply.is_empty() {
        return Err(ApiError::conflict("no text in chat response"));
    }
    super::tenant(&state, &headers).await?;
    turns.push(Turn {
        share_id: input.share_id,
        app: input.app,
        model: input.model,
        prompt: input.prompt,
        reply: reply.chars().take(16384).collect(),
    });
    turns.drain(..turns.len().saturating_sub(64));
    crate::infra::storage::write_json_pretty(&state.config_dir.join("workspace-chat.json"), &turns)
        .map_err(ApiError::internal)?;
    Ok(Json(json!({"turns":turns})))
}
