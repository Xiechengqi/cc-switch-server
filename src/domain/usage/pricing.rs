use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::providers::model::Provider;
use crate::domain::usage::store::TokenUsage;

const MODEL_PRICING_FILE_NAME: &str = "model-pricing.json";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricing {
    #[serde(default)]
    pub input_usd_per_million: Option<f64>,
    #[serde(default)]
    pub output_usd_per_million: Option<f64>,
    #[serde(default)]
    pub cache_read_usd_per_million: Option<f64>,
    #[serde(default)]
    pub cache_creation_usd_per_million: Option<f64>,
    #[serde(default)]
    pub cost_multiplier: Option<f64>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CostBreakdown {
    pub cost_multiplier: Option<f64>,
    pub input_cost_usd: Option<f64>,
    pub output_cost_usd: Option<f64>,
    pub cache_read_cost_usd: Option<f64>,
    pub cache_creation_cost_usd: Option<f64>,
    pub total_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricingStore {
    #[serde(default)]
    pub models: Vec<ModelPricingEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricingEntry {
    pub model_id: String,
    pub display_name: String,
    pub input_cost_per_million: String,
    pub output_cost_per_million: String,
    pub cache_read_cost_per_million: String,
    pub cache_creation_cost_per_million: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateModelPricingInput {
    #[serde(default)]
    pub model_id: Option<String>,
    pub display_name: String,
    pub input_cost_per_million: String,
    pub output_cost_per_million: String,
    pub cache_read_cost_per_million: String,
    pub cache_creation_cost_per_million: String,
}

pub fn pricing_for_model(provider: &Provider, pricing_model: Option<&str>) -> Option<ModelPricing> {
    pricing_for_model_with_store(provider, None, pricing_model)
}

pub fn pricing_for_model_with_store(
    provider: &Provider,
    pricing_store: Option<&ModelPricingStore>,
    pricing_model: Option<&str>,
) -> Option<ModelPricing> {
    let provider_multiplier = provider_cost_multiplier(provider);
    let provider_pricing = provider
        .settings_config
        .get("pricing")
        .or_else(|| provider.settings_config.get("modelPricing"));

    if let Some(pricing) = provider_pricing {
        if let Some(model) = pricing_model {
            if let Some(value) = pricing
                .pointer(&format!("/models/{model}"))
                .or_else(|| pricing.get(model))
            {
                return parse_pricing(value)
                    .map(|pricing| with_provider_multiplier(pricing, provider_multiplier));
            }
        }

        if let Some(pricing) = pricing
            .get("default")
            .or_else(|| pricing.get("*"))
            .and_then(parse_pricing)
            .or_else(|| parse_pricing(pricing))
        {
            return Some(with_provider_multiplier(pricing, provider_multiplier));
        }
    }

    pricing_model
        .and_then(|model| {
            pricing_store
                .and_then(|store| store.pricing_for_model(model))
                .or_else(|| default_pricing_for_model(model))
        })
        .map(|pricing| with_provider_multiplier(pricing, provider_multiplier))
}

pub fn calculate_cost(usage: TokenUsage, pricing: ModelPricing) -> CostBreakdown {
    let multiplier = pricing.cost_multiplier.unwrap_or(1.0);
    let billable_input_tokens = usage.billed_input_tokens.or(usage.input_tokens);
    let input_cost = token_cost(
        billable_input_tokens,
        pricing.input_usd_per_million,
        multiplier,
    );
    let output_cost = token_cost(
        usage.output_tokens,
        pricing.output_usd_per_million,
        multiplier,
    );
    let cache_read_cost = token_cost(
        usage.cache_read_tokens,
        pricing.cache_read_usd_per_million,
        multiplier,
    );
    let cache_creation_cost = token_cost(
        usage.cache_creation_tokens,
        pricing.cache_creation_usd_per_million,
        multiplier,
    );
    let total = [
        input_cost,
        output_cost,
        cache_read_cost,
        cache_creation_cost,
    ]
    .into_iter()
    .flatten()
    .sum::<f64>();
    let has_any_cost = input_cost.is_some()
        || output_cost.is_some()
        || cache_read_cost.is_some()
        || cache_creation_cost.is_some();

    CostBreakdown {
        cost_multiplier: Some(multiplier),
        input_cost_usd: input_cost,
        output_cost_usd: output_cost,
        cache_read_cost_usd: cache_read_cost,
        cache_creation_cost_usd: cache_creation_cost,
        total_cost_usd: has_any_cost.then_some(total),
    }
}

fn token_cost(tokens: Option<u64>, usd_per_million: Option<f64>, multiplier: f64) -> Option<f64> {
    Some(tokens? as f64 / 1_000_000.0 * usd_per_million? * multiplier)
}

impl ModelPricingStore {
    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let path = model_pricing_path(config_dir);
        let mut store = if path.exists() {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("read model pricing {}", path.display()))?;
            serde_json::from_str(&content)
                .with_context(|| format!("parse model pricing {}", path.display()))?
        } else {
            Self::default()
        };
        store.ensure_seeded();
        Ok(store)
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(config_dir)
            .with_context(|| format!("create config dir {}", config_dir.display()))?;
        let path = model_pricing_path(config_dir);
        crate::infra::storage::write_json_pretty(&path, self)
            .with_context(|| format!("write model pricing {}", path.display()))
    }

    pub fn list(&self) -> Vec<ModelPricingEntry> {
        let mut models = self.models.clone();
        models.sort_by(|left, right| {
            left.display_name
                .to_ascii_lowercase()
                .cmp(&right.display_name.to_ascii_lowercase())
                .then(left.model_id.cmp(&right.model_id))
        });
        models
    }

    pub fn upsert(
        &mut self,
        model_id: String,
        input: UpdateModelPricingInput,
    ) -> Result<ModelPricingEntry, String> {
        let entry = ModelPricingEntry {
            model_id: normalize_model_pricing_entry_id(&input.model_id.unwrap_or(model_id)),
            display_name: input.display_name.trim().to_string(),
            input_cost_per_million: normalize_price_string(&input.input_cost_per_million)?,
            output_cost_per_million: normalize_price_string(&input.output_cost_per_million)?,
            cache_read_cost_per_million: normalize_price_string(
                &input.cache_read_cost_per_million,
            )?,
            cache_creation_cost_per_million: normalize_price_string(
                &input.cache_creation_cost_per_million,
            )?,
        };
        validate_model_pricing_entry(&entry)?;
        if let Some(existing) = self
            .models
            .iter_mut()
            .find(|item| normalize_model_pricing_entry_id(&item.model_id) == entry.model_id)
        {
            *existing = entry.clone();
        } else {
            self.models.push(entry.clone());
        }
        Ok(entry)
    }

    pub fn delete(&mut self, model_id: &str) -> bool {
        let normalized = normalize_model_pricing_entry_id(model_id);
        let before = self.models.len();
        self.models
            .retain(|item| normalize_model_pricing_entry_id(&item.model_id) != normalized);
        self.models.len() != before
    }

    pub fn pricing_for_model(&self, model_id: &str) -> Option<ModelPricing> {
        self.find_entry(model_id)
            .and_then(ModelPricingEntry::to_pricing)
    }

    fn find_entry(&self, model_id: &str) -> Option<&ModelPricingEntry> {
        let candidates = model_pricing_candidates(model_id);
        if candidates.is_empty() {
            return None;
        }

        for candidate in &candidates {
            if let Some(entry) = self
                .models
                .iter()
                .find(|entry| normalize_model_pricing_entry_id(&entry.model_id) == *candidate)
            {
                return Some(entry);
            }
        }

        for candidate in &candidates {
            if !should_try_pricing_prefix_match(candidate) {
                continue;
            }
            if let Some(entry) = self
                .models
                .iter()
                .filter(|entry| {
                    normalize_model_pricing_entry_id(&entry.model_id)
                        .strip_prefix(candidate)
                        .is_some_and(|rest| rest.starts_with('-'))
                })
                .min_by_key(|entry| entry.model_id.len())
            {
                return Some(entry);
            }
        }

        None
    }

    fn ensure_seeded(&mut self) {
        let existing = self
            .models
            .iter()
            .map(|entry| normalize_model_pricing_entry_id(&entry.model_id))
            .collect::<BTreeSet<_>>();
        for entry in default_model_pricing_entries() {
            if !existing.contains(&entry.model_id) {
                self.models.push(entry);
            }
        }
    }
}

impl ModelPricingEntry {
    fn to_pricing(&self) -> Option<ModelPricing> {
        Some(ModelPricing {
            input_usd_per_million: parse_price(&self.input_cost_per_million),
            output_usd_per_million: parse_price(&self.output_cost_per_million),
            cache_read_usd_per_million: parse_price(&self.cache_read_cost_per_million),
            cache_creation_usd_per_million: parse_price(&self.cache_creation_cost_per_million),
            cost_multiplier: None,
        })
    }
}

pub fn model_pricing_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(MODEL_PRICING_FILE_NAME)
}

pub fn pricing_scope_matches<'a>(
    fields: impl IntoIterator<Item = Option<&'a str>>,
    target_model_id: &str,
) -> bool {
    let target_candidates = model_pricing_candidates(target_model_id);
    if target_candidates.is_empty() {
        return false;
    }
    fields.into_iter().flatten().any(|field| {
        model_pricing_candidates(field).iter().any(|candidate| {
            target_candidates.iter().any(|target| {
                target == candidate
                    || (should_try_pricing_prefix_match(candidate)
                        && target
                            .strip_prefix(candidate.as_str())
                            .is_some_and(|rest| rest.starts_with('-')))
                    || (should_try_pricing_prefix_match(target)
                        && candidate
                            .strip_prefix(target.as_str())
                            .is_some_and(|rest| rest.starts_with('-')))
            })
        })
    })
}

