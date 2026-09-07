//! Exact deterministic top-k retention shared by the two memory stores.
use super::MemoryHit;
use std::{cmp::Ordering, collections::BinaryHeap};

struct Ranked(MemoryHit);
impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.0.score == other.0.score && self.0.record.id == other.0.record.id
    }
}

impl Eq for Ranked {}
impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        // The worst retained hit is the root: lowest score, then greatest ID.
        other
            .0
            .score
            .cmp(&self.0.score)
            .then_with(|| self.0.record.id.cmp(&other.0.record.id))
    }
}
impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub(super) struct TopHits {
    heap: BinaryHeap<Ranked>,
    limit: usize,
}
impl TopHits {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            heap: BinaryHeap::with_capacity(limit),
            limit,
        }
    }
    pub(super) fn push(&mut self, hit: MemoryHit) {
        if self.limit == 0 {
            return;
        }
        let hit = Ranked(hit);
        if self.heap.len() < self.limit {
            self.heap.push(hit);
        } else if let Some(mut worst) = self.heap.peek_mut()
            && hit < *worst
        {
            *worst = hit;
        }
    }
    pub(super) fn finish(self) -> Vec<MemoryHit> {
        self.heap
            .into_sorted_vec()
            .into_iter()
            .map(|hit| hit.0)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_retention_matches_full_sort_for_scores_ties_and_zero_limit() {
        let hits: Vec<_> = (0..1000)
            .rev()
            .map(|index| MemoryHit {
                record: crate::tests::sample_record(&format!("m{index:04}"), "tenant"),
                score: (index % 17) * 123,
                matched: super::super::MatchEvidence::ExactId,
            })
            .collect();
        for limit in [0, 1, 8, 256] {
            let mut expected = hits.clone();
            expected.sort_by(|a, b| {
                b.score
                    .cmp(&a.score)
                    .then_with(|| a.record.id.cmp(&b.record.id))
            });
            expected.truncate(limit);
            let mut retained = TopHits::new(limit);
            for hit in hits.iter().cloned() {
                retained.push(hit);
                assert!(retained.heap.len() <= limit);
            }
            assert_eq!(retained.finish(), expected);
        }
    }
}
