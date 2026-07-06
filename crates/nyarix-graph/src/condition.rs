//! Routing conditions for conditional edges (see issue #28).
//!
//! Built on top of what already exists on [`Packet`] — [`Tags`] for
//! classification (#10) and [`Metadata::priority`] for QoS (#9) — rather
//! than a new predicate mini-language, since those already cover the
//! "which class of traffic is this" question the vision doc's routing
//! examples (§6) call for.

use nyarix_packet::{Packet, Tag, Tags};

/// A predicate evaluated against a packet to decide whether a conditional
/// edge should be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// True if the packet has this single tag set.
    HasTag(Tag),
    /// True if the packet's tags intersect this mask at all.
    TagsIntersect(Tags),
    /// True if the packet's priority is at least this value.
    PriorityAtLeast(u8),
    /// True if the inner condition is false.
    Not(Box<Condition>),
    /// True if every inner condition is true.
    All(Vec<Condition>),
    /// True if at least one inner condition is true.
    Any(Vec<Condition>),
}

impl Condition {
    /// Evaluate this condition against a packet.
    #[must_use]
    pub fn evaluate(&self, packet: &Packet) -> bool {
        match self {
            Self::HasTag(tag) => packet.has_tag(*tag),
            Self::TagsIntersect(mask) => packet.tags().intersects(*mask),
            Self::PriorityAtLeast(min) => packet.metadata().priority >= *min,
            Self::Not(inner) => !inner.evaluate(packet),
            Self::All(conditions) => conditions.iter().all(|c| c.evaluate(packet)),
            Self::Any(conditions) => conditions.iter().any(|c| c.evaluate(packet)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_tag_matches() {
        let mut pkt = Packet::new(b"data".as_slice());
        pkt.tag(Tag::Interactive);

        assert!(Condition::HasTag(Tag::Interactive).evaluate(&pkt));
        assert!(!Condition::HasTag(Tag::Bulk).evaluate(&pkt));
    }

    #[test]
    fn priority_at_least() {
        let mut pkt = Packet::new(b"data".as_slice());
        pkt.metadata_mut().priority = 200;

        assert!(Condition::PriorityAtLeast(128).evaluate(&pkt));
        assert!(!Condition::PriorityAtLeast(255).evaluate(&pkt));
    }

    #[test]
    fn not_negates() {
        let pkt = Packet::new(b"data".as_slice());
        assert!(Condition::Not(Box::new(Condition::HasTag(Tag::Interactive))).evaluate(&pkt));
    }

    #[test]
    fn all_requires_every_condition() {
        let mut pkt = Packet::new(b"data".as_slice());
        pkt.tag(Tag::Interactive);
        pkt.metadata_mut().priority = 200;

        let condition = Condition::All(vec![
            Condition::HasTag(Tag::Interactive),
            Condition::PriorityAtLeast(128),
        ]);
        assert!(condition.evaluate(&pkt));

        let unmet = Condition::All(vec![
            Condition::HasTag(Tag::Interactive),
            Condition::HasTag(Tag::Bulk),
        ]);
        assert!(!unmet.evaluate(&pkt));
    }

    #[test]
    fn any_requires_one_condition() {
        let mut pkt = Packet::new(b"data".as_slice());
        pkt.tag(Tag::Bulk);

        let condition = Condition::Any(vec![
            Condition::HasTag(Tag::Interactive),
            Condition::HasTag(Tag::Bulk),
        ]);
        assert!(condition.evaluate(&pkt));
    }
}
