//! Evaluation helpers shared by the diagnostic executable and its tests.
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Note {
    pub pitch: f64,
    pub start: f64,
    pub end: f64,
}
#[derive(Deserialize)]
pub struct Notes {
    pub notes: Vec<Note>,
}

pub fn scores(reference: &[Note], estimate: &[Note]) -> Result<serde_json::Value, &'static str> {
    if reference.len().saturating_mul(estimate.len()) > 4_000_000
        || reference.len().max(estimate.len()) > 10_000
    {
        return Err("evaluate shorter excerpts (at most 10000 notes / 4 million pairs)");
    }
    if reference.iter().chain(estimate).any(|n| {
        !n.pitch.is_finite()
            || !(0.0..=127.0).contains(&n.pitch)
            || !n.start.is_finite()
            || !n.end.is_finite()
            || n.start < 0.0
            || n.end <= n.start
    }) {
        return Err("invalid source-second note interval or MIDI pitch");
    }
    let metric = |offsets: bool| {
        let edges: Vec<Vec<usize>> = reference
            .iter()
            .map(|r| {
                estimate
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| {
                        (r.pitch - e.pitch).abs() <= 0.5 + 1e-9
                            && (r.start - e.start).abs() <= 0.05 + 1e-9
                            && (!offsets
                                || (r.end - e.end).abs()
                                    <= (0.2 * (r.end - r.start)).max(0.05) + 1e-9)
                    })
                    .map(|(i, _)| i)
                    .collect()
            })
            .collect();
        let matches = maximum_matching(&edges, estimate.len());
        let p = matches as f64 / estimate.len().max(1) as f64;
        let r = matches as f64 / reference.len().max(1) as f64;
        serde_json::json!({"matches":matches,"precision":p,"recall":r,"f1":if p+r > 0.0 {2.0*p*r/(p+r)} else {0.0},
            "false_positives":estimate.len()-matches,"false_negatives":reference.len()-matches})
    };
    Ok(
        serde_json::json!({"reference_notes":reference.len(),"estimated_notes":estimate.len(),
        "onset_seconds":0.05,"pitch_cents":50,"offset_seconds_min":0.05,"offset_duration_ratio":0.2,
        "onset":metric(false),"onset_offset":metric(true)}),
    )
}

fn maximum_matching(edges: &[Vec<usize>], estimates: usize) -> usize {
    let mut chosen = vec![None; edges.len()];
    let mut owner: Vec<Option<usize>> = vec![None; estimates];
    let mut count = 0;
    for root in 0..edges.len() {
        let mut parent = vec![None; estimates];
        let mut seen = vec![false; edges.len()];
        let mut queue = VecDeque::from([root]);
        seen[root] = true;
        let mut end = None;
        'search: while let Some(r) = queue.pop_front() {
            for &e in &edges[r] {
                if parent[e].is_some() {
                    continue;
                }
                parent[e] = Some(r);
                if let Some(next) = owner[e] {
                    if !seen[next] {
                        seen[next] = true;
                        queue.push_back(next);
                    }
                } else {
                    end = Some(e);
                    break 'search;
                }
            }
        }
        if end.is_some() {
            count += 1;
        }
        while let Some(e) = end {
            let r = parent[e].unwrap();
            end = chosen[r];
            chosen[r] = Some(e);
            owner[e] = Some(r);
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matching_reassigns_instead_of_greedily_losing_notes() {
        assert_eq!(maximum_matching(&[vec![0, 1], vec![0]], 2), 2);
        assert_eq!(maximum_matching(&[vec![0], vec![0]], 1), 1);
    }
    #[test]
    fn offsets_duplicates_empty_and_invalid_inputs_are_counted() {
        let r = Note {
            pitch: 60.0,
            start: 0.2,
            end: 0.6,
        };
        let e = Note {
            pitch: 60.0,
            start: 0.22,
            end: 1.0,
        };
        let m = scores(&[r.clone()], &[e.clone(), e]).unwrap();
        assert_eq!(m["onset"]["matches"], 1);
        assert_eq!(m["onset_offset"]["matches"], 0);
        assert_eq!(
            scores(&[], &[r.clone()]).unwrap()["onset"]["false_positives"],
            1
        );
        assert_eq!(scores(&[], &[]).unwrap()["onset"]["f1"], 0.0);
        assert!(
            scores(
                &[Note {
                    start: f64::NAN,
                    ..r
                }],
                &[]
            )
            .is_err()
        );
    }
}
