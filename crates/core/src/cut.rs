//! Parts of a track the user keeps: clean-up of the ranges and the ffmpeg graph that joins them.

use std::fmt::Write;

use serde::{Deserialize, Serialize};

/// Length of the fade-out/fade-in applied to the inner edges of a join, in seconds.
const FADE: f64 = 0.015;
/// Ranges shorter than this are noise from a slider, not something to keep.
const MIN_LEN: f64 = 0.05;
/// Ranges closer than this are one range.
const MERGE_GAP: f64 = 0.01;

/// A span of the source, in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Range {
    pub start: f64,
    pub end: f64,
}

/// What to keep of a track. `label` is the text appended to the file name, e.g. "đã cắt".
#[derive(Clone, Debug, Deserialize)]
pub struct Cut {
    pub keep: Vec<Range>,
    pub label: String,
}

/// Drops invalid and tiny ranges, then sorts and merges overlapping or touching ones.
pub fn normalize(keep: &[Range]) -> Vec<Range> {
    let mut ranges: Vec<Range> = keep
        .iter()
        .filter(|r| r.start.is_finite() && r.end.is_finite())
        .map(|r| Range { start: r.start.max(0.0), end: r.end })
        .filter(|r| r.end - r.start >= MIN_LEN)
        .collect();
    ranges.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut merged: Vec<Range> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start - last.end < MERGE_GAP => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

/// The `-filter_complex` value that cuts `keep` out of input 0 and joins it into `[out]`.
/// Only the inner edges of a join are faded, so the track's start and end stay untouched.
pub(crate) fn filter_graph(keep: &[Range]) -> String {
    let n = keep.len();
    let mut graph = String::new();
    for (i, r) in keep.iter().enumerate() {
        let _ = write!(
            graph,
            "[0:a]atrim=start={:.3}:end={:.3},asetpts=PTS-STARTPTS",
            r.start, r.end
        );
        if i > 0 {
            let _ = write!(graph, ",afade=t=in:d={FADE}");
        }
        if i + 1 < n {
            let st = (r.end - r.start - FADE).max(0.0);
            let _ = write!(graph, ",afade=t=out:st={st:.3}:d={FADE}");
        }
        let _ = write!(graph, "[a{i}];");
    }
    for i in 0..n {
        let _ = write!(graph, "[a{i}]");
    }
    let _ = write!(graph, "concat=n={n}:v=0:a=1[out]");
    graph
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_sorts_merges_and_drops_slivers() {
        let r = |start, end| Range { start, end };
        assert_eq!(
            normalize(&[r(10.0, 20.0), r(-3.0, 4.0), r(18.0, 25.0), r(30.0, 30.02), r(f64::NAN, 1.0)]),
            vec![r(0.0, 4.0), r(10.0, 25.0)]
        );
        assert_eq!(normalize(&[r(5.0, 6.0), r(6.005, 9.0)]), vec![r(5.0, 9.0)]);
        assert!(normalize(&[]).is_empty());
    }

    #[test]
    fn filter_graph_fades_only_inner_edges() {
        let r = |start, end| Range { start, end };
        assert_eq!(
            filter_graph(&[r(1.0, 3.0), r(6.0, 10.0)]),
            "[0:a]atrim=start=1.000:end=3.000,asetpts=PTS-STARTPTS,afade=t=out:st=1.985:d=0.015[a0];\
             [0:a]atrim=start=6.000:end=10.000,asetpts=PTS-STARTPTS,afade=t=in:d=0.015[a1];\
             [a0][a1]concat=n=2:v=0:a=1[out]"
        );
        assert_eq!(
            filter_graph(&[r(28.0, 245.0)]),
            "[0:a]atrim=start=28.000:end=245.000,asetpts=PTS-STARTPTS[a0];[a0]concat=n=1:v=0:a=1[out]"
        );
    }
}
