//! Content hashes, the names of stored content.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use super::ProtocolError;

const PREFIX: &str = "sha256:";

/// The name of a piece of stored content: the SHA-256 digest of its bytes,
/// written `sha256:` and 64 lowercase hex digits. The prefix names the
/// algorithm, so that a later version can add another.
///
/// Stored content never changes, so its hash names it for good, and whoever
/// fetches it can check what arrived. Which bytes are hashed is up to the
/// surface that names them: a [`ResultRef`](super::jobs::ResultRef) covers
/// the bytes a download returns, and a segment's
/// [`format`](super::segments::SegmentEntry::format) fixes its bytes.
///
/// ```
/// use quantforge::protocol::ContentHash;
///
/// // The SHA-256 of no bytes at all.
/// let text = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
/// let hash: ContentHash = text.parse()?;
/// assert_eq!(hash.digest()[0], 0xe3);
/// assert_eq!(hash.to_string(), text);
/// assert_eq!(serde_json::to_string(&hash)?, format!("\"{text}\""));
///
/// assert!(text.to_uppercase().parse::<ContentHash>().is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// The name of content whose SHA-256 digest is `digest`.
    pub const fn sha256(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// The SHA-256 digest.
    pub const fn digest(&self) -> &[u8; 32] {
        &self.0
    }
}

/// The wire text: `sha256:` and the digest in lowercase hex.
impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(PREFIX)?;
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({self})")
    }
}

/// Reads the wire text. Uppercase hex is refused, so that each hash has
/// exactly one spelling to compare and look up by.
impl FromStr for ContentHash {
    type Err = ProtocolError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || ProtocolError::InvalidContentHash {
            input: text.to_string(),
        };
        let hex = text.strip_prefix(PREFIX).ok_or_else(invalid)?.as_bytes();
        if hex.len() != 64 {
            return Err(invalid());
        }
        let mut digest = [0; 32];
        for (byte, pair) in digest.iter_mut().zip(hex.chunks_exact(2)) {
            let &[high, low] = pair else {
                return Err(invalid());
            };
            *byte = (nibble(high).ok_or_else(invalid)? << 4) | nibble(low).ok_or_else(invalid)?;
        }
        Ok(Self(digest))
    }
}

/// The value of one lowercase hex digit.
fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(HashText)
    }
}

struct HashText;

impl de::Visitor<'_> for HashText {
    type Value = ContentHash;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a content hash as a JSON string, sha256: and 64 lowercase hex digits")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Self::Value, E> {
        text.parse().map_err(E::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn a_hash_reads_and_writes_the_same_text() {
        let text = format!("sha256:{EMPTY}");
        let hash: ContentHash = text.parse().expect("hash");
        assert_eq!(hash.digest()[..2], [0xe3, 0xb0]);
        assert_eq!(hash.digest()[31], 0x55);
        assert_eq!(hash.to_string(), text);
        let json = format!("\"{text}\"");
        let read: ContentHash = serde_json::from_str(&json).expect("hash string");
        assert_eq!(read, hash);
        assert_eq!(serde_json::to_string(&read).expect("serialize"), json);
    }

    #[test]
    fn a_digest_is_written_as_lowercase_hex() {
        let mut digest = [0xab; 32];
        digest[0] = 0x00;
        digest[31] = 0xff;
        let hash = ContentHash::sha256(digest);
        let text = hash.to_string();
        assert_eq!(text, format!("sha256:00{}ff", "ab".repeat(30)));
        assert_eq!(text.parse::<ContentHash>().expect("hash"), hash);
        assert_eq!(format!("{hash:?}"), format!("ContentHash({text})"));
    }

    #[test]
    fn anything_but_sha256_and_64_lowercase_hex_digits_is_refused() {
        for text in [
            String::new(),
            "sha256:".to_string(),
            EMPTY.to_string(),
            format!("sha256:{}", &EMPTY[1..]),
            format!("sha256:{EMPTY}0"),
            format!("sha256:{}", EMPTY.to_uppercase()),
            format!("SHA256:{EMPTY}"),
            format!("sha-256:{EMPTY}"),
            format!("blake3:{EMPTY}"),
            format!(" sha256:{EMPTY}"),
            format!("sha256:{EMPTY}\n"),
            format!("sha256: {}", &EMPTY[1..]),
            format!("sha256:{}g", &EMPTY[1..]),
            format!("sha256:{}é", &EMPTY[2..]),
        ] {
            assert_eq!(
                text.parse::<ContentHash>(),
                Err(ProtocolError::InvalidContentHash {
                    input: text.clone()
                }),
                "{text:?}"
            );
        }
    }

    #[test]
    fn a_hash_is_a_json_string_and_nothing_else() {
        for json in ["0", "null", "[]", r#"{"sha256":"00"}"#] {
            assert!(
                serde_json::from_str::<ContentHash>(json).is_err(),
                "{json} must be refused"
            );
        }
    }
}
