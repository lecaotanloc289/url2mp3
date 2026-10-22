//! "A new version of the app is out" banner. Only tells: the user downloads it themselves.

use anyhow::Result;
use reqwest::Client;
use serde::Serialize;

const RELEASES: &str = "https://github.com/lecaotanloc289/url2mp3/releases";

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NewVersion {
    pub version: String,
    pub url: String,
}

/// The latest GitHub release, if it is newer than this build.
pub async fn check(client: &Client) -> Result<Option<NewVersion>> {
    // …/releases/latest redirects to …/releases/tag/<tag>, or to …/releases when there is none.
    let resp = client.get(format!("{RELEASES}/latest")).send().await?;
    let url = resp.url().clone();
    let Some(tag) = url.path().rsplit_once("/releases/tag/").map(|(_, tag)| tag) else {
        return Ok(None);
    };
    Ok(is_newer(tag, env!("CARGO_PKG_VERSION")).then(|| NewVersion {
        version: tag.trim_start_matches('v').to_owned(),
        url: url.to_string(),
    }))
}

/// `v1.2.10` > `1.2.9`. Tags that aren't x.y.z never count as newer.
fn is_newer(tag: &str, current: &str) -> bool {
    match (parse(tag), parse(current)) {
        (Some(tag), Some(current)) => tag > current,
        _ => false,
    }
}

fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim_start_matches('v').splitn(3, '.');
    let mut next = || parts.next()?.parse().ok();
    Some((next()?, next()?, next()?))
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v1.2.10", "1.2.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
        assert!(!is_newer("v1.0.0-beta", "0.1.0"));
    }
}
