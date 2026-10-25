//! Which lost packets to ask for again, and when (SR-04).
//!
//! Socket- and clock-free like the assembler that owns it: each block in
//! flight keeps a [`ResendState`], told of every packet that arrives and asked
//! on every tick which ranges are due. The receive thread turns the answer into
//! `PACKETRESEND_CMD`s.
//!
//! - **Gaps.** Packets arrive in id order on a quiet link, so a packet beyond
//!   the next one expected opens a gap: every id in between is missing. A gap
//!   is not asked for until it is [`ResendConfig::delay`] old, so a packet
//!   that was only overtaken fills it first.
//! - **Ranges.** A gap is one contiguous run of missing ids and goes out as one
//!   request. A packet that arrives inside a gap splits it.
//! - **Retries.** A gap still open is asked for again every
//!   [`ResendConfig::retry_interval`], at most [`ResendConfig::max_requests`]
//!   times in all.
//! - **Budget.** A block's requests — retries included — stop once they would
//!   ask for more than [`ResendConfig::budget`] of its packets.
//! - **Answers.** A camera that cannot resend says so with a bare GVSP header
//!   whose status names why; the final ones stop retries for the named packet
//!   (or that packet and its predecessors), preserving recoverable neighbours
//!   ([`ResendState::unavailable`]).
//!
//! A lost trailer is never asked for: nothing after it says it is missing.

use std::time::{Duration, Instant};

/// Wait this long after a gap opens before asking for it.
pub(crate) const DEFAULT_RESEND_DELAY: Duration = Duration::from_millis(2);
/// Ask again this long after a request that brought nothing.
pub(crate) const DEFAULT_RESEND_RETRY_INTERVAL: Duration = Duration::from_millis(20);
/// Requests per gap, the first included.
pub(crate) const DEFAULT_RESEND_MAX_REQUESTS: u32 = 3;
/// Share of a block's packets its requests may ask for, retries included.
pub(crate) const DEFAULT_RESEND_BUDGET: f64 = 0.25;

/// How a stream asks for lost packets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ResendConfig {
    /// Whether to ask at all.
    pub enabled: bool,
    /// Age a gap must reach before it is asked for.
    pub delay: Duration,
    /// Time between requests for a gap still open.
    pub retry_interval: Duration,
    /// Requests per gap, the first included.
    pub max_requests: u32,
    /// Share of a block's packets that may be asked for, retries included.
    pub budget: f64,
}

impl Default for ResendConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            delay: DEFAULT_RESEND_DELAY,
            retry_interval: DEFAULT_RESEND_RETRY_INTERVAL,
            max_requests: DEFAULT_RESEND_MAX_REQUESTS,
            budget: DEFAULT_RESEND_BUDGET,
        }
    }
}

/// One range to ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResendRequest {
    pub block_id: u64,
    pub first: u32,
    pub last: u32,
    pub extended_ids: bool,
}

/// A run of missing packet ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Gap {
    first: u32,
    last: u32,
    /// Requests sent for it so far.
    requests: u32,
    /// When to ask next; `None` once it has been asked for as often as allowed.
    due: Option<Instant>,
}

impl Gap {
    fn len(&self) -> u64 {
        u64::from(self.last - self.first) + 1
    }

    fn contains(&self, id: u32) -> bool {
        self.first <= id && id <= self.last
    }
}

/// Requests for one block, as its packets arrive.
#[derive(Debug, Default)]
pub(crate) struct ResendState {
    /// The next packet id not yet seen: every id below it arrived or is in a
    /// gap.
    frontier: u32,
    gaps: Vec<Gap>,
    /// Packets asked for, retries included, against the budget.
    asked: u64,
    /// The budget is spent: no further requests for this block.
    spent: bool,
}

/// What [`ResendState::due`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Due {
    /// Packets asked for the first time.
    pub newly_requested: u64,
    /// Whether this call spent the budget.
    pub budget_spent: bool,
}

/// How a camera answered a resend request it could not serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// The packet is gone (`PACKET_UNAVAILABLE`, `PACKET_REMOVED_FROM_MEMORY`).
    Gone,
    /// The packet and every one before it are gone
    /// (`PACKET_AND_PREVIOUS_REMOVED_FROM_MEMORY`).
    GoneWithPrevious,
}