fn parse_pricing(value: &Value) -> Option<ModelPricing> {
    let input = first_f64(
        value,
        &[
            "inputUsdPerMillion",
            "input_usd_per_million",
            "input",
            "prompt",
        ],
    );
    let output = first_f64(
        value,
        &[
            "outputUsdPerMillion",
            "output_usd_per_million",
            "output",
            "completion",
        ],
    );
    let cache_read = first_f64(
        value,
        &[
            "cacheReadUsdPerMillion",
            "cache_read_usd_per_million",
            "cacheRead",
            "cachedInput",
        ],
    );
    let cache_creation = first_f64(
        value,
        &[
            "cacheCreationUsdPerMillion",
            "cache_creation_usd_per_million",
            "cacheCreation",
            "cacheWrite",
        ],
    );
    let multiplier = first_f64(value, &["costMultiplier", "cost_multiplier", "multiplier"]);

    if input.is_none()
        && output.is_none()
        && cache_read.is_none()
        && cache_creation.is_none()
        && multiplier.is_none()
    {
        return None;
    }

    Some(ModelPricing {
        input_usd_per_million: input,
        output_usd_per_million: output,
        cache_read_usd_per_million: cache_read,
        cache_creation_usd_per_million: cache_creation,
        cost_multiplier: multiplier,
    })
}

