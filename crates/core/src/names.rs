//! File names and error classification shared by the pipeline.

use std::path::Path;

/// DNS/connection trouble on the user's side, not a YouTube change.
pub(crate) fn is_network_error(msg: &str) -> bool {
    const SIGNS: [&str; 7] = [
        "dns error",
        "failed to resolve",
        "nodename nor servname",
        "network is unreachable",
        "connection refused",
        "connection reset",
        "timed out",
    ];
    let msg = msg.to_lowercase();
    SIGNS.iter().any(|s| msg.contains(s))
}

/// Case-insensitive: APFS and NTFS treat "Track.mp3" and "TRACK.mp3" as one file.
pub(crate) fn path_key(path: &Path) -> String {
    format!("path:{}", path.to_string_lossy().to_lowercase())
}

/// Safe file name on macOS/Windows/Linux, ≤ 180 bytes.
pub(crate) fn sanitize(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | '*' | '?' | '"' | '<' | '>' => '_',
            ':' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let squeezed = mapped.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = squeezed.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let mut out = String::new();
    for c in trimmed.chars() {
        if out.len() + c.len_utf8() > 180 {
            break;
        }
        out.push(c);
    }
    let out = out.trim_end().to_owned();
    if out.is_empty() { "untitled".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::{is_network_error, sanitize};

    #[test]
    fn classifies_network_errors() {
        // Real messages from a run where the local DNS stopped resolving googlevideo hosts.
        assert!(is_network_error(
            "[download] Got error: HTTPSConnection(host='rr4---sn-oji3bc-5j.googlevideo.com', port=443): \
             Failed to resolve 'rr4---sn-oji3bc-5j.googlevideo.com' ([Errno 8] nodename nor servname \
             provided, or not known). Giving up after 10 retries"
        ));
        assert!(is_network_error(
            "error sending request: client error (Connect): dns error: failed to lookup address information"
        ));
        assert!(!is_network_error("Video unavailable. This video is private"));
        assert!(!is_network_error("Sign in to confirm you're not a bot"));
    }

    #[test]
    fn sanitize_names() {
        assert_eq!(
            sanitize("LẠC TRÔI | OFFICIAL MUSIC VIDEO | SƠN TÙNG M-TP"),
            "LẠC TRÔI - OFFICIAL MUSIC VIDEO - SƠN TÙNG M-TP"
        );
        assert_eq!(sanitize("a/b:c?  \n d"), "a_b-c_ d");
        assert_eq!(sanitize("...hidden"), "hidden");
        assert_eq!(sanitize("   "), "untitled");
        assert!(sanitize(&"ạ".repeat(200)).len() <= 180);
    }
}
