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
                if !assets || hop == 3 {
                    return Err(unavailable());
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(unavailable)?;
                let next = url.join(location).map_err(|_| unavailable())?;
                if !redirect_allowed(&next) {
                    return Err(unavailable());
                }
                url = next;
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