fn first_f64(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_f64))
}

fn provider_cost_multiplier(provider: &Provider) -> Option<f64> {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.cost_multiplier.as_deref())
        .and_then(parse_positive_f64)
        .or_else(|| {
            provider
                .settings_config
                .get("costMultiplier")
                .and_then(value_as_positive_f64)
        })
        .or_else(|| {
            provider
                .settings_config
                .get("cost_multiplier")
                .and_then(value_as_positive_f64)
        })
}

fn with_provider_multiplier(
    mut pricing: ModelPricing,
    provider_multiplier: Option<f64>,
) -> ModelPricing {
    if pricing.cost_multiplier.is_none() {
        pricing.cost_multiplier = provider_multiplier;
    }
    pricing
}

fn value_as_positive_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(parse_positive_f64))
        .filter(|value| *value >= 0.0)
}

fn parse_positive_f64(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| *value >= 0.0)
}

fn default_pricing_for_model(model: &str) -> Option<ModelPricing> {
    let model = normalize_model_pricing_entry_id(model);
    match model.as_str() {
        // Anthropic list price; the temporary introductory promo is intentionally
        // excluded so stored cost accounting remains stable.
        "claude-sonnet-5" => Some(ModelPricing {
            input_usd_per_million: Some(3.0),
            output_usd_per_million: Some(15.0),
            cache_read_usd_per_million: Some(0.30),
            cache_creation_usd_per_million: Some(3.75),
            cost_multiplier: None,
        }),
        _ => None,
    }
}

