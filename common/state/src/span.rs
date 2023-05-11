use std::convert::TryFrom;

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SpanError {
    #[error("Invalid Span")]
    InvalidSpan,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SpanCoalesced {
    Overlap(Span),
    IdentityOverlap(Span),
    LeftIdentityOverlap(Span, Span),
    RightIdentityOverlap(Span, Span),
    LeftIdentity(Span, Span),
    RightIdentity(Span, Span),
    LeftOverlap(Span, Span),
    RightOverlap(Span, Span),
    Disjoint(Span, Span, Span),
}

impl SpanCoalesced {
    fn new(left_res: MergeResult, right_res: MergeResult) -> Self {
        match (left_res, right_res) {
            (MergeResult::Merged(_), MergeResult::Merged(span)) => Self::Overlap(span),
            (MergeResult::Identity(span), MergeResult::Identity(_)) => Self::IdentityOverlap(span),
            (MergeResult::Identity(left_span), MergeResult::Merged(right_span)) => {
                Self::LeftIdentityOverlap(left_span, right_span)
            }
            (MergeResult::Merged(left_span), MergeResult::Identity(right_span)) => {
                Self::RightIdentityOverlap(left_span, right_span)
            }
            (MergeResult::Identity(left_span), MergeResult::UnMerged(_, right_span)) => {
                Self::LeftIdentity(left_span, right_span)
            }
            (MergeResult::UnMerged(left_span, _), MergeResult::Identity(right_span)) => {
                Self::RightIdentity(left_span, right_span)
            }
            (MergeResult::Merged(left_span), MergeResult::UnMerged(_, right_span)) => {
                Self::LeftOverlap(left_span, right_span)
            }
            (MergeResult::UnMerged(left_span, _), MergeResult::Merged(right_span)) => {
                Self::RightOverlap(left_span, right_span)
            }
            (MergeResult::UnMerged(left_span, orig), MergeResult::UnMerged(_, right_span)) => {
                Self::Disjoint(left_span, orig, right_span)
            }
        }
    }
}

#[derive(Copy, Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Span {
    pub start: u64,
    pub end: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum MergeResult {
    Merged(Span),
    Identity(Span),
    UnMerged(Span, Span),
}

impl Span {
    pub fn new(start: u64, end: u64) -> Result<Span, SpanError> {
        if start > end {
            Err(SpanError::InvalidSpan)
        } else {
            Ok(Span { start, end })
        }
    }

    fn merge(self, right: Span) -> MergeResult {
        match self.merge_right(right) {
            MergeResult::UnMerged(left, right) => match right.merge_right(left) {
                MergeResult::Merged(merged) => MergeResult::Merged(merged),
                _ => MergeResult::UnMerged(left, right),
            },
            res => res,
        }
    }

    fn merge_right(self, right: Span) -> MergeResult {
        if self == right {
            MergeResult::Identity(self)
        } else if self.start <= right.start && self.end + 1 >= right.start {
            // Safe to unwrap, Span::new ensures that self.end > self.start
            // and other.start > other.end
            MergeResult::Merged(
                Span::new(
                    std::cmp::min(self.start, right.start),
                    std::cmp::max(self.end, right.end),
                )
                .unwrap(),
            )
        } else {
            MergeResult::UnMerged(self, right)
        }
    }

    fn coalesce(self, left_other: Span, right_other: Span) -> SpanCoalesced {
        let (left_merge, right_merge) = {
            let left_merge = left_other.merge_right(self);

            match left_merge {
                MergeResult::Identity(merged) | MergeResult::Merged(merged) => {
                    (left_merge, merged.merge_right(right_other))
                }
                _ => (left_merge, self.merge_right(right_other)),
            }
        };
        SpanCoalesced::new(left_merge, right_merge)
    }
}

impl TryFrom<(u64, u64)> for Span {
    type Error = SpanError;

    fn try_from(tup: (u64, u64)) -> Result<Span, SpanError> {
        Span::new(tup.0, tup.1)
    }
}

impl TryFrom<&(u64, u64)> for Span {
    type Error = SpanError;

    fn try_from(tup: &(u64, u64)) -> Result<Span, SpanError> {
        Span::new(tup.0, tup.1)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SpanVec {
    spans: SmallVec<[Span; 12]>,
}

impl SpanVec {
    pub fn new() -> Self {
        SpanVec {
            spans: SmallVec::new(),
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        SpanVec {
            spans: SmallVec::with_capacity(cap),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    pub fn len(&self) -> usize {
        self.spans.len()
    }

    pub fn pop_first(&mut self) -> Option<Span> {
        if !self.is_empty() {
            Some(self.spans.remove(0))
        } else {
            None
        }
    }

    pub fn first(&self) -> Option<&Span> {
        self.spans.first()
    }

    pub fn last(&self) -> Option<&Span> {
        self.spans.last()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Span> {
        self.spans.iter()
    }

    pub fn insert(&mut self, mut elem: Span) {
        // left most idx of partition window either triple or pair
        #[derive(Debug)]
        enum Window {
            Pair(usize),
            Triple(usize),
        }

        fn partition_to_window(len: usize, idx: usize) -> Option<Window> {
            match (idx, len) {
                // There's vec is empty, no window available
                (_, len) if len == 0 => None,
                // We're at the start of the vec
                (idx, _) if idx == 0 => Some(Window::Pair(idx)),
                // We're at the end of the vec
                (idx, len) if idx == len => Some(Window::Pair(idx - 1)),
                (idx, _) => Some(Window::Triple(idx - 1)),
            }
        }

        // Utility funciton to actuall merge in changes
        // Returns Some((rightmost_changed_idx, Span)) when the merge results in a modification
        fn merge_in_span(
            spans: &mut SmallVec<[Span; 12]>,
            elem: Span,
            insert_idx: usize,
        ) -> Option<(usize, Span)> {
            if let Some(window) = partition_to_window(spans.len(), insert_idx) {
                match window {
                    // It's a short list or we're at the end
                    Window::Pair(l_idx) => {
                        let left_elem = spans[l_idx];
                        match left_elem.merge(elem) {
                            MergeResult::Identity(_) => {}
                            MergeResult::Merged(new_span) => {
                                spans[l_idx] = new_span;
                            }
                            MergeResult::UnMerged(_, _) => {
                                spans.insert(insert_idx, elem);
                            }
                        }
                        None
                    }
                    Window::Triple(l_idx) => {
                        // Get the existing spans, safe to unwrap as Window Triple requires len > 2
                        let [left, right] = <[Span; 2]>::try_from(&spans[l_idx..=l_idx + 1])
                            .ok()
                            .unwrap();
                        match elem.coalesce(left, right) {
                            // Overlap(Span),
                            SpanCoalesced::Overlap(new_span) => {
                                // Remove the right Span
                                spans.remove(l_idx + 1);
                                // Replace span under cursor with coalesced
                                spans[l_idx] = new_span;
                                Some((l_idx + 1, new_span))
                            }
                            // LeftOverlap(Span, Span),
                            SpanCoalesced::LeftOverlap(l_span, _) => {
                                spans[l_idx] = l_span;
                                Some((l_idx, l_span))
                            }
                            // RightOverlap(Span, Span),
                            SpanCoalesced::RightOverlap(_, new_span) => {
                                spans[l_idx + 1] = new_span;
                                Some((l_idx + 1, new_span))
                            }
                            // Disjoint(Span, Span, Span)
                            SpanCoalesced::Disjoint(_, elem, _) => {
                                spans.insert(l_idx + 1, elem);
                                None
                            }
                            // IdentityOverlap(Span),
                            SpanCoalesced::IdentityOverlap(elem) => {
                                // We've got duplicates in the vec, somehow...
                                spans.remove(l_idx + 1);
                                spans.remove(l_idx + 2);
                                Some((l_idx + 2, elem))
                            }
                            // LeftIdentityOverlap(Span, Span),
                            SpanCoalesced::LeftIdentityOverlap(_, new_span) => {
                                // We've got an unmerged overlap
                                // Overwrite the current index and cleanup the next elem
                                spans[l_idx] = new_span;
                                spans.remove(l_idx + 1);
                                Some((l_idx + 1, new_span))
                            }
                            // RightIdentityOverlap(Span, Span),
                            SpanCoalesced::RightIdentityOverlap(new_span, _) => {
                                // We've got an unmerged overlap
                                // Overwrite the current index and cleanup the next elem
                                spans[l_idx] = new_span;
                                spans.remove(l_idx + 1);
                                Some((l_idx + 1, new_span))
                            }
                            // LeftIdentity(Span, Span), RightIdentity(Span, Span)
                            SpanCoalesced::LeftIdentity(_, _)
                            | SpanCoalesced::RightIdentity(_, _) => {
                                // We're the same as the do nothing
                                None
                            }
                        }
                    }
                }
            } else {
                spans.insert(insert_idx, elem);
                None
            }
        }

        let elem_start = elem.start;
        // The location the span would be inserted in the the span
        let mut insert_idx = self.spans.partition_point(|&x| elem_start > x.start);

        while let Some((merge_idx, merge_elem)) = merge_in_span(&mut self.spans, elem, insert_idx) {
            insert_idx = merge_idx;
            elem = merge_elem;
        }
    }
}

impl Default for SpanVec {
    fn default() -> Self {
        Self::new()
    }
}

impl std::iter::FromIterator<Span> for SpanVec {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = Span>,
    {
        let iter = iter.into_iter();
        let mut ret = SpanVec::with_capacity(iter.size_hint().0);
        for s in iter {
            ret.insert(s);
        }
        ret
    }
}

impl<'a> std::iter::FromIterator<&'a Span> for SpanVec {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = &'a Span>,
    {
        let iter = iter.into_iter();
        let mut ret = SpanVec::with_capacity(iter.size_hint().0);
        for s in iter {
            ret.insert(*s);
        }
        ret
    }
}

impl From<&[Span]> for SpanVec {
    fn from(spans: &[Span]) -> SpanVec {
        spans.iter().cloned().collect()
    }
}

impl std::ops::Index<usize> for SpanVec {
    type Output = Span;

    fn index(&self, idx: usize) -> &Self::Output {
        &self.spans[idx]
    }
}

#[test]
fn test_span_merge_right() {
    // Overlapping
    assert_eq!(
        Span::new(0, 1)
            .unwrap()
            .merge_right(Span::new(0, 2).unwrap()),
        MergeResult::Merged(Span::new(0, 2).unwrap())
    );
    // Adjacent
    assert_eq!(
        Span::new(0, 1)
            .unwrap()
            .merge_right(Span::new(2, 3).unwrap()),
        MergeResult::Merged(Span::new(0, 3).unwrap())
    );
    // Disjoint
    assert_eq!(
        Span::new(0, 1)
            .unwrap()
            .merge_right(Span::new(3, 4).unwrap()),
        MergeResult::UnMerged(Span::new(0, 1).unwrap(), Span::new(3, 4).unwrap())
    );
}

#[test]
fn test_span_merge() {
    // Overlapping
    assert_eq!(
        Span::new(0, 2).unwrap().merge(Span::new(0, 1).unwrap()),
        MergeResult::Merged(Span::new(0, 2).unwrap())
    );
    // Adjacent
    assert_eq!(
        Span::new(2, 3).unwrap().merge(Span::new(0, 1).unwrap()),
        MergeResult::Merged(Span::new(0, 3).unwrap())
    );
    // Disjoint
    assert_eq!(
        Span::new(0, 1).unwrap().merge(Span::new(3, 4).unwrap()),
        MergeResult::UnMerged(Span::new(0, 1).unwrap(), Span::new(3, 4).unwrap())
    );
}

#[test]
fn test_span_coalesce() {
    // Overlapping
    assert_eq!(
        Span::new(1, 2)
            .unwrap()
            .coalesce(Span::new(0, 1).unwrap(), Span::new(2, 3).unwrap()),
        SpanCoalesced::Overlap(Span::new(0, 3).unwrap())
    );
    // LeftOverlap
    assert_eq!(
        Span::new(1, 2)
            .unwrap()
            .coalesce(Span::new(0, 1).unwrap(), Span::new(4, 5).unwrap()),
        SpanCoalesced::LeftOverlap(Span::new(0, 2).unwrap(), Span::new(4, 5).unwrap())
    );
    // RightOverlap
    assert_eq!(
        Span::new(3, 4)
            .unwrap()
            .coalesce(Span::new(0, 1).unwrap(), Span::new(5, 9).unwrap()),
        SpanCoalesced::RightOverlap(Span::new(0, 1).unwrap(), Span::new(3, 9).unwrap())
    );
    // Disjoint
    assert_eq!(
        Span::new(3, 4)
            .unwrap()
            .coalesce(Span::new(0, 1).unwrap(), Span::new(6, 9).unwrap()),
        SpanCoalesced::Disjoint(
            Span::new(0, 1).unwrap(),
            Span::new(3, 4).unwrap(),
            Span::new(6, 9).unwrap()
        )
    );
}

#[test]
fn test_span_vec_insert_disjoint() {
    let mut sv = SpanVec::new();
    let s_far = Span::new(1000, 1100).unwrap();
    let s_close = Span::new(0, 1).unwrap();

    sv.insert(s_far);
    sv.insert(s_close);
    assert_eq!(sv[0], s_close);
    assert_eq!(sv[1], s_far);

    let mut sv = SpanVec::new();
    let s_far = Span::new(1000, 1100).unwrap();
    let s_close = Span::new(0, 1).unwrap();

    sv.insert(s_far);
    sv.insert(s_close);
    assert_eq!(sv[0], s_close);
    assert_eq!(sv[1], s_far);

    let mut sv = SpanVec::new();
    let s_far = Span::new(1000, 1100).unwrap();
    let s_mid = Span::new(500, 550).unwrap();
    let s_close = Span::new(0, 1).unwrap();

    sv.insert(s_mid);
    sv.insert(s_far);
    sv.insert(s_close);
    assert_eq!(sv[0], s_close);
    assert_eq!(sv[1], s_mid);
    assert_eq!(sv[2], s_far);
}

#[test]
fn test_span_vec_insert_coalescing() {
    let mut sv = SpanVec::new();
    let s_close = Span::new(0, 1).unwrap();
    let s_mid = Span::new(1, 2).unwrap();
    let s_far = Span::new(3, 4).unwrap();

    sv.insert(s_far);
    assert_eq!(sv.len(), 1);
    sv.insert(s_mid);
    assert_eq!(sv.len(), 1);
    sv.insert(s_close);
    assert_eq!(sv.len(), 1);
    assert_eq!(sv[0], Span::new(0, 4).unwrap());

    let mut sv = SpanVec::new();
    let s_far = Span::new(1000, 1100).unwrap();
    let s_close = Span::new(0, 1).unwrap();
    let s_big = Span::new(2, 999).unwrap();

    sv.insert(s_far);
    assert_eq!(sv.len(), 1);
    sv.insert(s_close);
    assert_eq!(sv[0], s_close);
    assert_eq!(sv[1], s_far);
    assert_eq!(sv.len(), 2);
    sv.insert(s_big);
    assert_eq!(sv[0], Span::new(0, 1100).unwrap());
    assert_eq!(sv.len(), 1);

    let mut sv = SpanVec::new();
    let s_far = Span::new(1000, 1100).unwrap();
    let s_mid = Span::new(500, 550).unwrap();
    let s_bridge_mid_far = Span::new(551, 999).unwrap();
    let s_close = Span::new(0, 1).unwrap();

    sv.insert(s_mid);
    sv.insert(s_far);
    sv.insert(s_close);
    assert_eq!(sv[0], s_close);
    assert_eq!(sv[1], s_mid);
    assert_eq!(sv[2], s_far);
    sv.insert(s_bridge_mid_far);
    assert_eq!(sv[0], s_close);
    assert_eq!(sv[1], Span::new(500, 1100).unwrap());
    assert_eq!(sv.len(), 2);

    let mut sv = SpanVec::new();
    sv.insert(Span::new(0, 1).unwrap());
    sv.insert(Span::new(3, 4).unwrap());
    sv.insert(Span::new(6, 8).unwrap());
    sv.insert(Span::new(10, 11).unwrap());
    sv.insert(Span::new(13, 14).unwrap());
    sv.insert(Span::new(16, 17).unwrap());
    sv.insert(Span::new(19, 20).unwrap());
    sv.insert(Span::new(23, 24).unwrap());
    sv.insert(Span::new(26, 27).unwrap());
    sv.insert(Span::new(29, 30).unwrap());
    sv.insert(Span::new(400, 405).unwrap());

    assert_eq!(sv.len(), 11);

    sv.insert(Span::new(7, 399).unwrap());

    assert_eq!(sv.len(), 3, "{:#?}", sv);

    let mut sv = SpanVec::new();
    sv.insert(Span::new(0, 1).unwrap());
    sv.insert(Span::new(0, 1).unwrap());
    sv.insert(Span::new(3, 4).unwrap());
    sv.insert(Span::new(16, 17).unwrap());
    sv.insert(Span::new(19, 20).unwrap());
    sv.insert(Span::new(6, 8).unwrap());
    sv.insert(Span::new(23, 24).unwrap());
    sv.insert(Span::new(10, 11).unwrap());
    sv.insert(Span::new(26, 27).unwrap());
    sv.insert(Span::new(13, 14).unwrap());
    sv.insert(Span::new(29, 30).unwrap());
    sv.insert(Span::new(36, 37).unwrap());
    sv.insert(Span::new(34, 35).unwrap());
    sv.insert(Span::new(31, 32).unwrap());
    sv.insert(Span::new(33, 34).unwrap());
    sv.insert(Span::new(400, 405).unwrap());

    assert_eq!(sv.len(), 11);

    sv.insert(Span::new(9, 15).unwrap());
    assert_eq!(sv.len(), 8, "{:#?}", sv);

    sv.insert(Span::new(18, 35).unwrap());
    assert_eq!(sv.len(), 4, "{:#?}", sv);
}
