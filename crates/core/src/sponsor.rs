//! Trim suggestions from SponsorBlock (YouTube only; the caller checks the extractor). Only the
//! first 4 hex chars of the video id's SHA-256 leave the machine. Any failure means no suggestions.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const API: &str = "https://sponsor.ajay.app/api/skipSegments/";
/// `["music_offtopic","sponsor",...]`, percent-encoded.
const CATEGORIES: &str = "%5B%22music_offtopic%22%2C%22sponsor%22%2C%22intro%22%2C%22outro%22%2C%22selfpromo%22%2C%22interaction%22%2C%22preview%22%5D";

/// A non-music part of a video that SponsorBlock suggests cutting, in seconds.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub category: String,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "videoID")]
    video_id: String,
    segments: Vec<Raw>,
}

#[derive(Deserialize)]
struct Raw {
    category: String,
    #[serde(rename = "actionType")]
    action_type: String,
    segment: [f64; 2],
}

fn url(video_id: &str) -> String {
    let digest = Sha256::digest(video_id.as_bytes());
    format!(
        "{API}{:02x}{:02x}?categories={CATEGORIES}&actionType=skip",
        digest[0], digest[1]
    )
}

fn parse(body: &str, video_id: &str) -> Vec<Segment> {
    let Ok(entries) = serde_json::from_str::<Vec<Entry>>(body) else {
        return Vec::new();
    };
    let mut out: Vec<Segment> = entries
        .into_iter()
        .filter(|e| e.video_id == video_id)
        .flat_map(|e| e.segments)
        .filter(|s| s.action_type == "skip" && s.segment[1] > s.segment[0])
        .map(|s| Segment {
            start: s.segment[0],
            end: s.segment[1],
            category: s.category,
        })
        .collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// SponsorBlock's skip suggestions for a YouTube video; empty on any error or after 5 s.
pub async fn suggestions(client: &reqwest::Client, video_id: &str) -> Vec<Segment> {
    let fetch = async {
        let response = client
            .get(url(video_id))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?;
        response.text().await.ok()
    };
    match tokio::time::timeout(Duration::from_secs(5), fetch).await {
        Ok(Some(body)) => parse(&body, video_id),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_sends_only_a_hash_prefix() {
        let u = url("Llw9Q6akRo4");
        assert!(u.starts_with("https://sponsor.ajay.app/api/skipSegments/"));
        assert!(!u.contains("Llw9Q6akRo4"));
        let prefix = u
            .trim_start_matches("https://sponsor.ajay.app/api/skipSegments/")
            .split('?')
            .next()
            .unwrap();
        assert_eq!(prefix.len(), 4);
        assert!(u.contains("categories=")); // the 7 categories, JSON array, URL-encoded
    }

    #[test]
    fn parse_keeps_this_video_skip_segments_sorted() {
        let body = r#"[
          {"videoID":"other","segments":[{"category":"sponsor","actionType":"skip","segment":[1,2]}]},
          {"videoID":"abc","segments":[
            {"category":"outro","actionType":"skip","segment":[245.0,272.0]},
            {"category":"music_offtopic","actionType":"skip","segment":[0,28.4]},
            {"category":"sponsor","actionType":"mute","segment":[100,110]}]}]"#;
        assert_eq!(
            parse(body, "abc"),
            vec![
                Segment {
                    start: 0.0,
                    end: 28.4,
                    category: "music_offtopic".into()
                },
                Segment {
                    start: 245.0,
                    end: 272.0,
                    category: "outro".into()
                },
            ]
        );
        assert!(parse("not json", "abc").is_empty());
    }
}