fn validate_model_pricing_entry(entry: &ModelPricingEntry) -> Result<(), String> {
    if entry.model_id.trim().is_empty() {
        return Err("model ID is required".to_string());
    }
    if entry.display_name.trim().is_empty() {
        return Err("display name is required".to_string());
    }
    for (label, value) in [
        ("input cost", &entry.input_cost_per_million),
        ("output cost", &entry.output_cost_per_million),
        ("cache read cost", &entry.cache_read_cost_per_million),
        (
            "cache creation cost",
            &entry.cache_creation_cost_per_million,
        ),
    ] {
        let parsed = value
            .parse::<f64>()
            .map_err(|error| format!("{label} is invalid: {error}"))?;
        if parsed < 0.0 {
            return Err(format!("{label} must be non-negative"));
        }
    }
    Ok(())
}

fn normalize_price_string(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("price is required".to_string());
    }
    let parsed = trimmed
        .parse::<f64>()
        .map_err(|error| format!("price is invalid: {error}"))?;
    if parsed < 0.0 {
        return Err("price must be non-negative".to_string());
    }
    Ok(trimmed.to_string())
}

fn parse_price(value: &str) -> Option<f64> {
    value.trim().parse::<f64>().ok()
}

fn normalize_model_pricing_entry_id(model: &str) -> String {
    clean_model_id_for_pricing(model)
}

