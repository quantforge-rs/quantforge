//! The format's fixed words: what a signal asks for, the side of a
//! position, and a product family.

use std::fmt;

/// Defines an enum of the format's fixed words, with the word each one is
/// written as.
macro_rules! words {
    ($(#[$doc:meta])* $name:ident { $($(#[$word_doc:meta])* $variant:ident => $word:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$word_doc])* $variant,)+
        }

        impl $name {
            /// Every value, in the order the format lists them.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The word as a payload writes it.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $word,)+
                }
            }

            /// Reads the word; anything else, in any other case, is none
            /// of them.
            pub(super) fn named(word: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|value| value.as_str() == word)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

words!(
    /// What a signal asks for. It is a constant of the alert's template,
    /// never worked out from an order's `buy` or `sell`.
    SignalKind {
        /// Open a position.
        Entry => "entry",
        /// Close one: an exit or a take-profit alert.
        Exit => "exit",
    }
);

words!(
    /// The side of a position: the one an entry opens, or the one an exit
    /// closes. Like the kind, it is a constant of the alert's template.
    SignalSide {
        /// A long position.
        Long => "long",
        /// A short position.
        Short => "short",
    }
);

words!(
    /// The product family a signal may say it is meant for. A signal
    /// trades futures, so there is no `spot`.
    SignalCategory {
        /// Contracts settled in the quote asset.
        Linear => "linear",
        /// Contracts settled in the base coin.
        Inverse => "inverse",
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_written_in_lowercase_only() {
        assert_eq!(SignalKind::named("entry"), Some(SignalKind::Entry));
        assert_eq!(SignalSide::named("short"), Some(SignalSide::Short));
        assert_eq!(
            SignalCategory::named("inverse"),
            Some(SignalCategory::Inverse)
        );
        for word in ["", "Entry", "ENTRY", "buy", "entry "] {
            assert_eq!(SignalKind::named(word), None, "{word:?}");
        }
        assert_eq!(SignalSide::named("flat"), None);
        assert_eq!(SignalCategory::named("spot"), None);
        let kinds: Vec<&str> = SignalKind::ALL.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(kinds, ["entry", "exit"]);
        assert_eq!(SignalSide::Long.to_string(), "long");
    }
}
