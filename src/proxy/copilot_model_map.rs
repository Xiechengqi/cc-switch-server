use serde_json::Value;

pub(super) fn normalize_to_copilot_id(client_id: &str) -> Option<String> {
    let trimmed = client_id.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() < 8 || !bytes[..7].eq_ignore_ascii_case(b"claude-") {
        return None;
    }

    let has_one_m_bracket = ends_with_ascii_ci(bytes, b"[1m]");
    if trimmed.contains('.') && !has_one_m_bracket {
        return None;
    }

    let (base, has_1m_suffix) = split_one_m_suffix(trimmed);
    let stripped = strip_trailing_date(base);
    let dotted = dashes_to_dot_in_last_version(stripped);
    if dotted.is_none() && !has_1m_suffix {
        return None;
    }

    let mut candidate = dotted.unwrap_or_else(|| stripped.to_string());
    if has_1m_suffix {
        candidate.push_str("-1m");
    }
    (candidate != trimmed).then_some(candidate)
}

pub(super) fn normalize_or_resolve_model(client_id: &str, settings: &Value) -> Option<String> {
    let normalized = normalize_to_copilot_id(client_id);
    let target = normalized.as_deref().unwrap_or(client_id);
    let models = configured_models(settings);

    if models.is_empty() {
        return normalized;
    }
    if models
        .iter()
        .any(|model| model.eq_ignore_ascii_case(target))
    {
        return normalized.filter(|model| model != client_id);
    }
    let fallback = family_fallback(target, &models)?;
    if fallback.eq_ignore_ascii_case(client_id) {
        None
    } else {
        Some(fallback)
    }
}

fn configured_models(settings: &Value) -> Vec<String> {
    let mut models = Vec::new();
    for pointer in [
        "/copilotModels",
        "/copilot_models",
        "/modelCatalog/models",
        "/model_catalog/models",
    ] {
        let Some(items) = settings.pointer(pointer).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            if let Some(model) = item.as_str().map(str::trim).filter(|item| !item.is_empty()) {
                models.push(model.to_string());
                continue;
            }
            for key in ["upstreamModel", "upstream_model", "id", "model", "name"] {
                if let Some(model) = item
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                {
                    models.push(model.to_string());
                    break;
                }
            }
        }
    }
    models
}

fn ends_with_ascii_ci(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len()
        && haystack[haystack.len() - needle.len()..].eq_ignore_ascii_case(needle)
}

fn split_one_m_suffix(id: &str) -> (&str, bool) {
    let bytes = id.as_bytes();
    if ends_with_ascii_ci(bytes, b"[1m]") {
        return (&id[..bytes.len() - 4], true);
    }
    if ends_with_ascii_ci(bytes, b"-1m") {
        return (&id[..bytes.len() - 3], true);
    }
    (id, false)
}

fn strip_trailing_date(id: &str) -> &str {
    let Some(last_dash) = id.rfind('-') else {
        return id;
    };
    let suffix = &id[last_dash + 1..];
    if suffix.len() == 8 && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        &id[..last_dash]
    } else {
        id
    }
}

fn dashes_to_dot_in_last_version(id: &str) -> Option<String> {
    let last_dash = id.rfind('-')?;
    let last_segment = &id[last_dash + 1..];
    if last_segment.is_empty() || !last_segment.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let head = &id[..last_dash];
    let prev_dash = head.rfind('-')?;
    let prev_segment = &head[prev_dash + 1..];
    if prev_segment.is_empty() || !prev_segment.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(format!("{head}.{last_segment}"))
}

fn detect_family(id: &str) -> Option<&'static str> {
    let lower = id.to_ascii_lowercase();
    if lower.contains("haiku") {
        Some("haiku")
    } else if lower.contains("sonnet") {
        Some("sonnet")
    } else if lower.contains("opus") {
        Some("opus")
    } else {
        None
    }
}

fn extract_major_minor(id: &str) -> Option<(u32, u32)> {
    let lower = id.to_ascii_lowercase();
    let family = detect_family(&lower)?;
    let after = &lower[lower.find(family)? + family.len()..];
    let after = after.strip_prefix('-')?;
    let segment = after.split(['-', '[', ' ']).next()?;
    let mut parts = segment.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor))
}

fn family_fallback(target: &str, models: &[String]) -> Option<String> {
    let family = detect_family(target)?;
    let want_1m = target.ends_with("-1m");
    let pick_best = |require_1m: bool| -> Option<String> {
        models
            .iter()
            .filter(|model| {
                let lower = model.to_ascii_lowercase();
                lower.contains(family) && lower.ends_with("-1m") == require_1m
            })
            .filter_map(|model| extract_major_minor(model).map(|version| (model, version)))
            .max_by_key(|(_, version)| *version)
            .map(|(model, _)| model.clone())
    };

    if want_1m {
        pick_best(true).or_else(|| pick_best(false))
    } else {
        pick_best(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_claude_four_dash_versions() {
        assert_eq!(
            normalize_to_copilot_id("claude-sonnet-4-6"),
            Some("claude-sonnet-4.6".to_string())
        );
        assert_eq!(
            normalize_to_copilot_id("claude-sonnet-4-6[1m]"),
            Some("claude-sonnet-4.6-1m".to_string())
        );
        assert_eq!(
            normalize_to_copilot_id("claude-haiku-4-5-20251001"),
            Some("claude-haiku-4.5".to_string())
        );
    }

    #[test]
    fn keeps_non_copilot_or_already_normalized_models() {
        assert_eq!(normalize_to_copilot_id("gpt-5"), None);
        assert_eq!(normalize_to_copilot_id("claude-sonnet-4.6"), None);
        assert_eq!(normalize_to_copilot_id("claude-3-5-sonnet"), None);
    }

    #[test]
    fn resolves_against_configured_catalog() {
        let settings = json!({
            "modelCatalog": {
                "models": [
                    {"id": "claude-sonnet-4.5"},
                    {"id": "claude-sonnet-4.6"},
                    {"id": "claude-sonnet-4.6-1m"}
                ]
            }
        });
        assert_eq!(
            normalize_or_resolve_model("claude-sonnet-4-6[1m]", &settings),
            Some("claude-sonnet-4.6-1m".to_string())
        );
        assert_eq!(
            normalize_or_resolve_model("claude-sonnet-4.8", &settings),
            Some("claude-sonnet-4.6".to_string())
        );
    }
}