fn model_pricing_candidates(model_id: &str) -> Vec<String> {
    let cleaned = clean_model_id_for_pricing(model_id);
    if is_placeholder_pricing_model(&cleaned) {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    let mut queue = vec![cleaned];
    while let Some(candidate) = queue.pop() {
        if candidate.is_empty() || candidates.iter().any(|existing| existing == &candidate) {
            continue;
        }
        candidates.push(candidate.clone());

        if let Some(stripped) = strip_known_model_namespace(&candidate) {
            queue.push(stripped);
        }
        if let Some(stripped) = strip_bedrock_model_version_suffix(&candidate) {
            queue.push(stripped);
        }
        if let Some(stripped) = strip_model_date_suffix(&candidate) {
            queue.push(stripped);
        }
        if let Some(stripped) = strip_reasoning_effort_suffix(&candidate) {
            queue.push(stripped);
        }
        if candidate.starts_with("claude-") && candidate.contains('.') {
            queue.push(candidate.replace('.', "-"));
        }
    }
    candidates
}

fn clean_model_id_for_pricing(model_id: &str) -> String {
    let trimmed = model_id.trim();
    trimmed
        .rsplit_once('/')
        .map_or(trimmed, |(_, right)| right)
        .split(':')
        .next()
        .unwrap_or(trimmed)
        .trim()
        .replace(['@', '.'], "-")
        .to_ascii_lowercase()
}

fn is_placeholder_pricing_model(model_id: &str) -> bool {
    let normalized = model_id.trim().to_ascii_lowercase();
    normalized.is_empty() || matches!(normalized.as_str(), "unknown" | "null" | "none")
}

fn strip_known_model_namespace(model_id: &str) -> Option<String> {
    if let Some(pos) = model_id.rfind("claude-") {
        if pos > 0 {
            return Some(model_id[pos..].to_string());
        }
    }

    for marker in [
        "openai-",
        "anthropic-",
        "google-",
        "moonshot-",
        "moonshotai-",
        "bedrock-",
        "global-",
    ] {
        if let Some(stripped) = model_id.strip_prefix(marker) {
            return Some(stripped.to_string());
        }
    }
    None
}

fn strip_bedrock_model_version_suffix(model_id: &str) -> Option<String> {
    let (prefix, suffix) = model_id.rsplit_once("-v")?;
    suffix
        .chars()
        .all(|ch| ch.is_ascii_digit())
        .then(|| prefix.to_string())
}

fn strip_model_date_suffix(model_id: &str) -> Option<String> {
    let (prefix, suffix) = model_id.rsplit_once('-')?;
    let compact_date = suffix.len() == 8 && suffix.chars().all(|ch| ch.is_ascii_digit());
    let year = suffix.len() == 4
        && suffix.chars().all(|ch| ch.is_ascii_digit())
        && suffix.starts_with("20");
    (compact_date || year).then(|| prefix.to_string())
}

fn strip_reasoning_effort_suffix(model_id: &str) -> Option<String> {
    for suffix in ["-minimal", "-low", "-medium", "-high"] {
        if let Some(stripped) = model_id.strip_suffix(suffix) {
            return Some(stripped.to_string());
        }
    }
    None
}

fn should_try_pricing_prefix_match(model_id: &str) -> bool {
    model_id.starts_with("claude-")
        || model_id.starts_with("gpt-")
        || model_id.starts_with("gemini-")
        || model_id.starts_with("kimi-")
        || model_id.starts_with("deepseek-")
        || model_id.starts_with("glm-")
}

fn default_model_pricing_entries() -> Vec<ModelPricingEntry> {
    [
        (
            "claude-sonnet-5",
            "Claude Sonnet 5",
            "3",
            "15",
            "0.30",
            "3.75",
        ),
        (
            "claude-opus-4-8",
            "Claude Opus 4.8",
            "5",
            "25",
            "0.50",
            "6.25",
        ),
        (
            "claude-sonnet-4-6",
            "Claude Sonnet 4.6",
            "3",
            "15",
            "0.30",
            "3.75",
        ),
        (
            "claude-haiku-4-5",
            "Claude Haiku 4.5",
            "0.80",
            "4",
            "0.08",
            "1",
        ),
        ("gpt-5-5", "GPT-5.5", "2", "10", "0.20", "2.50"),
        (
            "gpt-5-5-codex-low",
            "GPT-5.5 Codex Low",
            "2",
            "10",
            "0.20",
            "2.50",
        ),
        (
            "gpt-5-5-codex-medium",
            "GPT-5.5 Codex Medium",
            "2",
            "10",
            "0.20",
            "2.50",
        ),
        (
            "gpt-5-5-codex-high",
            "GPT-5.5 Codex High",
            "2",
            "10",
            "0.20",
            "2.50",
        ),
        ("gemini-3-pro", "Gemini 3 Pro", "1.25", "10", "0.31", "1.25"),
        (
            "gemini-3-flash",
            "Gemini 3 Flash",
            "0.30",
            "2.50",
            "0.075",
            "0.30",
        ),
        ("kimi-k2", "Kimi K2", "0.60", "2.50", "0", "0"),
        ("glm-5-2", "GLM 5.2", "0.50", "2", "0", "0"),
        ("deepseek-v4-pro", "DeepSeek V4 Pro", "0.50", "2", "0", "0"),
    ]
    .into_iter()
    .map(
        |(
            model_id,
            display_name,
            input_cost_per_million,
            output_cost_per_million,
            cache_read_cost_per_million,
            cache_creation_cost_per_million,
        )| ModelPricingEntry {
            model_id: model_id.to_string(),
            display_name: display_name.to_string(),
            input_cost_per_million: input_cost_per_million.to_string(),
            output_cost_per_million: output_cost_per_million.to_string(),
            cache_read_cost_per_million: cache_read_cost_per_million.to_string(),
            cache_creation_cost_per_million: cache_creation_cost_per_million.to_string(),
        },
    )
    .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::domain::providers::model::Provider;

    use super::*;

    #[test]
    fn calculates_provider_config_pricing() {
        let provider = Provider {
            id: "p1".to_string(),
            name: "priced".to_string(),
            settings_config: json!({
                "pricing": {
                    "models": {
                        "glm-5.2": {
                            "inputUsdPerMillion": 1.0,
                            "outputUsdPerMillion": 2.0,
                            "cacheReadUsdPerMillion": 0.25,
                            "costMultiplier": 2.0
                        }
                    }
                }
            }),
            category: None,
            meta: None,
            extra: Default::default(),
        };
        let pricing = pricing_for_model(&provider, Some("glm-5.2")).unwrap();
        let cost = calculate_cost(
            TokenUsage {
                raw_input_tokens: Some(1_000_000),
                billed_input_tokens: None,
                input_tokens: Some(1_000_000),
                output_tokens: Some(500_000),
                cache_read_tokens: Some(1_000_000),
                cache_creation_tokens: None,
                total_tokens: Some(2_500_000),
            },
            pricing,
        );

        assert_eq!(cost.input_cost_usd, Some(2.0));
        assert_eq!(cost.output_cost_usd, Some(2.0));
        assert_eq!(cost.cache_read_cost_usd, Some(0.5));
        assert_eq!(cost.total_cost_usd, Some(4.5));
    }

    #[test]
    fn uses_billed_input_tokens_for_cost_when_present() {
        let pricing = ModelPricing {
            input_usd_per_million: Some(1.0),
            output_usd_per_million: None,
            cache_read_usd_per_million: Some(0.1),
            cache_creation_usd_per_million: None,
            cost_multiplier: None,
        };
        let cost = calculate_cost(
            TokenUsage {
                raw_input_tokens: Some(100),
                billed_input_tokens: Some(40),
                input_tokens: Some(100),
                output_tokens: None,
                cache_read_tokens: Some(60),
                cache_creation_tokens: None,
                total_tokens: Some(100),
            },
            pricing,
        );

        assert_eq!(cost.input_cost_usd, Some(0.00004));
        assert_eq!(cost.cache_read_cost_usd, Some(0.000006));
    }

    #[test]
    fn falls_back_to_claude_sonnet_5_default_pricing() {
        let provider = Provider {
            id: "p1".to_string(),
            name: "default-priced".to_string(),
            settings_config: json!({}),
            category: None,
            meta: None,
            extra: Default::default(),
        };

        let pricing = pricing_for_model(&provider, Some("anthropic/claude-sonnet-5")).unwrap();

        assert_eq!(pricing.input_usd_per_million, Some(3.0));
        assert_eq!(pricing.output_usd_per_million, Some(15.0));
        assert_eq!(pricing.cache_read_usd_per_million, Some(0.30));
        assert_eq!(pricing.cache_creation_usd_per_million, Some(3.75));
    }

    #[test]
    fn global_store_matches_normalized_model_ids() {
        let store = ModelPricingStore::default();
        let mut store = {
            let mut seeded = store;
            seeded.ensure_seeded();
            seeded
        };
        store
            .upsert(
                "openai/gpt-5.5@high".to_string(),
                UpdateModelPricingInput {
                    model_id: None,
                    display_name: "GPT custom".to_string(),
                    input_cost_per_million: "9".to_string(),
                    output_cost_per_million: "18".to_string(),
                    cache_read_cost_per_million: "1".to_string(),
                    cache_creation_cost_per_million: "2".to_string(),
                },
            )
            .unwrap();

        let pricing = store.pricing_for_model("OpenAI/GPT-5.5@HIGH").unwrap();

        assert_eq!(pricing.input_usd_per_million, Some(9.0));
        assert_eq!(pricing.output_usd_per_million, Some(18.0));
    }

    #[test]
    fn provider_pricing_overrides_global_store() {
        let provider = Provider {
            id: "p1".to_string(),
            name: "provider".to_string(),
            settings_config: json!({
                "pricing": {
                    "models": {
                        "gpt-5-5": {
                            "inputUsdPerMillion": 1.0,
                            "outputUsdPerMillion": 2.0
                        }
                    }
                }
            }),
            category: None,
            meta: None,
            extra: Default::default(),
        };
        let store = ModelPricingStore {
            models: vec![ModelPricingEntry {
                model_id: "gpt-5-5".to_string(),
                display_name: "global".to_string(),
                input_cost_per_million: "9".to_string(),
                output_cost_per_million: "18".to_string(),
                cache_read_cost_per_million: "0".to_string(),
                cache_creation_cost_per_million: "0".to_string(),
            }],
        };

        let pricing =
            pricing_for_model_with_store(&provider, Some(&store), Some("gpt-5-5")).unwrap();

        assert_eq!(pricing.input_usd_per_million, Some(1.0));
        assert_eq!(pricing.output_usd_per_million, Some(2.0));
    }

    #[test]
    fn pricing_scope_matches_normalized_log_fields() {
        assert!(pricing_scope_matches(
            [Some("OpenAI/GPT-5.5@HIGH"), None],
            "gpt-5-5-high"
        ));
        assert!(pricing_scope_matches(
            [Some("anthropic/claude-sonnet-5-20260601")],
            "claude-sonnet-5"
        ));
        assert!(!pricing_scope_matches(
            [Some("gemini-3-flash")],
            "claude-sonnet-5"
        ));
    }
}
