//! Request-local names for completed tool history. These aliases never grant
//! declaration, selector, or response-restoration authority.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const TOOL_SEARCH_PROXY_NAME: &str = "tool_search";
pub(super) const APPLY_PATCH_PROXY_NAME: &str = "cc_switch_apply_patch";
pub(super) const LOCAL_SHELL_PROXY_NAME: &str = "cc_switch_local_shell";
pub(super) const CHAT_TOOL_NAME_MAX_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ResponsesToolKind {
    Function,
    Namespace,
    Custom,
    ToolSearch,
    ApplyPatch,
    LocalShell,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ResponsesToolIdentity {
    pub(super) kind: ResponsesToolKind,
    pub(super) namespace: Option<String>,
    pub(super) name: String,
}

impl ResponsesToolIdentity {
    fn retained_bytes(&self) -> usize {
        self.name
            .capacity()
            .saturating_add(self.namespace.as_ref().map_or(0, |value| value.capacity()))
    }

    fn base_name(&self) -> String {
        match self.kind {
            ResponsesToolKind::ToolSearch => TOOL_SEARCH_PROXY_NAME.to_string(),
            ResponsesToolKind::ApplyPatch => APPLY_PATCH_PROXY_NAME.to_string(),
            ResponsesToolKind::LocalShell => LOCAL_SHELL_PROXY_NAME.to_string(),
            _ => self.namespace.as_ref().map_or_else(
                || self.name.clone(),
                |namespace| format!("{namespace}__{}", self.name),
            ),
        }
    }

    fn hashed_name(&self, base: &str, attempt: u64) -> String {
        let mut digest = Sha256::new();
        let kind = match self.kind {
            ResponsesToolKind::Function => "function",
            ResponsesToolKind::Namespace => "namespace_function",
            ResponsesToolKind::Custom => "custom",
            ResponsesToolKind::ToolSearch => "tool_search",
            ResponsesToolKind::ApplyPatch => "apply_patch",
            ResponsesToolKind::LocalShell => "local_shell",
        };
        // Length-prefix the original components; concatenated namespace/name
        // pairs can be identical while naming different tools.
        for component in [
            kind,
            self.namespace.as_deref().unwrap_or_default(),
            &self.name,
        ] {
            digest.update((component.len() as u64).to_be_bytes());
            digest.update(component.as_bytes());
        }
        digest.update(attempt.to_be_bytes());
        let suffix = format!("__{}", hex::encode(&digest.finalize()[..12]));
        let prefix = base
            .chars()
            .map(|character| {
                if allowed_name_character(character) {
                    character
                } else {
                    '_'
                }
            })
            .take(CHAT_TOOL_NAME_MAX_LEN - suffix.len())
            .collect::<String>();
        format!("{prefix}{suffix}")
    }
}

fn allowed_name_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
}

#[derive(Debug, Clone, Default)]
pub(super) struct HistoricalToolNames(BTreeMap<ResponsesToolIdentity, String>);

impl HistoricalToolNames {
    pub(super) fn build(
        identities: BTreeSet<ResponsesToolIdentity>,
        active: impl IntoIterator<Item = (String, ResponsesToolIdentity)>,
    ) -> Result<Self, String> {
        let mut owners = BTreeMap::new();
        for (kind, name, alias) in [
            (
                ResponsesToolKind::ToolSearch,
                "tool_search",
                TOOL_SEARCH_PROXY_NAME,
            ),
            (
                ResponsesToolKind::ApplyPatch,
                "apply_patch",
                APPLY_PATCH_PROXY_NAME,
            ),
            (
                ResponsesToolKind::LocalShell,
                "local_shell",
                LOCAL_SHELL_PROXY_NAME,
            ),
        ] {
            owners.insert(
                alias.to_string(),
                ResponsesToolIdentity {
                    kind,
                    name: name.to_string(),
                    namespace: None,
                },
            );
        }
        let mut active_names = BTreeMap::new();
        for (alias, identity) in active {
            owners.insert(alias.clone(), identity.clone());
            active_names.insert(identity, alias);
        }
        // Reserve bare historical spellings before allocating qualified aliases.
        // Sorted identities make allocation independent of input item order.
        for identity in &identities {
            if identity.namespace.is_none() {
                owners
                    .entry(identity.base_name())
                    .or_insert_with(|| identity.clone());
            }
        }
        let mut names = BTreeMap::new();
        for identity in identities {
            if identity.name.trim().is_empty() {
                return Err("Responses historical tool name must be nonempty".to_string());
            }
            if let Some(alias) = active_names.get(&identity) {
                names.insert(identity, alias.clone());
                continue;
            }
            let base = identity.base_name();
            let usable = |candidate: &str, owners: &BTreeMap<String, ResponsesToolIdentity>| {
                !candidate.is_empty()
                    && candidate.len() <= CHAT_TOOL_NAME_MAX_LEN
                    && candidate.chars().all(allowed_name_character)
                    && owners.get(candidate).is_none_or(|owner| owner == &identity)
            };
            let alias = if usable(&base, &owners) {
                base
            } else {
                (0..64)
                    .map(|attempt| identity.hashed_name(&base, attempt))
                    .find(|candidate| usable(candidate, &owners))
                    .ok_or_else(|| {
                        "Responses history tool alias allocation exhausted".to_string()
                    })?
            };
            owners.insert(alias.clone(), identity.clone());
            names.insert(identity, alias);
        }
        Ok(Self(names))
    }

    pub(super) fn get(&self, identity: &ResponsesToolIdentity) -> Option<&str> {
        self.0.get(identity).map(String::as_str)
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.0.iter().fold(0usize, |bytes, (identity, alias)| {
            bytes
                .saturating_add(std::mem::size_of::<ResponsesToolIdentity>())
                .saturating_add(std::mem::size_of::<String>())
                .saturating_add(std::mem::size_of::<usize>() * 3)
                .saturating_add(identity.retained_bytes())
                .saturating_add(alias.capacity())
        })
    }
}
