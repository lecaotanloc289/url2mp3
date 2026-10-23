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
    if out.is_empty() {
        "untitled".into()
    } else if is_reserved_on_windows(&out) {
        format!("_{out}")
    } else {
        out
    }
}

/// CON, NUL, COM1… name devices on Windows, with any extension: "nul.mp3" can't be a file.
fn is_reserved_on_windows(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

/// Longest path we create, in UTF-16 units: Windows' MAX_PATH is 260 including the drive and
/// the terminating NUL, and apps aren't long-path aware by default.
const MAX_PATH: usize = 250;

/// `name` shortened (whole characters) so that `dir/<name><suffix>` stays under MAX_PATH.
pub(crate) fn fit(dir: &Path, name: &str, suffix: &str) -> String {
    let fixed = dir.join(suffix).to_string_lossy().encode_utf16().count();
    let mut out = String::new();
    let mut used = fixed;
    for c in name.chars() {
        if used + c.len_utf16() > MAX_PATH {
            break;
        }
        used += c.len_utf16();
        out.push(c);
    }
    let out = out.trim_end().to_owned();
    if out.is_empty() { "untitled".into() } else { out }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{fit, is_network_error, sanitize};

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

    #[test]
    fn windows_device_names_are_escaped() {
        assert_eq!(sanitize("CON"), "_CON");
        assert_eq!(sanitize("nul.remix"), "_nul.remix");
        assert_eq!(sanitize("Com7"), "_Com7");
        assert_eq!(sanitize("COM0"), "COM0");
        assert_eq!(sanitize("Console"), "Console");
        assert_eq!(sanitize("LPT10"), "LPT10");
    }

    #[test]
    fn long_paths_are_cut_to_fit() {
        let dir = Path::new("/Users/someone/Downloads/").join("p".repeat(150));
        let name = fit(&dir, &"ạ".repeat(200), " [abc].mp3");
        let full = dir.join(format!("{name} [abc].mp3"));
        assert!(full.to_string_lossy().encode_utf16().count() <= 250);
        assert!(name.chars().count() > 50, "{}", name.chars().count());
        assert_eq!(fit(Path::new("/m"), "Song", ".mp3"), "Song");
    }
}
