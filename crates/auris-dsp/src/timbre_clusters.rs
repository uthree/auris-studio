//! Bounded, deterministic refinement of populous acoustic groups.

use super::{MAX_TIMBRE_CLUSTERS, distance, kmeans, lloyd};

pub(super) fn refine(rows: &[Vec<f64>], labels: Vec<usize>, max_groups: usize) -> Vec<usize> {
    let Some(&last) = labels.iter().max() else {
        return labels;
    };
    let mut groups = vec![Vec::new(); last + 1];
    for (i, &label) in labels.iter().enumerate() {
        groups[label].push(i);
    }
    groups.retain(|group| !group.is_empty());
    let target = (2 * rows.len().div_ceil(groups.len())).max(12);
    let mut tried = vec![false; groups.len()];
    while groups.len() < max_groups.min(MAX_TIMBRE_CLUSTERS) {
        let selected = groups
            .iter()
            .enumerate()
            .filter(|(i, members)| !tried[*i] && members.len() > target)
            .max_by(|(a, members), (b, other)| members.len().cmp(&other.len()).then(b.cmp(a)))
            .map(|(i, _)| i);
        let Some(index) = selected else { break };
        let members = &groups[index];
        let data: Vec<_> = members.iter().map(|&i| rows[i].as_slice()).collect();
        let parent_error = squared_error(&data, &vec![0; data.len()]);
        let Some((split, error)) = binary_partition(&data) else {
            tried[index] = true;
            continue;
        };
        // A population target is a refinement hint, never a reason to cut duplicate sounds or
        // a diffuse group whose best candidate barely improves its acoustic fit.
        if parent_error <= 1e-10 || error > parent_error * 0.95 {
            tried[index] = true;
            continue;
        }
        let mut left = Vec::new();
        let mut right = Vec::new();
        for (&member, child) in members.iter().zip(split) {
            if child == 0 {
                left.push(member);
            } else {
                right.push(member);
            }
        }
        groups[index] = left;
        groups.push(right);
        tried.push(false);
    }
    let mut refined = vec![0; labels.len()];
    for (id, members) in groups.iter().enumerate() {
        for &member in members {
            refined[member] = id;
        }
    }
    refined
}

fn squared_error(rows: &[&[f64]], labels: &[usize]) -> f64 {
    let groups = labels.iter().max().map_or(0, |id| id + 1);
    let mut means = vec![vec![0.; rows[0].len()]; groups];
    let mut counts = vec![0; groups];
    for (row, &id) in rows.iter().zip(labels) {
        counts[id] += 1;
        for (mean, value) in means[id].iter_mut().zip(row.iter()) {
            *mean += value;
        }
    }
    for (mean, count) in means.iter_mut().zip(counts) {
        if count > 0 {
            for value in mean {
                *value /= count as f64;
            }
        }
    }
    rows.iter()
        .zip(labels)
        .map(|(row, &id)| distance(row, &means[id]))
        .sum()
}

fn binary_partition(rows: &[&[f64]]) -> Option<(Vec<usize>, f64)> {
    let mut best = None;
    // Retain the coarse method as one candidate. Fixed-seed D² restarts also give dense
    // subgroups a chance when farthest-first seeding spends a center on a rare extreme.
    for restart in 0..9u32 {
        let labels = if restart == 0 {
            kmeans(rows, 2)
        } else {
            let mut state = restart.wrapping_mul(0x9e37_79b9);
            let first = (random(&mut state) * rows.len() as f64) as usize;
            let weights: Vec<_> = rows.iter().map(|row| distance(row, rows[first])).collect();
            let total: f64 = weights.iter().sum();
            if total <= 1e-10 {
                continue;
            }
            let mut draw = random(&mut state) * total;
            let second = weights
                .iter()
                .position(|weight| {
                    draw -= weight;
                    draw < 0.
                })
                .unwrap_or(rows.len() - 1);
            lloyd(rows, vec![rows[first].to_vec(), rows[second].to_vec()])
        };
        let left = labels.iter().filter(|&&id| id == 0).count();
        if left < 3 || rows.len() - left < 3 {
            continue;
        }
        let error = squared_error(rows, &labels);
        if best.as_ref().is_none_or(|(_, previous)| error < *previous) {
            best = Some((labels, error));
        }
    }
    best
}

fn random(state: &mut u32) -> f64 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    f64::from(*state) / (f64::from(u32::MAX) + 1.)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalogue() -> (Vec<Vec<f64>>, Vec<usize>) {
        let mut rows = Vec::new();
        let mut labels = Vec::new();
        // Three dense lobes were absorbed into one coarse group; five small groups are distinct.
        for center in [-3., 0., 3.] {
            for i in 0..30 {
                rows.push(vec![center + (i % 3) as f64 * 0.01, 0.]);
                labels.push(0);
            }
        }
        for group in 1..6 {
            for _ in 0..2 {
                rows.push(vec![100. + group as f64, 100.]);
                labels.push(group);
            }
        }
        (rows, labels)
    }

    #[test]
    fn populous_mixed_groups_split_while_small_groups_keep_their_members() {
        let (rows, original) = catalogue();
        let labels = refine(&rows, original.clone(), 8);
        assert_ne!(labels[0], labels[30]);
        assert_ne!(labels[30], labels[60]);
        assert_ne!(labels[0], labels[60]);
        for (a, &group) in original.iter().enumerate().skip(90) {
            for (b, &other) in original.iter().enumerate() {
                assert_eq!(labels[a] == labels[b], group == other);
            }
        }
        assert_eq!(labels, refine(&rows, original, 8));
    }

    #[test]
    fn refinement_keeps_constant_groups_intact_and_obeys_its_budget() {
        let (mut rows, original) = catalogue();
        let bounded = refine(&rows, original.clone(), 7);
        assert_eq!(bounded.iter().max(), Some(&6));
        rows[..90].fill(vec![1., 0.]);
        assert_eq!(refine(&rows, original.clone(), 24), original);
    }

    #[test]
    fn diffuse_high_dimensional_groups_do_not_split_just_to_meet_a_population_target() {
        // Orthogonal Walsh rows have no dominant separating direction. Any binary split
        // explains less than 1% of this group's variance, despite its large population.
        let mut rows: Vec<Vec<f64>> = (0..128usize)
            .map(|i| {
                (0..128usize)
                    .map(|j| {
                        if (i & j).count_ones().is_multiple_of(2) {
                            1.
                        } else {
                            -1.
                        }
                    })
                    .collect()
            })
            .collect();
        let mut labels = vec![0; rows.len()];
        for group in 1..6 {
            rows.extend(std::iter::repeat_n(vec![group as f64 + 10.; 128], 2));
            labels.extend([group; 2]);
        }
        assert_eq!(refine(&rows, labels.clone(), 24), labels);
    }
}
