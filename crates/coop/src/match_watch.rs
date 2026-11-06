//! Telling that a session's peer left the match we're both in, without waiting for its connection
//! to time out. A player closing the game says no goodbye (`Message::Leave`), and Steam takes about
//! 15 seconds to give up on their connection; the match itself knows at once, as it drops them from
//! its players.

use crate::message::PeerId;

/// The session's peers seen in the match.
#[derive(Default)]
pub struct MatchWatch {
    seen: Vec<PeerId>,
}

impl MatchWatch {
    /// The peers of `expected` that were in the match at the last call and aren't in `players` now.
    /// `players` is `None` when the match can't be read or is another one: nothing can be told then,
    /// and everyone has to be seen again. A peer that never was in the match (a friend playing
    /// elsewhere) is never told.
    pub fn gone(&mut self, expected: &[PeerId], players: Option<&[PeerId]>) -> Vec<PeerId> {
        let Some(players) = players else {
            self.seen.clear();
            return Vec::new();
        };
        let gone = self
            .seen
            .iter()
            .copied()
            .filter(|peer| expected.contains(peer) && !players.contains(peer))
            .collect();
        self.seen = expected
            .iter()
            .copied()
            .filter(|peer| players.contains(peer))
            .collect();
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_that_was_seen_is_told_once_when_it_goes() {
        let mut watch = MatchWatch::default();
        assert!(watch.gone(&[2, 3], Some(&[1, 2, 3])).is_empty());
        assert_eq!(watch.gone(&[2, 3], Some(&[1, 3])), vec![2]);
        assert!(watch.gone(&[2, 3], Some(&[1, 3])).is_empty());
    }

    #[test]
    fn a_peer_never_in_the_match_is_not_told() {
        let mut watch = MatchWatch::default();
        assert!(watch.gone(&[2], Some(&[1])).is_empty());
        assert!(watch.gone(&[2], Some(&[1])).is_empty());
    }

    #[test]
    fn a_peer_arriving_late_counts_from_then_on() {
        let mut watch = MatchWatch::default();
        assert!(watch.gone(&[2], Some(&[1])).is_empty());
        assert!(watch.gone(&[2], Some(&[1, 2])).is_empty());
        assert_eq!(watch.gone(&[2], Some(&[1])), vec![2]);
    }

    #[test]
    fn a_peer_no_longer_expected_is_not_told() {
        let mut watch = MatchWatch::default();
        assert!(watch.gone(&[2], Some(&[1, 2])).is_empty());
        // It said goodbye or was kicked first.
        assert!(watch.gone(&[], Some(&[1])).is_empty());
    }

    #[test]
    fn another_match_or_none_starts_over() {
        let mut watch = MatchWatch::default();
        assert!(watch.gone(&[2], Some(&[1, 2])).is_empty());
        assert!(watch.gone(&[2], None).is_empty());
        // Not there, but it wasn't seen since the match could be read again.
        assert!(watch.gone(&[2], Some(&[1])).is_empty());
    }
}
