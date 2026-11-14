//! A signal against the positions its profile already has: which position
//! an exit closes, and what an entry is on a position already held.

use super::super::{EntryIntent, ExitIntent};
use super::SignalRefusal;
use crate::model::{PositionSide, Refusal};

impl ExitIntent {
    /// Whether this exit closes the position `entry` opened: the same
    /// profile, the same account and instrument, the side the exit names,
    /// and an entry that fired strictly before the exit did.
    ///
    /// The last rule is what keeps a delayed exit from closing a newer
    /// position: an exit that fired before the position's entry, or in the
    /// same second, was meant for an earlier one, however late it arrives.
    /// An exit that closes nothing is a no-op, recorded as one.
    pub fn closes(&self, entry: &EntryIntent) -> bool {
        self.profile == entry.profile
            && self.key == entry.key
            && self.side == entry.side
            && entry.signal_time_ms < self.signal_time_ms
    }
}

impl EntryIntent {
    /// Holds the entry against the side its profile already holds on the
    /// entry's instrument, if it holds one. An entry on the side held would
    /// add to the position: `pyramiding`. One on the other side would
    /// reverse it: `reversal`, since the indicator sends an exit first.
    /// Neither is done later; the entry is refused for good.
    pub fn against(&self, held: Option<PositionSide>) -> Result<(), SignalRefusal> {
        let Some(held) = held else {
            return Ok(());
        };
        let symbol = &self.key.instrument().symbol;
        let profile = &self.profile;
        if held == self.side {
            return Err(SignalRefusal::new(
                Refusal::Pyramiding,
                format!(
                    "profile {profile} already holds a {held} position on {symbol}; a second \
                     entry on its side would add to it"
                ),
            ));
        }
        Err(SignalRefusal::new(
            Refusal::Reversal,
            format!(
                "profile {profile} holds a {held} position on {symbol}; a {} entry would reverse \
                 it, so send the exit first",
                self.side
            ),
        ))
    }
}
