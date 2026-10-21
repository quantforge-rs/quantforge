//! The strict reader: how a server checks that an upload is exactly the
//! canonical tar of the files it holds, and nothing else.

use super::archive::{self, BLOCK, END, FIELDS};
use super::files::{BundleLimits, Candidate};
use super::problems::{BundleProblem, ProblemCode};

/// The files of a canonical tar, in order. The first fault stops the read:
/// after it, offsets mean nothing. A tar this accepts is the one `write`
/// makes of the files it returns, byte for byte.
pub(super) fn read(tar: &[u8], limits: &BundleLimits) -> Result<Vec<Candidate>, BundleProblem> {
    let length = tar.len();
    if u64::try_from(length).unwrap_or(u64::MAX) > limits.max_archive_bytes() {
        return Err(BundleProblem::new(
            ProblemCode::BundleTooLarge,
            format!(
                "the archive is {length} bytes, more than a bundle within the limits can take, {} \
                 bytes",
                limits.max_archive_bytes()
            ),
        ));
    }
    if length % BLOCK != 0 {
        return Err(not_canonical(format!(
            "the archive is {length} bytes, not a whole number of 512-byte blocks"
        )));
    }
    let mut candidates = Vec::new();
    let mut previous: Option<&[u8]> = None;
    let mut offset = 0;
    loop {
        let Some(header) = tar.get(offset..offset + BLOCK) else {
            return Err(not_canonical(
                "the archive ends without the two zero blocks that close it".to_string(),
            ));
        };
        let rest = &tar[offset..];
        if header.iter().all(|&byte| byte == 0) {
            let all_zero = rest.iter().all(|&byte| byte == 0);
            return if all_zero && rest.len() == END {
                Ok(candidates)
            } else if all_zero && rest.len() > END {
                Err(not_canonical(
                    "the archive is padded past its two closing zero blocks; send it without \
                     record padding"
                        .to_string(),
                ))
            } else if all_zero {
                Err(not_canonical(
                    "the archive ends with one zero block where it needs two".to_string(),
                ))
            } else {
                Err(not_canonical(
                    "the archive goes on after a zero block; it ends with exactly two zero \
                     blocks"
                        .to_string(),
                ))
            };
        }
        let index = candidates.len() + 1;
        let name_length = header[..100]
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(100);
        let name = &header[..name_length];
        let shown = std::str::from_utf8(name)
            .map(str::to_owned)
            .map_err(|_| not_canonical(format!("entry {index} has a non-UTF-8 name")))?;
        let entry = format!("entry {index} ({shown:?})");
        if name.is_empty() {
            return Err(not_canonical(format!("entry {index} has no name")));
        }
        let Some(size) = parse_size(&header[124..136]) else {
            return Err(not_canonical(format!(
                "{entry}: its size is not 11 octal digits and a NUL"
            )));
        };
        let canonical = archive::header(name, size);
        if header != canonical {
            return Err(not_canonical(format!(
                "{entry}: {}",
                difference(header, &canonical)
            )));
        }
        if previous.is_some_and(|previous| name <= previous) {
            return Err(not_canonical(format!(
                "{entry} is out of order; a canonical archive lists each path once, sorted by \
                 its bytes"
            )));
        }
        let data_start = offset + BLOCK;
        let data_end = usize::try_from(size)
            .ok()
            .and_then(|size| Some((size, size.checked_add(archive::padding(size))?)))
            .and_then(|(size, padded)| Some((size, data_start.checked_add(padded)?)));
        let Some((size, data_end)) = data_end.filter(|&(_, end)| end <= length) else {
            return Err(not_canonical(format!(
                "{entry} runs past the end of the archive"
            )));
        };
        let data = &tar[data_start..data_start + size];
        if tar[data_start + size..data_end]
            .iter()
            .any(|&byte| byte != 0)
        {
            return Err(not_canonical(format!(
                "{entry}: the padding after its data is not all zero"
            )));
        }
        candidates.push(Candidate::read(shown, data.to_vec()));
        previous = Some(name);
        offset = data_end;
    }
}

fn not_canonical(reason: String) -> BundleProblem {
    BundleProblem::new(
        ProblemCode::ArchiveNotCanonical,
        format!(
            "the archive is not the canonical tar of its files: {reason}; build it with a builder \
             that follows the bundle spec"
        ),
    )
}

/// Exactly 11 octal digits and a NUL, as the canonical header writes a size.
fn parse_size(field: &[u8]) -> Option<u64> {
    let (&last, digits) = field.split_last()?;
    if last != 0 || digits.len() != 11 {
        return None;
    }
    digits.iter().try_fold(0u64, |size, &digit| {
        (b'0'..=b'7')
            .contains(&digit)
            .then(|| size * 8 + u64::from(digit - b'0'))
    })
}

