use anyhow::Context;

pub fn direct_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(same_origin_redirect_policy())
}

fn same_origin_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        let Some(origin) = attempt.previous().first() else {
            return attempt.follow();
        };
        let target = attempt.url();
        let same_origin = origin.scheme() == target.scheme()
            && origin.host_str() == target.host_str()
            && origin.port_or_known_default() == target.port_or_known_default();
        if same_origin {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

pub fn direct_client() -> anyhow::Result<reqwest::Client> {
    direct_client_builder()
        .build()
        .context("build direct HTTP client")
}

#[cfg(test)]
mod tests {
    #[test]
    fn direct_client_ignores_proxy_environment() {
        super::direct_client().unwrap();
    }
}