impl ResendState {
    /// Packet `id` arrived at `now`. Returns whether it had been asked for.
    pub(super) fn arrived(&mut self, id: u32, now: Instant, delay: Duration) -> bool {
        if id >= self.frontier {
            if id > self.frontier {
                self.gaps.push(Gap {
                    first: self.frontier,
                    last: id - 1,
                    requests: 0,
                    due: Some(now + delay),
                });
            }
            self.frontier = id.saturating_add(1);
            return false;
        }
        self.remove_packet(id).is_some_and(|gap| gap.requests > 0)
    }

    /// Remove just this missing id, preserving retry history on either side.
    fn remove_packet(&mut self, id: u32) -> Option<Gap> {
        let at = self.gaps.iter().position(|g| g.contains(id))?;
        let gap = self.gaps[at];
        match (id == gap.first, id == gap.last) {
            (true, true) => {
                self.gaps.remove(at);
            }
            (true, false) => self.gaps[at].first = id + 1,
            (false, true) => self.gaps[at].last = id - 1,
            (false, false) => {
                self.gaps[at].last = id - 1;
                self.gaps.insert(
                    at + 1,
                    Gap {
                        first: id + 1,
                        ..gap
                    },
                );
            }
        }
        Some(gap)
    }

    /// The trailer, packet `id`, arrived: nothing beyond it belongs to the
    /// block.
    pub(super) fn trailer(&mut self, id: u32) {
        self.gaps.retain(|g| g.first < id);
        if let Some(last) = self.gaps.last_mut() {
            last.last = last.last.min(id.saturating_sub(1));
        }
        self.frontier = self.frontier.min(id.saturating_add(1));
    }

    /// The camera answered `answer` for packet `id`. Returns how many packets
    /// asked for it has thereby declared unavailable.
    ///
    /// "And every one before it" closes the earlier gaps too, counting those
    /// asked for: a late arrival there is then neither asked for again nor
    /// counted as recovered, so each packet asked for ends in one outcome
    /// (Codex review of #195). An earlier gap not yet asked for closes
    /// uncounted.
    pub(super) fn unavailable(&mut self, id: u32, answer: Answer) -> u64 {
        if answer == Answer::Gone {
            if !self
                .gaps
                .iter()
                .any(|gap| gap.contains(id) && gap.requests > 0)
            {
                return 0;
            }
            return u64::from(self.remove_packet(id).is_some());
        }
        let mut gone = 0;
        self.gaps.retain_mut(|gap| {
            if gap.first > id {
                return true;
            }
            let last_gone = gap.last.min(id);
            if gap.requests > 0 {
                gone += u64::from(last_gone - gap.first) + 1;
            }
            if gap.last <= id {
                return false;
            }
            // Here id < gap.last, so incrementing cannot overflow.
            gap.first = id + 1;
            true
        });
        gone
    }

    /// Append the requests due at `now` for block `block_id`, whose packets —
    /// leader and trailer included — number `packets`.
    pub(super) fn due(
        &mut self,
        now: Instant,
        config: &ResendConfig,
        packets: u64,
        block: (u64, bool),
        out: &mut Vec<ResendRequest>,
    ) -> Due {
        let mut done = Due::default();
        if self.spent {
            return done;
        }
        let budget = (packets as f64 * config.budget).ceil() as u64;
        for gap in &mut self.gaps {
            if !gap.due.is_some_and(|due| due <= now) {
                continue;
            }
            let len = gap.len();
            if self.asked + len > budget {
                self.spent = true;
                done.budget_spent = true;
                break;
            }
            self.asked += len;
            if gap.requests == 0 {
                done.newly_requested += len;
            }
            gap.requests += 1;
            gap.due = (gap.requests < config.max_requests).then(|| now + config.retry_interval);
            out.push(ResendRequest {
                block_id: block.0,
                first: gap.first,
                last: gap.last,
                extended_ids: block.1,
            });
        }
        done
    }

    /// The next packet id not yet seen.
    pub(super) fn frontier(&self) -> u32 {
        self.frontier
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn config() -> ResendConfig {
        ResendConfig {
            enabled: true,
            ..ResendConfig::default()
        }
    }

    fn due(state: &mut ResendState, at: Instant, packets: u64) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        state.due(at, &config(), packets, (7, false), &mut out);
        out.iter().map(|r| (r.first, r.last)).collect()
    }

