#[derive(Debug, Clone, Copy)]
pub struct EmbeddedWebAsset {
    pub path: &'static str,
    pub content_type: &'static str,
    pub bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/embedded_web_assets.rs"));

pub fn asset_count() -> usize {
    EMBEDDED_WEB_ASSETS.len()
}

pub fn asset_for_uri_path(path: &str) -> Option<&'static EmbeddedWebAsset> {
    let normalized = normalize_uri_path(path)?;
    asset(&normalized).or_else(|| should_spa_fallback(&normalized).then(index_asset).flatten())
}

pub fn index_asset() -> Option<&'static EmbeddedWebAsset> {
    asset("index.html")
}

fn asset(path: &str) -> Option<&'static EmbeddedWebAsset> {
    EMBEDDED_WEB_ASSETS.iter().find(|asset| asset.path == path)
}

fn normalize_uri_path(path: &str) -> Option<String> {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() || trimmed.ends_with('/') {
        return Some("index.html".to_string());
    }
    let mut parts = Vec::new();
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        parts.push(part);
    }
    Some(parts.join("/"))
}

fn should_spa_fallback(path: &str) -> bool {
    if path.starts_with("api/")
        || path.starts_with("web-api/")
        || path.starts_with("v1/")
        || path.starts_with("v1beta/")
        || path.starts_with("_share-router/")
        || path.starts_with("_ctl/")
    {
        return false;
    }
    std::path::Path::new(path).extension().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_index_html() {
        let asset = index_asset().expect("embedded index.html");
        assert_eq!(asset.path, "index.html");
        assert!(asset.content_type.starts_with("text/html"));
        assert!(!asset.bytes.is_empty());
    }

    #[test]
    fn resolves_root_and_spa_routes_to_index() {
        assert_eq!(asset_for_uri_path("/").unwrap().path, "index.html");
        assert_eq!(asset_for_uri_path("/providers").unwrap().path, "index.html");
    }

    #[test]
    fn does_not_spa_fallback_api_routes_or_bad_paths() {
        assert!(asset_for_uri_path("/api/missing").is_none());
        assert!(asset_for_uri_path("/../index.html").is_none());
    }
}
