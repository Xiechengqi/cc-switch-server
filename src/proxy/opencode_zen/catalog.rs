//! One bounded Zen listing drives management discovery and generation admission.
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    Mutex,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    base_model_id, gateway_headers, legacy_free_id, legacy_wire_for, now_ms,
    session_for_conversation, ZenWire, ORIGIN,
};

pub const CATALOG_PATH: &str = "/zen/v1/models";
const FRESH_MS: i64 = 8 * 60 * 1000;
const MAX_STALE_MS: i64 = 24 * 60 * 60 * 1000;
const REFRESH_INTERVAL_MS: i64 = 30_000;
const MAX_MODELS: usize = 512;
const MAX_BYTES: usize = 1024 * 1024;

// These are reviewed fixed routes. A listing never becomes an arbitrary URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CatalogEndpoint {
    LegacyChat,
    LegacyResponses,
    LegacyMessages,
    Chat,
    Responses,
    Messages,
}

impl CatalogEndpoint {
    pub(super) fn wire(self) -> ZenWire {
        match self {
            Self::LegacyChat | Self::Chat => ZenWire::Chat,
            Self::LegacyResponses | Self::Responses => ZenWire::Responses,
            Self::LegacyMessages | Self::Messages => ZenWire::Messages,
        }
    }