/// The first field where `found` differs from the canonical header. The
/// checksum covers every other field, so it is reported only when nothing
/// else differs.
fn difference(found: &[u8], canonical: &[u8; BLOCK]) -> String {
    let differs = |start: usize, end: usize| found.get(start..end) != canonical.get(start..end);
    let field = FIELDS
        .iter()
        .find(|&&(field, start, end)| field != "checksum" && differs(start, end));
    match field {
        Some(&("typeflag", start, _)) => format!(
            "its type is {:?}, but a bundle holds only regular files, type '0'",
            char::from(found.get(start).copied().unwrap_or_default())
        ),
        Some(&(field, ..)) => format!("its {field} field differs from the canonical header"),
        None => "its checksum is wrong, or not six octal digits, a NUL, and a space".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::path::{BundleFile, BundlePath};
    use super::*;

    fn tar_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let files: Vec<BundleFile> = files
            .iter()
            .map(|&(path, data)| {
                BundleFile::new(BundlePath::new(path).expect("path"), data.to_vec())
            })
            .collect();
        archive::write(&files)
    }

    fn refusal(tar: &[u8]) -> String {
        match read(tar, &BundleLimits::default()) {
            Ok(files) => panic!("read {} files from a non-canonical archive", files.len()),
            Err(problem) => {
                assert_eq!(problem.code(), ProblemCode::ArchiveNotCanonical);
                problem.message().to_string()
            }
        }
    }

    /// Patches a header field and rewrites the checksum, so that only the
    /// patched field is wrong.
    fn patched(mut tar: Vec<u8>, at: usize, bytes: &[u8]) -> Vec<u8> {
        tar[at..at + bytes.len()].copy_from_slice(bytes);
        tar[148..156].copy_from_slice(b"        ");
        let sum: u64 = tar[..BLOCK].iter().map(|&byte| u64::from(byte)).sum();
        tar[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        tar
    }

    #[test]
    fn a_canonical_archive_reads_back_to_its_files() {
        let tar = tar_of(&[
            ("a.py", b"x = 1\n"),
            ("b/c.json", b"{}"),
            ("d.txt", &[0; 512]),
        ]);
        let files = read(&tar, &BundleLimits::default()).expect("canonical");
        let read_back: Vec<(&str, usize)> = files
            .iter()
            .map(|file| {
                (
                    file.path.as_str(),
                    file.contents.as_ref().map_or(0, Vec::len),
                )
            })
            .collect();
        assert_eq!(read_back, [("a.py", 6), ("b/c.json", 2), ("d.txt", 512)]);
    }

    #[test]
    fn each_header_field_that_is_not_canonical_is_named() {
        let tar = tar_of(&[("a.py", b"x")]);
        for (at, bytes, reason) in [
            (100, &b"0000755\0"[..], "its mode field differs"),
            (108, b"0001750\0", "its uid field differs"),
            (136, b"14000000000\0", "its mtime field differs"),
            (156, b"5", "its type is '5'"),
            (156, b"2", "its type is '2'"),
            (156, b"x", "its type is 'x'"),
            (156, b"\0", "its type is '\\0'"),
            (257, b"ustar ", "its magic field differs"),
            (265, b"root", "its uname field differs"),
            (329, b"0000000\0", "its devmajor field differs"),
            (345, b"pkg", "its prefix field differs"),
        ] {
            let message = refusal(&patched(tar.clone(), at, bytes));
            assert!(
                message.contains(&format!("entry 1 (\"a.py\"): {reason}")),
                "{message}"
            );
        }
        let mut wrong_sum = tar.clone();
        wrong_sum[153] = if wrong_sum[153] == b'7' { b'6' } else { b'7' };
        assert!(refusal(&wrong_sum).contains("its checksum is wrong"));
        let spaced = patched(tar, 124, b"          1 ");
        assert!(refusal(&spaced).contains("its size is not 11 octal digits and a NUL"));
    }

    #[test]
    fn anything_around_the_entries_that_is_not_canonical_is_refused() {
        let tar = tar_of(&[("a.py", b"x"), ("b.py", b"y")]);
        let mut padded = tar.clone();
        padded.resize(10240, 0);
        assert!(refusal(&padded).contains("padded past its two closing zero blocks"));
        let mut extra = tar.clone();
        extra.extend_from_slice(&[1; BLOCK]);
        assert!(refusal(&extra).contains("goes on after a zero block"));
        assert!(refusal(&tar[..tar.len() - BLOCK]).contains("ends with one zero block"));
        assert!(refusal(&tar[..2 * BLOCK]).contains("ends without the two zero blocks"));
        assert!(refusal(&tar[..tar.len() - 1]).contains("not a whole number of 512-byte blocks"));
        let mut dirty = tar.clone();
        dirty[BLOCK + 1] = b'!';
        assert!(refusal(&dirty).contains("the padding after its data is not all zero"));
        let mut swapped = tar.clone();
        swapped[..2 * BLOCK].copy_from_slice(&tar[2 * BLOCK..4 * BLOCK]);
        swapped[2 * BLOCK..4 * BLOCK].copy_from_slice(&tar[..2 * BLOCK]);
        assert!(refusal(&swapped).contains("entry 2 (\"a.py\") is out of order"));
        let b = &tar[2 * BLOCK..4 * BLOCK];
        let twice = [b, b, &tar[4 * BLOCK..]].concat();
        assert!(refusal(&twice).contains("entry 2 (\"b.py\") is out of order"));
        // A size of 4096 bytes, in an archive of 2048.
        let long = patched(tar, 124, b"00000010000\0");
        assert!(refusal(&long[..4 * BLOCK]).contains("runs past the end of the archive"));
    }

    #[test]
    fn an_archive_larger_than_the_limits_allow_is_refused_unread() {
        let limits = BundleLimits {
            max_file_bytes: 10,
            max_total_bytes: 10,
            max_files: 1,
        };
        let tar = tar_of(&[("a.py", b"x"), ("b.py", b"y")]);
        let problem = read(&tar, &limits).expect_err("too large");
        assert_eq!(problem.code(), ProblemCode::BundleTooLarge);
        assert_eq!(
            problem.message(),
            "the archive is 3072 bytes, more than a bundle within the limits can take, 2057 bytes"
        );
    }
}
