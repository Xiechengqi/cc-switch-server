use super::context::BindingSnapshot;
use super::terminal::CommitGuard;

/// A bounded, provider-neutral recovery class. Provider-specific classifiers
/// stay with their drivers and map into one of these classes only after they
/// have proved a replay is safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::proxy) enum RecoveryStage {
    Auth,
    Capacity,
    BodyCompatibility,
    Reasoning,
    SessionRollover,
    Transport,
    WebsocketToHttp,
}

impl RecoveryStage {
    pub(in crate::proxy) const fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::Capacity => "capacity",
            Self::BodyCompatibility => "body_compatibility",
            Self::Reasoning => "reasoning",
            Self::SessionRollover => "session_rollover",
            Self::Transport => "transport",
            Self::WebsocketToHttp => "websocket_to_http",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Auth => 0,
            Self::Capacity => 1,
            Self::BodyCompatibility => 2,
            Self::Reasoning => 3,
            Self::SessionRollover => 4,
            Self::Transport => 5,
            Self::WebsocketToHttp => 6,
        }
    }
}

const RECOVERY_STAGE_COUNT: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::proxy) enum DelaySource {
    None,
    Immediate,
    RetryAfter,
    ProviderHint,
    BoundedBackoff,
}

impl DelaySource {
    pub(in crate::proxy) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Immediate => "immediate",
            Self::RetryAfter => "retry_after",
            Self::ProviderHint => "provider_hint",
            Self::BoundedBackoff => "bounded_backoff",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::proxy) enum RetryDecision {
    Reserved,
    DeniedTotalLimit,
    DeniedStageLimit,
    DeniedElapsedLimit,
    DeniedCommitted,
    DeniedBindingDrift,
}

impl RetryDecision {
    pub(in crate::proxy) const fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::DeniedTotalLimit => "denied_total_limit",
            Self::DeniedStageLimit => "denied_stage_limit",
            Self::DeniedElapsedLimit => "denied_elapsed_limit",
            Self::DeniedCommitted => "denied_committed",
            Self::DeniedBindingDrift => "denied_binding_drift",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::proxy) struct AttemptLimits {
    pub(in crate::proxy) total_retries: u32,
    pub(in crate::proxy) elapsed_ms: u128,
    per_stage: [u32; RECOVERY_STAGE_COUNT],
}

impl AttemptLimits {
    pub(in crate::proxy) const fn forward_default(total_retries: u32, elapsed_ms: u128) -> Self {
        Self {
            total_retries,
            elapsed_ms,
            per_stage: [1, 2, 3, 1, 1, 3, 1],
        }
    }

    #[cfg(test)]
    pub(super) const fn for_test(total_retries: u32, elapsed_ms: u128, per_stage: u32) -> Self {
        Self {
            total_retries,
            elapsed_ms,
            per_stage: [per_stage; RECOVERY_STAGE_COUNT],
        }
    }

    const fn stage_limit(self, stage: RecoveryStage) -> u32 {
        self.per_stage[stage.index()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::proxy) struct AttemptObservation {
    pub(in crate::proxy) stage: RecoveryStage,
    pub(in crate::proxy) reason: &'static str,
    pub(in crate::proxy) delay_source: DelaySource,
    pub(in crate::proxy) decision: RetryDecision,
}

#[derive(Debug, Clone)]
pub(in crate::proxy) struct AttemptBudget {
    limits: AttemptLimits,
    started_at_ms: u128,
    retries_used: u32,
    per_stage_used: [u32; RECOVERY_STAGE_COUNT],
    last_observation: Option<AttemptObservation>,
}

impl AttemptBudget {
    pub(in crate::proxy) fn new(limits: AttemptLimits, started_at_ms: u128) -> Self {
        Self {
            limits,
            started_at_ms,
            retries_used: 0,
            per_stage_used: [0; RECOVERY_STAGE_COUNT],
            last_observation: None,
        }
    }

    pub(in crate::proxy) fn retries_used(&self) -> u32 {
        self.retries_used
    }

    pub(in crate::proxy) fn started_at_ms(&self) -> u128 {
        self.started_at_ms
    }

    pub(in crate::proxy) fn used_for(&self, stage: RecoveryStage) -> u32 {
        self.per_stage_used[stage.index()]
    }

    pub(in crate::proxy) fn has_remaining(&self, now_ms: u128, commit: &CommitGuard) -> bool {
        !commit.is_committed()
            && self.retries_used < self.limits.total_retries
            && now_ms.saturating_sub(self.started_at_ms) < self.limits.elapsed_ms
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::proxy) fn reserve(
        &mut self,
        stage: RecoveryStage,
        reason: &'static str,
        delay_source: DelaySource,
        now_ms: u128,
        commit: &CommitGuard,
        expected_binding: Option<&BindingSnapshot>,
        current_binding: Option<&BindingSnapshot>,
    ) -> RetryDecision {
        let decision = if commit.is_committed() {
            RetryDecision::DeniedCommitted
        } else if expected_binding != current_binding {
            RetryDecision::DeniedBindingDrift
        } else if now_ms.saturating_sub(self.started_at_ms) >= self.limits.elapsed_ms {
            RetryDecision::DeniedElapsedLimit
        } else if self.retries_used >= self.limits.total_retries {
            RetryDecision::DeniedTotalLimit
        } else if self.per_stage_used[stage.index()] >= self.limits.stage_limit(stage) {
            RetryDecision::DeniedStageLimit
        } else {
            self.retries_used = self.retries_used.saturating_add(1);
            self.per_stage_used[stage.index()] =
                self.per_stage_used[stage.index()].saturating_add(1);
            RetryDecision::Reserved
        };
        self.last_observation = Some(AttemptObservation {
            stage,
            reason,
            delay_source,
            decision,
        });
        decision
    }

    pub(in crate::proxy) fn last_observation(&self) -> Option<AttemptObservation> {
        self.last_observation
    }
}
