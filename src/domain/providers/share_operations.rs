//! Executable Share operations derive from the compiled Registry runtime contract.
use super::{registry::UpstreamProtocol, runtime::ProviderRuntimePlan};
use crate::domain::sharing::capabilities::ShareOperation;
use serde_json::Value;
/// Driver/transport implementation capabilities, not a Provider-name whitelist.
/// This describes controllable request intent, not successful upstream live verification.
pub fn driver_implements(plan: &ProviderRuntimePlan, op: ShareOperation) -> bool {
    use ShareOperation::*;
    let driver = plan.driver_id.as_str();
    if crate::domain::usage::value_pricing::value_metering_strategy(driver).is_none() {
        return false;
    }
    // These transports have a narrower contract than their wire protocol.
    // In particular, Grok Responses does not implement OpenAI file/code tools.
    if driver == "oauth.grok_responses" {
        return matches!(op, ImageGenerate | ImageEdit | VideoGenerate | WebSearch);
    }
    // The v1internal adapter translates Google Search for both App surfaces,
    // including its model fallback. Other Gemini tools are not implied by it.
    if matches!(driver, "special.antigravity" | "special.agy") {
        return op == WebSearch;
    }
    match op {
        ImageGenerate | ImageEdit => {
            matches!(
                plan.upstream_protocol,
                UpstreamProtocol::OpenAiResponses | UpstreamProtocol::GeminiNative
            )
        }
        AudioGenerate => matches!(
            plan.upstream_protocol,
            UpstreamProtocol::OpenAiChat | UpstreamProtocol::GeminiNative
        ),
        WebSearch => {
            matches!(
                plan.upstream_protocol,
                UpstreamProtocol::OpenAiResponses
                    | UpstreamProtocol::AnthropicMessages
                    | UpstreamProtocol::GeminiNative
            ) || matches!(
                driver,
                "special.deepseek_account" | "special.perplexity_web_session"
            )
        }
        WebFetch => matches!(
            plan.upstream_protocol,
            UpstreamProtocol::AnthropicMessages | UpstreamProtocol::GeminiNative
        ),
        FileSearch | CodeExecution => matches!(
            plan.upstream_protocol,
            UpstreamProtocol::OpenAiResponses | UpstreamProtocol::GeminiNative
        ),
        // No reliable hosted computer/remote MCP collector and control contract yet.
        Computer | RemoteMcp | VideoGenerate => false,
    }
}

/// Provider configuration is separate from implementation and account evidence.
pub fn required_provider_option(
    plan: &ProviderRuntimePlan,
    op: ShareOperation,
) -> Option<&'static str> {
    use ShareOperation::*;
    match (plan.driver_id.as_str(), op) {
        ("oauth.grok_responses", ImageGenerate) => Some("grokImageGenerationEnabled"),
        ("oauth.grok_responses", ImageEdit) => Some("grokImageEditEnabled"),
        ("oauth.grok_responses", VideoGenerate) => Some("grokVideoGenerationEnabled"),
        ("oauth.openai_codex", ImageGenerate | ImageEdit) => Some("codexImageGenerationEnabled"),
        _ => None,
    }
}

pub fn provider_enabled(plan: &ProviderRuntimePlan, op: ShareOperation) -> bool {
    required_provider_option(plan, op)
        .is_none_or(|key| plan.driver_options.get(key).and_then(Value::as_bool) == Some(true))
}

/// Used by both preview and admission. A catalogue entry alone grants nothing.
pub fn implemented(plan: &ProviderRuntimePlan, op: ShareOperation) -> bool {
    driver_implements(plan, op) && provider_enabled(plan, op)
}

/// Reuse generation-scoped evidence where a Driver already publishes it.
/// Missing/stale evidence remains unknown and never becomes a static success claim.
pub fn observed_account_state(
    plan: &ProviderRuntimePlan,
    accounts: &crate::domain::accounts::store::AccountStore,
    op: ShareOperation,
    now: i64,
) -> Option<crate::domain::accounts::capability_evidence::AccountCapabilityObservationState> {
    use super::runtime::RuntimeAuthRef;
    use crate::domain::accounts::capability_evidence as evidence;
    let RuntimeAuthRef::ManagedAccount {
        account_id,
        expected_provider_type,
        auth_identity_generation,
    } = &plan.auth_ref
    else {
        return None;
    };
    let account = accounts.find_for_provider(*expected_provider_type, Some(account_id))?;
    if account.auth_identity_generation != *auth_identity_generation {
        return None;
    }
    let (capability, dimension) = match (plan.driver_id.as_str(), op) {
        ("oauth.grok_responses", ShareOperation::ImageGenerate) => (
            evidence::GROK_CODE_PLAN_CAPABILITY,
            evidence::IMAGE_GENERATION_DIMENSION,
        ),
        ("oauth.grok_responses", ShareOperation::ImageEdit) => (
            evidence::GROK_CODE_PLAN_CAPABILITY,
            evidence::IMAGE_EDIT_DIMENSION,
        ),
        ("oauth.grok_responses", ShareOperation::VideoGenerate) => (
            evidence::GROK_CODE_PLAN_CAPABILITY,
            evidence::VIDEO_GENERATION_DIMENSION,
        ),
        ("oauth.grok_responses", ShareOperation::WebSearch) => (
            evidence::GROK_CODE_PLAN_CAPABILITY,
            evidence::SEARCH_DIMENSION,
        ),
        _ => return None,
    };
    evidence::fresh_observation_state(account, capability, dimension, now)
}