    /// Packets 3 and 4 are overtaken by 5: one gap, asked for as one range
    /// once it is older than the delay.
    #[test]
    fn a_gap_is_asked_for_as_one_range_after_the_delay() {
        let mut state = ResendState::default();
        let t0 = Instant::now();
        for id in [0, 1, 2, 5, 6] {
            assert!(!state.arrived(id, t0, DEFAULT_RESEND_DELAY));
        }
        assert!(due(&mut state, t0 + MS, 100).is_empty(), "too young");
        assert_eq!(due(&mut state, t0 + 2 * MS, 100), vec![(3, 4)]);
    }

    /// A packet that was only overtaken fills its gap before it is due.
    #[test]
    fn a_reordered_packet_is_not_asked_for() {
        let mut state = ResendState::default();
        let t0 = Instant::now();
        for id in [0, 1, 3, 2, 4] {
            state.arrived(id, t0, DEFAULT_RESEND_DELAY);
        }
        assert!(due(&mut state, t0 + 10 * MS, 100).is_empty());
    }

    /// A packet asked for counts as recovered when it arrives; one arriving
    /// in the middle of a gap splits it.
    #[test]
    fn an_arrival_inside_a_requested_gap_splits_it() {
        let mut state = ResendState::default();
        let t0 = Instant::now();
        state.arrived(0, t0, DEFAULT_RESEND_DELAY);
        state.arrived(10, t0, DEFAULT_RESEND_DELAY);
        assert_eq!(due(&mut state, t0 + 2 * MS, 100), vec![(1, 9)]);
        assert!(state.arrived(5, t0 + 3 * MS, DEFAULT_RESEND_DELAY));
        assert!(
            !state.arrived(5, t0 + 3 * MS, DEFAULT_RESEND_DELAY),
            "a repeat"
        );
        assert_eq!(
            due(&mut state, t0 + 22 * MS, 100),
            vec![(1, 4), (6, 9)],
            "the retry asks for both halves"
        );
    }

    /// A camera answering per packet has said nothing about this packet's
    /// neighbours. They remain eligible for recovery, with their retry count
    /// and due time intact.
    #[test]
    fn a_packet_refusal_preserves_recoverable_neighbours() {
        let mut state = ResendState::default();
        let t0 = Instant::now();
        state.arrived(0, t0, DEFAULT_RESEND_DELAY);
        state.arrived(10, t0, DEFAULT_RESEND_DELAY);
        assert_eq!(due(&mut state, t0 + 2 * MS, 100), vec![(1, 9)]);
        assert_eq!(state.unavailable(5, Answer::Gone), 1);
        assert_eq!(state.unavailable(5, Answer::Gone), 0, "duplicate refusal");
        assert!(state.arrived(6, t0 + 3 * MS, DEFAULT_RESEND_DELAY));
        assert!(due(&mut state, t0 + 21 * MS, 100).is_empty());
        assert_eq!(due(&mut state, t0 + 22 * MS, 100), vec![(1, 4), (7, 9)]);
    }

    #[test]
    fn an_unsolicited_refusal_does_not_suppress_the_first_request() {
        let mut state = ResendState::default();
        let t0 = Instant::now();
        state.arrived(0, t0, DEFAULT_RESEND_DELAY);
        state.arrived(5, t0, DEFAULT_RESEND_DELAY);
        assert_eq!(state.unavailable(2, Answer::Gone), 0);
        assert_eq!(due(&mut state, t0 + 2 * MS, 100), vec![(1, 4)]);
    }

    #[test]
    fn gone_with_previous_keeps_the_tail_of_a_gap() {
        let mut state = ResendState::default();
        let t0 = Instant::now();
        state.arrived(0, t0, DEFAULT_RESEND_DELAY);
        state.arrived(10, t0, DEFAULT_RESEND_DELAY);
        assert_eq!(due(&mut state, t0 + 2 * MS, 100), vec![(1, 9)]);
        assert_eq!(state.unavailable(5, Answer::GoneWithPrevious), 5);
        assert_eq!(state.unavailable(5, Answer::GoneWithPrevious), 0);
        assert!(!state.arrived(4, t0 + 3 * MS, DEFAULT_RESEND_DELAY));
        assert!(state.arrived(6, t0 + 3 * MS, DEFAULT_RESEND_DELAY));
        assert_eq!(due(&mut state, t0 + 22 * MS, 100), vec![(7, 9)]);
    }

