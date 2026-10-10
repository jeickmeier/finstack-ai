//! Exact deterministic top-k retention shared by the two memory stores.
use super::MemoryHit;
use crate::record::MemoryId;
use std::{cmp::Ordering, collections::BinaryHeap};

/// A rankable item: ordered by descending `score`, then ascending `id`.
pub(super) trait Ranked {
    fn score(&self) -> u32;
    fn id(&self) -> &MemoryId;
}

impl Ranked for MemoryHit {
    fn score(&self) -> u32 {
        self.score
    }

    fn id(&self) -> &MemoryId {
        &self.record.id
    }
}

struct Entry<T>(T);
impl<T: Ranked> PartialEq for Entry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.score() == other.0.score() && self.0.id() == other.0.id()
    }
}

impl<T: Ranked> Eq for Entry<T> {}
impl<T: Ranked> Ord for Entry<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // The worst retained hit is the root: lowest score, then greatest ID.
        other
            .0
            .score()
            .cmp(&self.0.score())
            .then_with(|| self.0.id().cmp(other.0.id()))
    }
}
impl<T: Ranked> PartialOrd for Entry<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub(super) struct TopHits<T: Ranked> {
    heap: BinaryHeap<Entry<T>>,
    limit: usize,
}
impl<T: Ranked> TopHits<T> {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            heap: BinaryHeap::with_capacity(limit),
            limit,
        }
    }
    pub(super) fn push(&mut self, hit: T) {
        if self.limit == 0 {
            return;
        }
        let hit = Entry(hit);
        if self.heap.len() < self.limit {
            self.heap.push(hit);
        } else if let Some(mut worst) = self.heap.peek_mut()
            && hit < *worst
        {
            *worst = hit;
        }
    }
    /// The retained items, best first.
    pub(super) fn finish(self) -> Vec<T> {
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

    #[test]
    fn any_ranked_item_orders_like_hits() {
        struct Scored(u32, MemoryId);
        impl Ranked for Scored {
            fn score(&self) -> u32 {
                self.0
            }
            fn id(&self) -> &MemoryId {
                &self.1
            }
        }
        let mut ids = TopHits::new(3);
        for (score, id) in [(5, "b"), (5, "a"), (9, "z"), (1, "c"), (9, "y")] {
            ids.push(Scored(score, MemoryId::parse(id).unwrap()));
        }
        let ranked: Vec<(u32, String)> = ids
            .finish()
            .iter()
            .map(|entry| (entry.0, entry.1.as_str().to_owned()))
            .collect();
        let expected: Vec<(u32, String)> = [(9, "y"), (9, "z"), (5, "a")]
            .iter()
            .map(|(score, id)| (*score, (*id).to_owned()))
            .collect();
        assert_eq!(ranked, expected);
    }
}
