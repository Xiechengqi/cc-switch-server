use std::collections::{BTreeSet, HashSet};

pub const CURSOR_MODEL_ALIASES: &[&str] = &[
    "cursor",
    "cursor-agent",
    "cursor-ask",
    "cursor-composer",
    "cursor-composer-fast",
    "cursor-plan",
    "composer-2.5",
    "composer-2.5-fast",
];

pub const CURSOR_MODEL_PREFIXES: &[&str] =
    &["cursor:", "cursor-agent:", "cursor-plan:", "cursor-ask:"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorAgentMode {
    Agent,
    Ask,
    Plan,
}

impl CursorAgentMode {
    pub const fn wire_value(self) -> u64 {
        match self {
            Self::Agent => 1,
            Self::Ask => 2,
            Self::Plan => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorModelResolution {
    pub model_id: String,
    pub mode: CursorAgentMode,
    pub fast: bool,
}

pub fn normalize_cursor_model_id(model: &str) -> String {
    let model = model.trim();
    match model.to_ascii_lowercase().as_str() {
        "composer-2-5" | "composer-2.5-sdk" | "composer-latest" => "composer-2.5".to_string(),
        "composer-2-5-fast" | "composer-2.5-sdk-fast" | "composer-latest-fast" => {
            "composer-2.5-fast".to_string()
        }
        _ => model.to_string(),
    }
}

pub fn is_explicit_cursor_selector(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    CURSOR_MODEL_PREFIXES
        .iter()
        .any(|prefix| model.starts_with(prefix))
        || CURSOR_MODEL_ALIASES.iter().any(|alias| model == *alias)
}

pub fn resolve_cursor_model(model: &str) -> Result<CursorModelResolution, String> {
    resolve_cursor_model_with_catalog(model, None)
}

pub fn resolve_cursor_model_with_catalog(
    model: &str,
    live_catalog_ids: Option<&HashSet<String>>,
) -> Result<CursorModelResolution, String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("Cursor model selector must not be empty".to_string());
    }

    let lower = model.to_ascii_lowercase();
    for (prefix, mode) in [
        ("cursor-agent:", CursorAgentMode::Agent),
        ("cursor-plan:", CursorAgentMode::Plan),
        ("cursor-ask:", CursorAgentMode::Ask),
        ("cursor:", CursorAgentMode::Agent),
    ] {
        if lower.starts_with(prefix) {
            let raw = model[prefix.len()..].trim();
            if raw.is_empty() {
                return Err(format!(
                    "Cursor model selector `{prefix}` requires a model id"
                ));
            }
            return resolve_wire_model(raw, mode, live_catalog_ids);
        }
    }

    let resolution = match lower.as_str() {
        "cursor" | "cursor-agent" | "auto" => CursorModelResolution {
            model_id: "default".to_string(),
            mode: CursorAgentMode::Agent,
            fast: false,
        },
        "cursor-plan" => CursorModelResolution {
            model_id: "default".to_string(),
            mode: CursorAgentMode::Plan,
            fast: false,
        },
        "cursor-ask" => CursorModelResolution {
            model_id: "default".to_string(),
            mode: CursorAgentMode::Ask,
            fast: false,
        },
        "cursor-composer" => CursorModelResolution {
            model_id: "composer-2.5".to_string(),
            mode: CursorAgentMode::Agent,
            fast: false,
        },
        "cursor-composer-fast" => CursorModelResolution {
            model_id: "composer-2.5".to_string(),
            mode: CursorAgentMode::Agent,
            fast: true,
        },
        _ => return resolve_wire_model(model, CursorAgentMode::Agent, live_catalog_ids),
    };
    Ok(resolution)
}

pub fn cursor_supported_models() -> Vec<String> {
    let mut models = CURSOR_MODEL_ALIASES
        .iter()
        .map(|model| (*model).to_string())
        .collect::<Vec<_>>();
    models.sort_unstable();
    models
}

pub fn cursor_namespaced_model_ids(model: &str) -> Vec<String> {
    let model = model.trim();
    if model.is_empty() {
        return Vec::new();
    }
    let mut ids = BTreeSet::new();
    for prefix in CURSOR_MODEL_PREFIXES {
        ids.insert(format!("{prefix}{model}"));
    }
    ids.into_iter().collect()
}

fn resolve_wire_model(
    raw: &str,
    mode: CursorAgentMode,
    live_catalog_ids: Option<&HashSet<String>>,
) -> Result<CursorModelResolution, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("Cursor wire model id must not be empty".to_string());
    }
    if live_catalog_ids.is_some_and(|catalog| catalog.contains(raw)) {
        return Ok(CursorModelResolution {
            model_id: raw.to_string(),
            mode,
            fast: false,
        });
    }
    let normalized = normalize_cursor_model_id(raw);
    let fast = normalized.to_ascii_lowercase().ends_with("-fast");
    let model_id = if fast {
        normalized[..normalized.len() - "-fast".len()]
            .trim()
            .to_string()
    } else {
        normalized
    };
    if model_id.is_empty() {
        return Err("Cursor wire model id must not be empty".to_string());
    }
    Ok(CursorModelResolution {
        model_id,
        mode,
        fast,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_resolve_modes_and_wire_models() {
        assert_eq!(
            resolve_cursor_model("cursor").unwrap(),
            CursorModelResolution {
                model_id: "default".to_string(),
                mode: CursorAgentMode::Agent,
                fast: false,
            }
        );
        assert_eq!(
            resolve_cursor_model("cursor-plan").unwrap().mode,
            CursorAgentMode::Plan
        );
        assert_eq!(
            resolve_cursor_model("cursor-ask").unwrap().mode,
            CursorAgentMode::Ask
        );
        assert_eq!(
            resolve_cursor_model("cursor-composer-fast").unwrap(),
            CursorModelResolution {
                model_id: "composer-2.5".to_string(),
                mode: CursorAgentMode::Agent,
                fast: true,
            }
        );
    }

    #[test]
    fn prefixes_support_modes_and_arbitrary_fast_variants() {
        let plan = resolve_cursor_model("cursor-plan:gpt-5.5-fast").unwrap();
        assert_eq!(plan.model_id, "gpt-5.5");
        assert_eq!(plan.mode, CursorAgentMode::Plan);
        assert!(plan.fast);

        let ask = resolve_cursor_model("cursor-ask:claude-sonnet-4-6").unwrap();
        assert_eq!(ask.model_id, "claude-sonnet-4-6");
        assert_eq!(ask.mode, CursorAgentMode::Ask);
        assert!(!ask.fast);
    }

    #[test]
    fn bare_wire_models_remain_passthrough_compatible() {
        let resolution = resolve_cursor_model("gpt-5.5-fast").unwrap();
        assert_eq!(resolution.model_id, "gpt-5.5");
        assert_eq!(resolution.mode, CursorAgentMode::Agent);
        assert!(resolution.fast);
    }

    #[test]
    fn fresh_live_catalog_preserves_an_exact_fast_model_id() {
        let live = HashSet::from(["composer-2.5-fast".to_string()]);
        assert_eq!(
            resolve_cursor_model_with_catalog("composer-2.5-fast", Some(&live)).unwrap(),
            CursorModelResolution {
                model_id: "composer-2.5-fast".to_string(),
                mode: CursorAgentMode::Agent,
                fast: false,
            }
        );
        assert_eq!(
            resolve_cursor_model_with_catalog("cursor-plan:composer-2.5-fast", Some(&live))
                .unwrap(),
            CursorModelResolution {
                model_id: "composer-2.5-fast".to_string(),
                mode: CursorAgentMode::Plan,
                fast: false,
            }
        );
        assert!(
            resolve_cursor_model_with_catalog("composer-2.5-fast", None)
                .unwrap()
                .fast
        );
    }

    #[test]
    fn empty_prefixed_selectors_fail_closed() {
        assert!(resolve_cursor_model("cursor:").is_err());
        assert!(resolve_cursor_model("cursor-plan:   ").is_err());
        assert!(resolve_cursor_model("cursor:-fast").is_err());
    }

    #[test]
    fn catalog_aliases_and_prefixes_are_collision_free() {
        let aliases = cursor_supported_models();
        assert_eq!(aliases.len(), CURSOR_MODEL_ALIASES.len());
        assert!(aliases.iter().any(|model| model == "cursor-composer"));

        let ids = cursor_namespaced_model_ids("gpt-5.5");
        assert_eq!(ids.len(), CURSOR_MODEL_PREFIXES.len());
        assert!(ids.iter().any(|model| model == "cursor:gpt-5.5"));
        assert!(ids.iter().any(|model| model == "cursor-plan:gpt-5.5"));
    }

    #[test]
    fn every_catalog_alias_is_an_explicit_selector() {
        for alias in CURSOR_MODEL_ALIASES {
            assert!(is_explicit_cursor_selector(alias), "missing alias {alias}");
        }
        assert!(!is_explicit_cursor_selector("gpt-5.5"));
    }
}