    /// Each gap is asked for at most three times, 20 ms apart.
    #[test]
    fn a_gap_is_asked_for_three_times_at_most() {
        let mut state = ResendState::default();
        let t0 = Instant::now();
        state.arrived(0, t0, DEFAULT_RESEND_DELAY);
        state.arrived(2, t0, DEFAULT_RESEND_DELAY);
        let mut asked = 0;
        let mut t = t0;
        while t < t0 + 200 * MS {
            asked += due(&mut state, t, 100).len();
            t += MS;
        }
        assert_eq!(asked, 3);
    }

    /// Requests stop once they would ask for more than a quarter of the
    /// block's packets, retries included.
    #[test]
    fn requests_stop_at_the_budget() {
        let mut state = ResendState::default();
        let t0 = Instant::now();
        for id in (0..40).step_by(2) {
            state.arrived(id, t0, DEFAULT_RESEND_DELAY);
        }
        // 19 single-packet gaps in a 40-packet block: 10 may be asked for.
        let mut out = Vec::new();
        let done = state.due(t0 + 2 * MS, &config(), 40, (7, false), &mut out);
        assert_eq!(out.len(), 10);
        assert_eq!(
            done,
            Due {
                newly_requested: 10,
                budget_spent: true
            }
        );
        assert!(due(&mut state, t0 + 100 * MS, 40).is_empty());
    }

    /// A final "no longer available" closes that packet and counts it;
    /// "and every one before it" also stops the earlier gaps.
    #[test]
    fn an_unavailable_answer_preserves_the_rest_of_the_gap() {
        let mut state = ResendState::default();
        let t0 = Instant::now();
        for id in [0, 2, 5, 9] {
            state.arrived(id, t0, DEFAULT_RESEND_DELAY);
        }
        assert_eq!(
            due(&mut state, t0 + 2 * MS, 100),
            vec![(1, 1), (3, 4), (6, 8)]
        );
        assert_eq!(state.unavailable(7, Answer::Gone), 1);
        assert_eq!(state.unavailable(7, Answer::Gone), 0, "already closed");
        assert_eq!(
            state.unavailable(3, Answer::GoneWithPrevious),
            1 + 1,
            "packet 3, and gap 1 before it; packet 4 is still recoverable"
        );
        assert_eq!(
            due(&mut state, t0 + 50 * MS, 100),
            vec![(4, 4), (6, 6), (8, 8)],
            "only the named ids and predecessors are unavailable"
        );
    }

    /// "This packet and every one before it are gone" closes the earlier
    /// gaps asked for and counts their packets as unavailable, so a late
    /// arrival in one of them is not counted as recovered as well (Codex
    /// review of #195): each packet asked for ends in one outcome.
    #[test]
    fn gone_with_previous_closes_and_counts_the_earlier_gaps() {
        let mut state = ResendState::default();
        let t0 = Instant::now();
        for id in [0, 2, 5, 9] {
            state.arrived(id, t0, DEFAULT_RESEND_DELAY);
        }
        assert_eq!(
            due(&mut state, t0 + 2 * MS, 100),
            vec![(1, 1), (3, 4), (6, 8)]
        );
        assert_eq!(
            state.unavailable(7, Answer::GoneWithPrevious),
            2 + 2 + 1,
            "packets 6-7 and both gaps before them; 8 is still recoverable"
        );
        assert!(
            !state.arrived(1, t0 + 3 * MS, DEFAULT_RESEND_DELAY),
            "packet 1 was counted unavailable; it cannot be recovered too"
        );
        assert_eq!(due(&mut state, t0 + 50 * MS, 100), vec![(8, 8)]);
    }

    /// Ids beyond the trailer are not the block's.
    #[test]
    fn the_trailer_bounds_the_gaps() {
        let mut state = ResendState::default();
        let t0 = Instant::now();
        state.arrived(0, t0, DEFAULT_RESEND_DELAY);
        state.arrived(9, t0, DEFAULT_RESEND_DELAY);
        state.arrived(5, t0, DEFAULT_RESEND_DELAY);
        state.trailer(5);
        assert_eq!(state.frontier(), 6);
        assert_eq!(due(&mut state, t0 + 2 * MS, 100), vec![(1, 4)]);
    }
}