    pub(super) fn path(self) -> &'static str {
        match self {
            Self::LegacyChat => "/inference/v1/chat/completions",
            Self::LegacyResponses => "/inference/v1/responses",
            Self::LegacyMessages => "/inference/v1/messages",
            Self::Chat => "/inference/openai/v1/chat/completions",
            Self::Responses => "/inference/openai/v1/responses",
            Self::Messages => "/inference/anthropic/v1/messages",
        }
    }

    pub(super) fn legacy(id: &str) -> Self {
        match legacy_wire_for(id) {
            ZenWire::Chat => Self::LegacyChat,
            ZenWire::Responses => Self::LegacyResponses,
            ZenWire::Messages => Self::LegacyMessages,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Pricing {
    Free,
    Paid,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PricingSource {
    UpstreamDeclared,
    LegacyId,
    Unspecified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogModel {
    id: String,
    name: String,
    model_type: String,
    pricing: Pricing,
    pricing_source: PricingSource,
    endpoint: Option<CatalogEndpoint>,
    context_window: Option<u64>,
    max_output_tokens: Option<u64>,
}

impl CatalogModel {
    fn unavailable_reason(&self) -> Option<&'static str> {
        if self.model_type != "chat"
            || self
                .id
                .rsplit('/')
                .next()
                .unwrap_or(&self.id)
                .starts_with("jev")
        {
            Some("unsupported_model_type")
        } else if self.pricing == Pricing::Paid {
            Some("paid_model")
        } else if self.pricing == Pricing::Unknown {
            Some("pricing_unknown")
        } else if self.endpoint.is_none() {
            Some("unsupported_protocol")
        } else {
            None
        }
    }

    fn view(&self) -> ZenModelView {
        ZenModelView {
            id: self.id.clone(),
            upstream_model: self.id.clone(),
            display_name: self.name.clone(),
            model_type: self.model_type.clone(),
            pricing: self.pricing,
            pricing_source: self.pricing_source,
            selectable: self.unavailable_reason().is_none(),
            unavailable_reason: self.unavailable_reason(),
            protocol: self.endpoint.map(CatalogEndpoint::wire),
            context_window: self.context_window,
            max_output_tokens: self.max_output_tokens,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZenModelView {
    id: String,
    upstream_model: String,
    display_name: String,
    model_type: String,
    pricing: Pricing,
    pricing_source: PricingSource,
    selectable: bool,
    unavailable_reason: Option<&'static str>,
    protocol: Option<ZenWire>,
    context_window: Option<u64>,
    max_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ZenModelCatalogView {
    source: &'static str,
    fetched_at_ms: i64,
    stale: bool,
    pub models: Vec<ZenModelView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogSnapshot {
    schema_version: u32,
    fetched_at_ms: i64,
    models: Vec<CatalogModel>,
}

impl CatalogSnapshot {
    fn usable(&self, now: i64) -> bool {
        (0..=MAX_STALE_MS).contains(&now.saturating_sub(self.fetched_at_ms))
    }

    #[cfg(test)]
    fn ids(&self) -> Vec<String> {
        self.models
            .iter()
            .filter(|m| m.unavailable_reason().is_none())
            .map(|m| m.id.clone())
            .collect()
    }

    fn view(&self) -> ZenModelCatalogView {
        ZenModelCatalogView {
            source: "opencode_zen_live_model_catalog",
            fetched_at_ms: self.fetched_at_ms,
            stale: now_ms().saturating_sub(self.fetched_at_ms) >= FRESH_MS
                || LAST_FAILED.load(Ordering::Relaxed),
            models: self.models.iter().map(CatalogModel::view).collect(),
        }
    }
}

static CATALOG: Mutex<Option<CatalogSnapshot>> = Mutex::new(None);
static FETCH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static RETRY_AT: AtomicI64 = AtomicI64::new(0);
static LAST_ATTEMPT: AtomicI64 = AtomicI64::new(0);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static LAST_FAILED: AtomicBool = AtomicBool::new(false);

fn snapshot() -> Option<CatalogSnapshot> {
    CATALOG.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

fn model(id: &str) -> Option<CatalogModel> {
    let guard = CATALOG.lock().unwrap_or_else(|p| p.into_inner());
    guard
        .as_ref()?
        .models
        .iter()
        .find(|m| m.id == base_model_id(id))
        .cloned()
}

#[cfg(test)]
pub(super) fn catalog_endpoint(id: &str) -> Option<CatalogEndpoint> {
    model(id)?.endpoint
}
#[cfg(test)]
pub(super) fn catalog_free_status(id: &str) -> Option<bool> {
    Some(model(id)?.pricing == Pricing::Free)
}
#[cfg(test)]
pub fn current_catalog() -> Option<Vec<String>> {
    snapshot().map(|s| s.ids())
}
pub fn catalog_fetched_at_ms() -> Option<i64> {
    snapshot().map(|s| s.fetched_at_ms)
}
pub fn catalog_is_stale() -> bool {
    snapshot().is_none_or(|s| s.view().stale)
}

pub fn catalog_metadata(id: &str) -> Value {
    let Some(m) = model(id) else {
        return json!({});
    };
    let mut out = serde_json::Map::new();
    if let Some(n) = m.context_window {
        out.insert("context_window".into(), n.into());
    }
    if let Some(n) = m.max_output_tokens {
        out.insert("max_output_tokens".into(), n.into());
    }
    Value::Object(out)
}

pub fn generation_endpoint(id: &str) -> Result<CatalogEndpoint, &'static str> {
    let s = snapshot().ok_or("OpenCode Zen catalog is unavailable")?;
    if !s.usable(now_ms()) {
        return Err("OpenCode Zen catalog has expired");
    }
    let m = s
        .models
        .iter()
        .find(|m| m.id == base_model_id(id))
        .ok_or("OpenCode Zen catalog does not list this free-lane model")?;
    if m.unavailable_reason().is_some() {
        return Err("OpenCode Zen only forwards supported free-lane models");
    }
    m.endpoint
        .ok_or("OpenCode Zen model protocol is unsupported")
}

#[cfg(test)]
pub fn generation_allowed(id: &str) -> Result<(), &'static str> {
    generation_endpoint(id).map(|_| ())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/' | b':'))
}

fn limit(value: Option<&Value>) -> Option<u64> {
    value?.as_u64().filter(|n| *n > 0 && *n <= 100_000_000)
}

// Preserve unknown/invalid pricing as unknown; never turn a missing price into zero.
// Every declared text tier/cache/mode price must be zero, with an
// explicit zero base input AND output. A positive price always wins over the ID.
fn pricing(row: &Value, id: &str) -> (Pricing, PricingSource) {
    let Some(cost) = row.get("cost") else {
        let legacy_format = row.is_string()
            || (row.get("api").is_none()
                && row.get("type").is_none()
                && row.get("limit").is_none());
        return if legacy_format && legacy_free_id(id) {
            (Pricing::Free, PricingSource::LegacyId)
        } else {
            (Pricing::Unknown, PricingSource::Unspecified)
        };
    };
    fn price_state(value: &Value, paid: &mut bool, invalid: &mut bool) {
        match value {
            Value::Number(n) => {
                let n = n.as_f64().unwrap_or(f64::NAN);
                if n > 0.0 {
                    *paid = true;
                } else if n != 0.0 {
                    *invalid = true;
                }
            }
            Value::Object(map) => {
                for (k, v) in map {
                    // IDs, conditions and multiplier factors aren't token prices.
                    if k == "when" || k == "tier" || k == "multipliers" {
                        continue;
                    }
                    if k == "unit" {
                        if v.as_str() != Some("token") {
                            *invalid = true;
                        }
                        continue;
                    }
                    price_state(v, paid, invalid);
                }
            }
            Value::Array(rows) => {
                for v in rows {
                    price_state(v, paid, invalid);
                }
            }
            _ => {
                *invalid = true;
            }
        }
    }
    let text = cost.get("text").unwrap_or(cost);
    let input = text
        .get("input")
        .and_then(|v| v.get("price").or(Some(v)))
        .and_then(Value::as_f64);
    let output = text
        .get("output")
        .and_then(|v| v.get("price").or(Some(v)))
        .and_then(Value::as_f64);
    let mut paid = false;
    let mut invalid = false;
    price_state(text, &mut paid, &mut invalid);
    if let Some(modes) = row
        .get("experimental")
        .and_then(|v| v.get("modes"))
        .and_then(Value::as_object)
    {
        for mode in modes.values() {
            if let Some(cost) = mode.get("cost") {
                price_state(cost.get("text").unwrap_or(cost), &mut paid, &mut invalid);
            }
        }
    }
    (
        if paid {
            Pricing::Paid
        } else if !invalid && input == Some(0.0) && output == Some(0.0) {
            Pricing::Free
        } else {
            Pricing::Unknown
        },
        PricingSource::UpstreamDeclared,
    )
}

fn endpoint(row: &Value, id: &str) -> Option<CatalogEndpoint> {
    let Some(api) = row.get("api") else {
        return Some(CatalogEndpoint::legacy(id));
    };
    let api = api.as_object()?;
    for (key, modern, old) in [
        (
            "chat-completions",
            CatalogEndpoint::Chat,
            CatalogEndpoint::LegacyChat,
        ),
        (
            "responses",
            CatalogEndpoint::Responses,
            CatalogEndpoint::LegacyResponses,
        ),
        (
            "messages",
            CatalogEndpoint::Messages,
            CatalogEndpoint::LegacyMessages,
        ),
    ] {
        let Some(entry) = api.get(key) else {
            continue;
        };
        let base = entry.get("base_url").and_then(Value::as_str).unwrap_or("");
        if base == "https://opencode.ai/inference/v1" {
            return Some(old);
        }
        if matches!(
            (key, base),
            (
                "chat-completions" | "responses",
                "https://opencode.ai/inference/openai/v1"
            ) | ("messages", "https://opencode.ai/inference/anthropic/v1")
        ) {
            return Some(modern);
        }
    }
    None
}

fn parse_model(row: &Value) -> Option<CatalogModel> {
    let id = row.as_str().or_else(|| row.get("id")?.as_str())?.trim();
    if !valid_id(id) {
        return None;
    }
    let name = row
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
        .unwrap_or(id);
    let kind = match row.get("type") {
        None => "chat",
        Some(value) => value.as_str().unwrap_or("unknown"),
    };
    let (pricing, pricing_source) = pricing(row, id);
    Some(CatalogModel {
        id: id.into(),
        name: name.into(),
        model_type: kind.chars().take(32).collect(),
        pricing,
        pricing_source,
        endpoint: endpoint(row, id),
        context_window: limit(row.get("context_window"))
            .or_else(|| limit(row.pointer("/limit/context"))),
        max_output_tokens: limit(row.get("max_output_tokens"))
            .or_else(|| limit(row.pointer("/limit/output"))),
    })
}

fn parse_catalog(payload: &Value, fetched_at_ms: i64) -> Result<CatalogSnapshot, String> {
    let rows = payload
        .get("data")
        .or_else(|| payload.get("models"))
        .unwrap_or(payload)
        .as_array()
        .ok_or("OpenCode Zen catalog has an unsupported shape")?;
    if rows.len() > MAX_MODELS {
        return Err("OpenCode Zen catalog exceeds 512 models".into());
    }
    let mut models: BTreeMap<String, CatalogModel> = BTreeMap::new();
    for row in rows {
        let Some(m) = parse_model(row) else {
            return Err("OpenCode Zen catalog has an invalid model".into());
        };
        if models.get(&m.id).is_some_and(|previous| previous != &m) {
            return Err("OpenCode Zen catalog has conflicting model IDs".into());
        }
        models.insert(m.id.clone(), m);
    }
    // A successful empty roster is authoritative, unlike a failed refresh.
    Ok(CatalogSnapshot {
        schema_version: 2,
        fetched_at_ms,
        models: models.into_values().collect(),
    })
}

fn load_catalog_cache(path: &std::path::Path) -> Option<CatalogSnapshot> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_BYTES {
        return None;
    }
    let raw: Value = serde_json::from_slice(&bytes).ok()?;
    let snapshot =
        if raw.get("schema_version").is_some() {
            let snapshot: CatalogSnapshot = serde_json::from_value(raw).ok()?;
            if snapshot.schema_version != 2 || snapshot.models.len() > MAX_MODELS {
                return None;
            }
            let mut ids = std::collections::BTreeSet::new();
            for m in &snapshot.models {
                if !valid_id(&m.id)
                    || !ids.insert(&m.id)
                    || m.name.len() > 256
                    || m.name.chars().any(char::is_control)
                    || m.model_type.len() > 32
                    || m.context_window.is_some_and(|n| n == 0 || n > 100_000_000)
                    || m.max_output_tokens
                        .is_some_and(|n| n == 0 || n > 100_000_000)
                    || (m.pricing_source == PricingSource::LegacyId
                        && (!legacy_free_id(&m.id) || m.pricing != Pricing::Free))
                {
                    return None;
                }
            }
            snapshot
        } else {
            let ids = raw.get("ids")?.as_array()?;
            if ids.len() > MAX_MODELS {
                return None;
            }
            let rows: Vec<Value> = ids.iter().filter_map(|id| {
            let id = id.as_str()?;
            if !valid_id(id) || !legacy_free_id(id) { return None; }
            let metadata = raw.get("metadata").and_then(|v| v.get(id));
            Some(json!({"id":id, "context_window":metadata.and_then(|v| v.get("context_window")),
                "max_output_tokens":metadata.and_then(|v| v.get("max_output_tokens"))}))
        }).collect();
            if rows.is_empty() {
                return None;
            }
            parse_catalog(&json!(rows), raw.get("fetched_at_ms")?.as_i64()?).ok()?
        };
    snapshot.usable(now_ms()).then_some(snapshot)
}

pub async fn refresh_catalog(
    client: &reqwest::Client,
    config_dir: &std::path::Path,
) -> Result<Vec<String>, String> {
    refresh_model_catalog(client, config_dir, false)
        .await
        .map(|view| {
            view.models
                .into_iter()
                .filter(|m| m.selectable)
                .map(|m| m.id)
                .collect()
        })
}

pub async fn refresh_model_catalog(
    client: &reqwest::Client,
    config_dir: &std::path::Path,
    force: bool,
) -> Result<ZenModelCatalogView, String> {
    refresh_model_catalog_at(client, config_dir, force, ORIGIN).await
}

async fn refresh_model_catalog_at(
    client: &reqwest::Client,
    config_dir: &std::path::Path,
    force: bool,
    origin: &str,
) -> Result<ZenModelCatalogView, String> {
    let observed = GENERATION.load(Ordering::Relaxed);
    let _flight = FETCH.lock().await;
    let cache_path = config_dir.join("opencode-zen-catalog.json");
    if snapshot().is_none() {
        let path = cache_path.clone();
        if let Ok(Some(s)) = tokio::task::spawn_blocking(move || load_catalog_cache(&path)).await {
            *CATALOG.lock().unwrap_or_else(|p| p.into_inner()) = Some(s);
        }
    }
    let cached = || snapshot().filter(|s| s.usable(now_ms())).map(|s| s.view());
    if GENERATION.load(Ordering::Relaxed) != observed || (!force && !catalog_is_stale()) {
        if let Some(view) = cached() {
            return Ok(view);
        }
    }
    let now = now_ms();
    if now < RETRY_AT.load(Ordering::Relaxed)
        || (force && now.saturating_sub(LAST_ATTEMPT.load(Ordering::Relaxed)) < REFRESH_INTERVAL_MS)
    {
        return cached().ok_or_else(|| "OpenCode Zen catalog refresh is cooling down".into());
    }
    LAST_ATTEMPT.store(now, Ordering::Relaxed);
    match fetch_catalog(client, origin).await {
        Ok(s) => {
            RETRY_AT.store(0, Ordering::Relaxed);
            LAST_FAILED.store(false, Ordering::Relaxed);
            let view = s.view();
            *CATALOG.lock().unwrap_or_else(|p| p.into_inner()) = Some(s.clone());
            GENERATION.fetch_add(1, Ordering::Relaxed);
            // Cache IO is best effort; the upstream success is already accepted.
            let _ = tokio::task::spawn_blocking(move || {
                crate::infra::storage::write_json_pretty(&cache_path, &s)
            })
            .await;
            Ok(view)
        }
        Err(e) => {
            LAST_FAILED.store(true, Ordering::Relaxed);
            RETRY_AT.store(now_ms() + REFRESH_INTERVAL_MS, Ordering::Relaxed);
            cached().ok_or(e)
        }
    }
}

async fn fetch_catalog(client: &reqwest::Client, origin: &str) -> Result<CatalogSnapshot, String> {
    let session = session_for_conversation("catalog:opencode-zen");
    let mut request = client
        .get(format!("{origin}{CATALOG_PATH}"))
        .timeout(std::time::Duration::from_secs(15));
    for (name, value) in gateway_headers(&session, false) {
        if name != "content-type" {
            request = request.header(name, value);
        }
    }
    let mut response = request
        .send()
        .await
        .map_err(|_| "OpenCode Zen catalog transport failed".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "OpenCode Zen catalog {}: HTTP {}",
            super::classify_upstream_status(response.status().as_u16()),
            response.status().as_u16()
        ));
    }
    let bytes = crate::infra::http::read_response_body_limited(&mut response, MAX_BYTES)
        .await
        .map_err(|_| "OpenCode Zen catalog body invalid or exceeds 1 MiB".to_string())?;
    let payload = serde_json::from_slice(&bytes)
        .map_err(|_| "OpenCode Zen catalog invalid JSON".to_string())?;
    parse_catalog(&payload, now_ms())
}

#[cfg(test)]
pub(crate) fn clear_catalog() {
    *CATALOG.lock().unwrap_or_else(|p| p.into_inner()) = None;
    RETRY_AT.store(0, Ordering::Relaxed);
    LAST_ATTEMPT.store(0, Ordering::Relaxed);
    LAST_FAILED.store(false, Ordering::Relaxed);
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
pub(crate) fn store_catalog(ids: Vec<String>, fetched_at_ms: i64) -> Vec<String> {
    let s = parse_catalog(&json!(ids), fetched_at_ms).unwrap();
    let ids = s.ids();
    *CATALOG.lock().unwrap_or_else(|p| p.into_inner()) = Some(s);
    ids
}

#[cfg(test)]
pub(crate) fn store_catalog_payload(payload: &Value) {
    *CATALOG.lock().unwrap_or_else(|p| p.into_inner()) =
        Some(parse_catalog(payload, now_ms()).unwrap());
    LAST_FAILED.store(false, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::super::CATALOG_TEST_LOCK;
    use super::*;

    fn zero_model(id: &str) -> Value {
        json!({"id":id,"name":"A free model","type":"chat", "limit":{"context":200000,"output":32000},
            "cost":{"text":{"input":{"price":0,"cache":{"read":0}},"output":{"price":0}}},
            "api":{"chat-completions":{"base_url":"https://opencode.ai/inference/openai/v1"}}})
    }

    #[test]
    fn opencode_zen_catalog_classifies_prices_without_suffix_or_missing_price_guesses() {
        let mut paid = zero_model("looks-free");
        paid["cost"]["text"]["output"]["price"] = json!(1);
        let mut partial = zero_model("partial-free");
        partial["cost"]["text"]
            .as_object_mut()
            .unwrap()
            .remove("output");
        let mut tiered = zero_model("tiered-free");
        tiered["cost"]["text"]["input"]["overrides"] =
            json!([{"when":{"context":{"gt":200000}},"price":0.1}]);
        let mut cached = zero_model("cache-free");
        cached["cost"]["text"]["input"]["cache"]["write"] = json!({"1h":0.1});
        let mut mode = zero_model("mode-free");
        mode["experimental"] = json!({"modes":{"fast":{"cost":{"input":0,"output":0.2}}}});
        let mut invalid = zero_model("invalid-free");
        invalid["cost"]["text"]["input"]["price"] = json!(-1);
        let mut missing = zero_model("missing-cost-free");
        missing.as_object_mut().unwrap().remove("cost");
        for row in [paid, tiered, cached, mode] {
            assert_eq!(parse_model(&row).unwrap().pricing, Pricing::Paid);
        }
        for row in [
            partial,
            invalid,
            missing,
            json!({"id":"unknown-free","cost":{}}),
        ] {
            let model = parse_model(&row).unwrap();
            assert_eq!(model.pricing, Pricing::Unknown);
            assert_eq!(model.unavailable_reason(), Some("pricing_unknown"));
        }
        for row in [
            zero_model("big-pickle"),
            json!({"id":"old-zero","cost":{"input":0,"output":0,"cache_read":0}}),
        ] {
            let model = parse_model(&row).unwrap();
            assert_eq!(model.pricing, Pricing::Free);
            assert_eq!(model.pricing_source, PricingSource::UpstreamDeclared);
            assert!(model.unavailable_reason().is_none());
        }
        let old = parse_model(&json!("old-free")).unwrap();
        assert_eq!(old.pricing_source, PricingSource::LegacyId);
    }

    #[test]
    fn opencode_zen_catalog_unifies_discovery_admission_and_reviewed_protocols() {
        let _lock = CATALOG_TEST_LOCK.blocking_lock();
        clear_catalog();
        let mut image = zero_model("image-free");
        image["type"] = json!("image");
        let mut foreign = zero_model("foreign-free");
        foreign["api"]["chat-completions"]["base_url"] = json!("https://arbitrary.invalid/v1");
        let mut messages = zero_model("new-messages-free");
        messages["api"] =
            json!({"messages":{"base_url":"https://opencode.ai/inference/anthropic/v1"}});
        let mut paid = zero_model("paid-free");
        paid["cost"]["text"]["input"]["price"] = json!(2);
        store_catalog_payload(
            &json!({"data":[zero_model("big-pickle"),image,foreign,messages,paid]}),
        );
        assert_eq!(
            current_catalog().unwrap(),
            ["big-pickle", "new-messages-free"]
        );
        assert!(generation_allowed("big-pickle").is_ok());
        assert!(generation_allowed("new-messages-free(high)").is_ok());
        for id in ["image-free", "foreign-free", "paid-free", "missing-free"] {
            assert!(generation_allowed(id).is_err());
        }
        assert!(!super::super::is_free_model("paid-free"));
        assert_eq!(
            super::super::endpoint_for("big-pickle"),
            "/inference/openai/v1/chat/completions"
        );
        assert_eq!(
            super::super::wire_for("new-messages-free"),
            ZenWire::Messages
        );
        assert_eq!(
            catalog_metadata("big-pickle"),
            json!({"context_window":200000,"max_output_tokens":32000})
        );
        assert_eq!(
            serde_json::to_value(snapshot().unwrap().view()).unwrap()["models"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
        // One authoritative empty success withdraws the previous roster.
        store_catalog_payload(&json!({"data":[]}));
        assert_eq!(current_catalog(), Some(vec![]));
        assert!(generation_allowed("big-pickle").is_err());
        clear_catalog();
    }

    #[test]
    fn opencode_zen_catalog_rejects_conflicts_invalid_ids_and_oversized_rosters() {
        let free = zero_model("one-free");
        let mut paid = free.clone();
        paid["cost"]["text"]["output"]["price"] = json!(1);
        for data in [
            json!({"data":[free,paid]}),
            json!({"data":[{"id":"invalid id-free"}]}),
            json!({"unexpected":[]}),
            json!({"data":vec![json!("one-free");513]}),
        ] {
            assert!(parse_catalog(&data, now_ms()).is_err());
        }
    }

    #[test]
    fn opencode_zen_disk_cache_accepts_recent_success_only() {
        let path = std::env::temp_dir().join(format!(
            "zen-catalog-{}.json",
            session_for_conversation("catalog-disk-test")
        ));
        let mut old = json!({"ids":["chat-free","paid","jev-free"],"fetched_at_ms":now_ms(),"metadata":{"chat-free":{"context_window":32768}}});
        crate::infra::storage::write_json_pretty(&path, &old).unwrap();
        let loaded = load_catalog_cache(&path).unwrap();
        assert_eq!(loaded.ids(), ["chat-free"]);
        assert_eq!(loaded.models[0].context_window, Some(32768));
        let modern = parse_catalog(&json!([zero_model("big-pickle")]), now_ms()).unwrap();
        crate::infra::storage::write_json_pretty(&path, &modern).unwrap();
        assert_eq!(load_catalog_cache(&path).unwrap().ids(), ["big-pickle"]);
        for time in [now_ms() - MAX_STALE_MS - 1, now_ms() + 10000] {
            old["fetched_at_ms"] = json!(time);
            crate::infra::storage::write_json_pretty(&path, &old).unwrap();
            assert!(load_catalog_cache(&path).is_none());
        }
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn opencode_zen_cold_catalog_refresh_is_single_flight_with_backoff() {
        use futures_util::StreamExt;
        use std::sync::{atomic::AtomicUsize, Arc};
        let _lock = CATALOG_TEST_LOCK.lock().await;
        clear_catalog();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let router = axum::Router::new().fallback(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async { http::StatusCode::BAD_GATEWAY }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = crate::infra::http::test_outbound_client_builder_with_proxy(&proxy)
            .unwrap()
            .build()
            .unwrap();
        let directory = std::env::temp_dir().join(format!("zen-catalog-flight-{}", now_ms()));
        let mut requests = futures_util::stream::FuturesUnordered::new();
        for _ in 0..16 {
            requests.push(refresh_catalog(&client, &directory));
        }
        while let Some(result) = requests.next().await {
            assert!(result.is_err());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // A failed forced refresh marks even a fresh saved snapshot stale, retains
        // its time and contents, and the next caller obeys the same backoff.
        store_catalog_payload(&json!([zero_model("big-pickle")]));
        LAST_ATTEMPT.store(0, Ordering::Relaxed);
        RETRY_AT.store(0, Ordering::Relaxed);
        let before = catalog_fetched_at_ms();
        let view = refresh_model_catalog(&client, &directory, true)
            .await
            .unwrap();
        assert!(view.stale);
        assert_eq!(view.fetched_at_ms, before.unwrap());
        assert_eq!(current_catalog().unwrap(), ["big-pickle"]);
        assert!(
            refresh_model_catalog(&client, &directory, true)
                .await
                .unwrap()
                .stale
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        clear_catalog();
        server.abort();
    }

    #[tokio::test]
    async fn opencode_zen_force_refresh_coalesces_persists_and_withdraws_empty_rosters() {
        use futures_util::StreamExt;
        use std::sync::{atomic::AtomicUsize, Arc};
        let _lock = CATALOG_TEST_LOCK.lock().await;
        clear_catalog();
        let calls = Arc::new(AtomicUsize::new(0));
        let data = Arc::new(Mutex::new(json!({"data":[zero_model("big-pickle")]})));
        let count = calls.clone();
        let upstream_data = data.clone();
        let router = axum::Router::new().route(
            CATALOG_PATH,
            axum::routing::get(move |headers: axum::http::HeaderMap| {
                assert_eq!(headers["authorization"], "Bearer public");
                count.fetch_add(1, Ordering::SeqCst);
                let body = upstream_data.lock().unwrap().clone();
                async move { axum::Json(body) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let directory = std::env::temp_dir().join(format!("zen-catalog-success-{}", now_ms()));
        refresh_model_catalog_at(&client, &directory, false, &origin)
            .await
            .unwrap();
        refresh_model_catalog_at(&client, &directory, false, &origin)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        LAST_ATTEMPT.store(0, Ordering::Relaxed);
        let mut pending = futures_util::stream::FuturesUnordered::new();
        for _ in 0..16 {
            pending.push(refresh_model_catalog_at(&client, &directory, true, &origin));
        }
        while let Some(result) = pending.next().await {
            assert!(!result.unwrap().stale);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            load_catalog_cache(&directory.join("opencode-zen-catalog.json"))
                .unwrap()
                .ids(),
            ["big-pickle"]
        );
        *data.lock().unwrap() = json!({"data":[]});
        LAST_ATTEMPT.store(0, Ordering::Relaxed);
        assert!(refresh_model_catalog_at(&client, &directory, true, &origin)
            .await
            .unwrap()
            .models
            .is_empty());
        assert!(generation_allowed("big-pickle").is_err());
        assert!(
            load_catalog_cache(&directory.join("opencode-zen-catalog.json"))
                .unwrap()
                .models
                .is_empty()
        );
        clear_catalog();
        server.abort();
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
