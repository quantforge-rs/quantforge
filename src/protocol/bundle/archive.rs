//! The canonical tar: the single byte sequence a bundle's files make. The
//! code hash is taken over it, and it is what travels to the server.

use sha2::{Digest, Sha256};

use super::super::ContentHash;
use super::path::BundleFile;

/// A tar block.
pub(super) const BLOCK: usize = 512;

/// The two zero blocks that end an archive.
pub(super) const END: usize = 2 * BLOCK;

/// The fields of a ustar header, as `(name, start, end)` byte ranges.
pub(super) const FIELDS: [(&str, usize, usize); 17] = [
    ("name", 0, 100),
    ("mode", 100, 108),
    ("uid", 108, 116),
    ("gid", 116, 124),
    ("size", 124, 136),
    ("mtime", 136, 148),
    ("checksum", 148, 156),
    ("typeflag", 156, 157),
    ("linkname", 157, 257),
    ("magic", 257, 263),
    ("version", 263, 265),
    ("uname", 265, 297),
    ("gname", 297, 329),
    ("devmajor", 329, 337),
    ("devminor", 337, 345),
    ("prefix", 345, 500),
    ("pad", 500, 512),
];

/// The canonical header of a file named `name` holding `size` bytes. `name`
/// is at most 100 bytes and `size` below 8^11, as the path rules and the
/// size limit ensure; a longer name or larger size would be cut short.
///
/// Every field but the name and size is fixed: mode `0000644`, owner `0`,
/// mtime `0`, a regular file, ustar magic and version `00`, and no names,
/// device numbers, or prefix.
pub(super) fn header(name: &[u8], size: u64) -> [u8; BLOCK] {
    let mut header = [0; BLOCK];
    header
        .iter_mut()
        .take(100)
        .zip(name)
        .for_each(|(slot, &byte)| *slot = byte);
    header[100..108].copy_from_slice(b"0000644\0");
    header[108..116].copy_from_slice(b"0000000\0");
    header[116..124].copy_from_slice(b"0000000\0");
    octal(&mut header[124..135], size);
    header[136..148].copy_from_slice(b"00000000000\0");
    // The checksum counts its own field as eight spaces.
    header[148..156].copy_from_slice(b"        ");
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum = header.iter().map(|&byte| u64::from(byte)).sum();
    octal(&mut header[148..154], checksum);
    header[154] = 0;
    header
}

/// Fills `digits` with `value` in octal, zero-padded on the left.
fn octal(digits: &mut [u8], mut value: u64) {
    for digit in digits.iter_mut().rev() {
        *digit = b'0' + (value & 7) as u8;
        value >>= 3;
    }
}

/// How many NUL bytes follow `len` bytes of data to fill its last block.
pub(super) fn padding(len: usize) -> usize {
    (BLOCK - len % BLOCK) % BLOCK
}

/// The canonical tar of `files`, which are sorted by path.
pub(super) fn write(files: &[BundleFile]) -> Vec<u8> {
    let length = files.iter().fold(END, |length, file| {
        let data = file.contents().len();
        length
            .saturating_add(BLOCK)
            .saturating_add(data)
            .saturating_add(padding(data))
    });
    let mut tar = Vec::with_capacity(length);
    for file in files {
        let data = file.contents();
        let size = u64::try_from(data.len()).unwrap_or(u64::MAX);
        tar.extend_from_slice(&header(file.path().as_str().as_bytes(), size));
        tar.extend_from_slice(data);
        tar.resize(tar.len() + padding(data.len()), 0);
    }
    tar.resize(tar.len() + END, 0);
    tar
}

/// The SHA-256 of a canonical tar, as a content hash.
pub(super) fn code_hash(tar: &[u8]) -> ContentHash {
    ContentHash::sha256(Sha256::digest(tar).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Built field by field from the spec's table, independently of `header`.
    #[test]
    fn a_header_holds_the_canonical_bytes_of_every_field() {
        let header = header(b"pkg/strategy.py", 5);
        let field = |name: &str| {
            let &(_, start, end) = FIELDS
                .iter()
                .find(|(field, ..)| *field == name)
                .expect("a field");
            &header[start..end]
        };
        let mut name = b"pkg/strategy.py".to_vec();
        name.resize(100, 0);
        assert_eq!(field("name"), name.as_slice());
        assert_eq!(field("mode"), b"0000644\0");
        assert_eq!(field("uid"), b"0000000\0");
        assert_eq!(field("gid"), b"0000000\0");
        assert_eq!(field("size"), b"00000000005\0");
        assert_eq!(field("mtime"), b"00000000000\0");
        // Python's tarfile writes the same header for this file.
        assert_eq!(field("checksum"), b"010735\0 ");
        assert_eq!(field("typeflag"), b"0");
        assert_eq!(field("magic"), b"ustar\0");
        assert_eq!(field("version"), b"00");
        for empty in [
            "linkname", "uname", "gname", "devmajor", "devminor", "prefix", "pad",
        ] {
            assert!(field(empty).iter().all(|&byte| byte == 0), "{empty}");
        }
        let covered: usize = FIELDS.iter().map(|&(_, start, end)| end - start).sum();
        assert_eq!(covered, BLOCK);
    }

    #[test]
    fn a_hundred_byte_name_fills_its_field_without_a_nul() {
        let name = [b'a'; 100];
        let header = header(&name, 0);
        assert_eq!(&header[..100], &name);
        assert_eq!(&header[100..108], b"0000644\0");
    }

    #[test]
    fn data_is_padded_to_whole_blocks() {
        for (len, pad) in [(0, 0), (1, 511), (511, 1), (512, 0), (513, 511), (1024, 0)] {
            assert_eq!(padding(len), pad, "{len}");
        }
    }

    #[test]
    fn an_empty_bundle_is_the_two_closing_blocks() {
        assert_eq!(write(&[]), vec![0; END]);
    }
}
