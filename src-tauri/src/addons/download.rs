use super::{ErrorCode, ProtocolError, Result};
use std::time::Duration;
fn unavailable() -> ProtocolError {
    ProtocolError::new(ErrorCode::NetworkDenied, "Add-on download failed")
}
// GitHub's documented asset hosts, not *.githubusercontent.com. See
// https://docs.github.com/en/actions/reference/runners/github-hosted-runners
pub fn redirect_allowed(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && matches!(
            url.host_str(),
            Some(
                "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
                    | "github-releases.githubusercontent.com"
            )
        )
}
/// The next hop of a redirect: only release assets may redirect, at most three
/// times, and only to the documented asset hosts.
fn next_hop(
    current: &url::Url,
    location: Option<&str>,
    assets: bool,
    hop: usize,
) -> Result<url::Url> {
    if !assets || hop >= 3 {
        return Err(unavailable());
    }
    let next = current
        .join(location.ok_or_else(unavailable)?)
        .map_err(|_| unavailable())?;
    if !redirect_allowed(&next) {
        return Err(unavailable());
    }
    Ok(next)
}
pub async fn download(start: &str, maximum: usize, assets: bool) -> Result<Vec<u8>> {
    let work = async {
        let mut url = url::Url::parse(start).map_err(|_| unavailable())?;
        for hop in 0..=3 {
            let host = url.host_str().ok_or_else(unavailable)?.to_owned();
            let addresses = tokio::net::lookup_host((host.as_str(), 443))
                .await
                .map_err(|_| unavailable())?
                .collect::<Vec<_>>();
            if addresses.is_empty()
                || addresses
                    .iter()
                    .any(|a| !super::http::public_address(a.ip()))
            {
                return Err(unavailable());
            }
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(30))
                .resolve_to_addrs(&host, &addresses)
                .build()
                .map_err(|_| unavailable())?;
            let mut response = client
                .get(url.clone())
                .header("User-Agent", "CodeMux-Addon-Catalog/1")
                .header("Accept-Encoding", "identity")
                .send()
                .await
                .map_err(|_| unavailable())?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok());
                url = next_hop(&url, location, assets, hop)?;
                continue;
            }
            if !response.status().is_success()
                || response
                    .headers()
                    .get("content-encoding")
                    .is_some_and(|v| v != "identity")
                || response
                    .content_length()
                    .is_some_and(|n| n > maximum as u64)
            {
                return Err(unavailable());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
                if bytes.len() + chunk.len() > maximum {
                    return Err(unavailable());
                }
                bytes.extend_from_slice(&chunk);
            }
            return Ok(bytes);
        }
        Err(unavailable())
    };
    tokio::time::timeout(Duration::from_secs(60), work)
        .await
        .map_err(|_| unavailable())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_release_assets_redirect_three_times_to_documented_hosts() {
        let start =
            url::Url::parse("https://github.com/example/hello/releases/download/v1/hello.cmxaddon")
                .unwrap();
        let asset = "https://objects.githubusercontent.com/asset?signature=opaque";
        let mut url = start.clone();
        for hop in 0..3 {
            url = next_hop(&url, Some(asset), true, hop).unwrap();
        }
        assert_eq!(url.as_str(), asset);
        assert!(next_hop(&url, Some(asset), true, 3).is_err(), "fourth hop");
        assert!(
            next_hop(&start, Some(asset), false, 0).is_err(),
            "catalog redirect"
        );
        assert!(next_hop(&start, None, true, 0).is_err(), "missing location");
        for location in [
            "/example/hello/raw/main/hello.cmxaddon",
            "https://example.com/hello.cmxaddon",
            "http://objects.githubusercontent.com/asset",
            "https://token@objects.githubusercontent.com/asset",
        ] {
            assert!(
                next_hop(&start, Some(location), true, 0).is_err(),
                "{location}"
            );
        }
    }
    #[tokio::test]
    async fn non_public_addresses_are_refused_before_connecting() {
        for url in [
            "https://127.0.0.1/addons/catalog-v1.json",
            "https://[::1]/addons/catalog-v1.json",
            "https://10.0.0.1/addons/catalog-v1.json",
            "https://169.254.169.254/latest/meta-data",
            "https://[::ffff:127.0.0.1]/addons/catalog-v1.json",
        ] {
            let started = std::time::Instant::now();
            assert!(download(url, 1024, true).await.is_err(), "{url}");
            assert!(started.elapsed() < Duration::from_secs(5), "{url}");
        }
    }
}
