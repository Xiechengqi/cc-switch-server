use anyhow::Context;

pub fn direct_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(same_origin_redirect_policy())
}

pub fn self_update_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(self_update_redirect_policy())
}

fn same_origin(previous: &reqwest::Url, target: &reqwest::Url) -> bool {
    previous.scheme() == target.scheme()
        && previous.host_str() == target.host_str()
        && previous.port_or_known_default() == target.port_or_known_default()
}

fn same_origin_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        let Some(origin) = attempt.previous().first() else {
            return attempt.follow();
        };
        if same_origin(origin, attempt.url()) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

fn self_update_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        let Some(origin) = attempt.previous().first() else {
            return attempt.follow();
        };
        let target = attempt.url();
        if same_origin(origin, target) {
            return attempt.follow();
        }
        if attempt.previous().iter().all(is_github_release_url) && is_github_release_url(target) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

fn is_github_release_url(url: &reqwest::Url) -> bool {
    if url.scheme() != "https" || url.port_or_known_default() != Some(443) {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    matches!(
        host,
        "github.com" | "api.github.com" | "githubusercontent.com"
    ) || host.ends_with(".githubusercontent.com")
}

pub fn direct_client() -> anyhow::Result<reqwest::Client> {
    direct_client_builder()
        .build()
        .context("build direct HTTP client")
}

#[cfg(test)]
mod tests {
    use reqwest::Url;

    #[test]
    fn direct_client_ignores_proxy_environment() {
        super::direct_client().unwrap();
    }

    #[test]
    fn self_update_redirects_are_limited_to_github_https_hosts() {
        for value in [
            "https://github.com/example/project/releases/download/latest/asset",
            "https://api.github.com/repos/example/project/releases/tags/latest",
            "https://release-assets.githubusercontent.com/github-production-release-asset/1/2",
            "https://objects.githubusercontent.com/github-production-release-asset/1/2",
        ] {
            assert!(super::is_github_release_url(&Url::parse(value).unwrap()));
        }
        for value in [
            "http://github.com/example/project/releases/download/latest/asset",
            "https://github.com:8443/example/project/releases/download/latest/asset",
            "https://github.com.example.invalid/asset",
            "https://example.invalid/asset",
        ] {
            assert!(!super::is_github_release_url(&Url::parse(value).unwrap()));
        }
    }
}
