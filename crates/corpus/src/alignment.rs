//! Deterministic token alignment shared by code comparisons and family matching.
use std::collections::HashMap;

/// Align tokens without a popularity/junk heuristic. Equal-length ties select
/// the earliest left position, then the earliest right position. Unequal gaps
/// pair their common prefix and retain every remaining token on either side.
pub fn align_sequences(left: &[String], right: &[String]) -> Vec<(Option<usize>, Option<usize>)> {
    let mut tokens = HashMap::new();
    let mut intern = |sequence: &[String]| {
        sequence
            .iter()
            .map(|token| {
                let next = tokens.len() as u32;
                *tokens.entry(token.clone()).or_insert(next)
            })
            .collect::<Vec<_>>()
    };
    let a = intern(left);
    let b = intern(right);
    let mut result = Vec::with_capacity(left.len().max(right.len()));
    let (mut i, mut j) = (0, 0);
    let mut blocks = crate::matching::blocks(&a, &b);
    blocks.push((left.len(), right.len(), 0));
    for (ai, bj, length) in blocks {
        for offset in 0..(ai - i).max(bj - j) {
            result.push((
                (i + offset < ai).then_some(i + offset),
                (j + offset < bj).then_some(j + offset),
            ));
        }
        for offset in 0..length {
            result.push((Some(ai + offset), Some(bj + offset)));
        }
        i = ai + length;
        j = bj + length;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn align(a: &[&str], b: &[&str]) -> Vec<(Option<usize>, Option<usize>)> {
        align_sequences(
            &a.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &b.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
    }
    #[test]
    fn empty_and_unequal_gaps_preserve_every_token() {
        assert_eq!(align(&[], &[]), vec![]);
        assert_eq!(
            align(&["a", "b"], &[]),
            vec![(Some(0), None), (Some(1), None)]
        );
        assert_eq!(
            align(&["x", "a", "y", "z"], &["u", "v", "a"]),
            vec![
                (Some(0), Some(0)),
                (None, Some(1)),
                (Some(1), Some(2)),
                (Some(2), None),
                (Some(3), None)
            ]
        );
    }
    #[test]
    fn ties_prefer_earliest_left_then_right() {
        assert_eq!(
            align(&["a", "b"], &["b", "a"]),
            vec![(None, Some(0)), (Some(0), Some(1)), (Some(1), None)]
        );
        assert_eq!(
            align(&["a", "a"], &["a"]),
            vec![(Some(0), Some(0)), (Some(1), None)]
        );
    }
}
