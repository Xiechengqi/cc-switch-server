//! Reserve each web scope before sending. Authentication invalidation is never
//! evicted for capacity or age; explicit credential reimport changes the scope.
use crate::domain::providers::web_session::{
    WebSessionRuntimeStore, WebSessionScope, WebSessionStateRecord,
};
use crate::infra::resident_flights::node_bytes;
use crate::infra::resident_memory::{
    process_resident_memory, ResidentMemoryBudget, ResidentMemoryReservation,
};
use std::collections::{BTreeMap, BTreeSet};

const MAX_WEB_SCOPES: usize = 1024;
const MAX_SESSION_ID_BYTES: usize = 512;

#[derive(Debug)]
pub(crate) struct WebSessionRuntime {
    store: WebSessionRuntimeStore,
    charges: BTreeMap<WebSessionScope, ResidentMemoryReservation>,
    parent: ResidentMemoryBudget,
}
impl Default for WebSessionRuntime {
    fn default() -> Self {
        Self::with_parent(process_resident_memory())
    }
}
impl WebSessionRuntime {
    pub(crate) fn with_parent(parent: ResidentMemoryBudget) -> Self {
        Self {
            store: WebSessionRuntimeStore::default(),
            charges: BTreeMap::new(),
            parent,
        }
    }
    pub(crate) fn admit(&mut self, scope: &WebSessionScope) -> bool {
        self.retain_current(scope);
        if self.charges.contains_key(scope) {
            return true;
        }
        if self.charges.len() >= MAX_WEB_SCOPES {
            crate::metrics::record_resident_retention_refused("web_session_scope");
            return false;
        }
        let dynamic = scope
            .provider_key
            .provider_id
            .capacity()
            .saturating_add(scope.runtime_fingerprint.capacity())
            .saturating_add(scope.profile_id.capacity())
            .saturating_add(scope.fixed_origin.capacity());
        // Reserve all keys, the bounded session payload and the auth marker
        // before a 401/403 can arrive, including after a parent limit decrease.
        let bytes = node_bytes::<WebSessionScope, ResidentMemoryReservation>(dynamic)
            .saturating_add(node_bytes::<WebSessionScope, WebSessionStateRecord>(
                dynamic.saturating_add(MAX_SESSION_ID_BYTES),
            ))
            .saturating_add(node_bytes::<WebSessionScope, ()>(dynamic));
        let Some(memory) = self.parent.reserve(bytes) else {
            crate::metrics::record_resident_retention_refused("web_session_scope");
            return false;
        };
        self.charges.insert(scope.clone(), memory);
        true
    }
    pub(crate) fn requires_explicit_reimport(&self, scope: &WebSessionScope) -> bool {
        self.store.requires_explicit_reimport(scope)
    }
    pub(crate) fn invalidate_authentication(&mut self, scope: &WebSessionScope) -> bool {
        if !self.admit(scope) {
            return false;
        }
        self.store.invalidate_authentication(scope);
        true
    }
    pub(crate) fn insert_session(
        &mut self,
        scope: WebSessionScope,
        record: WebSessionStateRecord,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.charges.contains_key(&scope),
            "Web Session scope was not admitted"
        );
        anyhow::ensure!(
            record.session_id.len() <= MAX_SESSION_ID_BYTES,
            "Web Session id exceeds reserved slot"
        );
        self.store.insert_session(scope, record)
    }
    pub(crate) fn retain_current(&mut self, scope: &WebSessionScope) {
        self.store.retain_current(scope);
        self.charges.retain(|candidate, _| {
            candidate.provider_key != scope.provider_key || candidate == scope
        });
    }
    pub(crate) fn retain_scopes(&mut self, scopes: &BTreeSet<WebSessionScope>) {
        self.store.retain_scopes(scopes);
        self.charges.retain(|scope, _| scopes.contains(scope));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::providers::{model::AppKind, registry::ProviderKey};
    fn scope(id: &str, generation: u64) -> WebSessionScope {
        WebSessionScope {
            provider_key: ProviderKey::new(AppKind::Claude, id).unwrap(),
            provider_revision: generation,
            runtime_fingerprint: format!("runtime-{generation}"),
            credential_generation: generation,
            profile_id: "web_session.grok_web".into(),
            fixed_origin: "https://grok.com".into(),
        }
    }
    #[test]
    fn r5_resource_web_scope_reserves_auth_marker_before_send_and_survives_lowered_limit() {
        let parent = ResidentMemoryBudget::new(None);
        let mut runtime = WebSessionRuntime::with_parent(parent.clone());
        let first = scope("first", 1);
        let second = scope("second", 1);
        assert!(runtime.admit(&first));
        assert!(runtime.admit(&second));
        let bytes = parent.snapshot().used_bytes;
        parent.configure_limit(Some(0));
        assert!(runtime.invalidate_authentication(&first));
        assert!(runtime.requires_explicit_reimport(&first));
        assert!(!runtime.requires_explicit_reimport(&second));
        assert_eq!(parent.snapshot().used_bytes, bytes);
        assert!(!runtime.admit(&scope("overflow", 1)));
        assert!(runtime.requires_explicit_reimport(&first));
        drop(runtime);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }
    #[test]
    fn r5_resource_web_rebind_and_delete_release_only_the_matching_scope() {
        let parent = ResidentMemoryBudget::new(None);
        let mut runtime = WebSessionRuntime::with_parent(parent.clone());
        let first = scope("first", 1);
        let second = scope("second", 1);
        assert!(runtime.admit(&first));
        assert!(runtime.admit(&second));
        assert!(runtime.invalidate_authentication(&first));
        let next = scope("first", 2);
        assert!(runtime.admit(&next));
        assert!(!runtime.requires_explicit_reimport(&next));
        assert_eq!(runtime.charges.len(), 2);
        runtime
            .insert_session(
                next.clone(),
                WebSessionStateRecord {
                    session_id: "x".repeat(MAX_SESSION_ID_BYTES),
                    observed_at_ms: 1000,
                },
            )
            .unwrap();
        let bytes = parent.snapshot().used_bytes;
        runtime.retain_scopes(&BTreeSet::from([next]));
        assert_eq!(runtime.charges.len(), 1);
        assert!(parent.snapshot().used_bytes < bytes);
        runtime.retain_scopes(&BTreeSet::new());
        assert_eq!(parent.snapshot().used_bytes, 0);
    }
    #[test]
    fn r5_resource_web_scope_capacity_never_evicts_an_authentication_invalidation() {
        let parent = ResidentMemoryBudget::new(None);
        let mut runtime = WebSessionRuntime::with_parent(parent.clone());
        let first = scope("first", 1);
        assert!(runtime.invalidate_authentication(&first));
        for index in 1..MAX_WEB_SCOPES {
            assert!(runtime.admit(&scope(&format!("provider-{index}"), 1)));
        }
        assert!(!runtime.admit(&scope("overflow", 1)));
        assert!(runtime.requires_explicit_reimport(&first));
        assert_eq!(runtime.charges.len(), MAX_WEB_SCOPES);
        drop(runtime);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }
}
