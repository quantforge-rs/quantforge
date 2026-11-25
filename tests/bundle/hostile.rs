//! Bundles made to do harm, and bundles that are simply broken: each is
//! refused with the problem the spec names, before anything of it is kept,
//! and nothing a reader accepts names a place outside the bundle.
//!
//! - **A name that would leave the bundle**, however it is spelled, in
//!   memory and in an upload's tar.
//! - **An entry that is no regular file**: a link of either kind, a
//!   device, a directory, a header of another dialect.
//! - **An archive cut short**, at every length it can be cut to.
//! - **An archive that claims more than it holds**, a compressed one, and
//!   one with more entries than a bundle has: nothing is ever unpacked,
//!   and nothing is sized by what a header says.
//! - **A manifest nested as deep as it is long.**
//! - **Mutations**: thousands of them, from a fixed seed, of canonical tars.
//!   A reader refuses each or reads back exactly the bytes it was given.
//!
//! The archives here are written field by field from the spec's table, by
//! a writer of this file's own that writes whatever it is told: the
//! reference writer writes only what the rules allow.

use std::path::{Component, Path};

use quantforge::protocol::bundle::{Bundle, BundleError, BundleLimits, BundlePath, ProblemCode};
use sha2::{Digest, Sha256};

use crate::fixture;

const MANIFEST: &str =
    "format = 1\nentry = \"strategy.py:Hold\"\nengine = \">=0.3.0, <0.4.0\"\npython = \"3.12\"\n";
const STRATEGY: &str = "class Hold:\n    pass\n";

/// One entry of an archive, as a client that means harm writes it.
#[derive(Clone)]
struct Entry {
    name: Vec<u8>,
    data: Vec<u8>,
    typeflag: u8,
    linkname: Vec<u8>,
    prefix: Vec<u8>,
    /// The size its header claims, where that is not its data's.
    claimed: Option<Vec<u8>>,
}

impl Entry {
    fn file(name: impl AsRef<[u8]>, data: impl AsRef<[u8]>) -> Self {
        Self {
            name: name.as_ref().to_vec(),
            data: data.as_ref().to_vec(),
            typeflag: b'0',
            linkname: Vec::new(),
            prefix: Vec::new(),
            claimed: None,
        }
    }

    fn header(&self) -> [u8; 512] {
        let mut header = [0_u8; 512];
        let put = |header: &mut [u8; 512], at: usize, bytes: &[u8]| {
            header[at..at + bytes.len()].copy_from_slice(bytes);
        };
        put(&mut header, 0, &self.name);
        put(&mut header, 100, b"0000644\0");
        put(&mut header, 108, b"0000000\0");
        put(&mut header, 116, b"0000000\0");
        let size = format!("{:011o}\0", self.data.len()).into_bytes();
        put(&mut header, 124, self.claimed.as_deref().unwrap_or(&size));
        put(&mut header, 136, b"00000000000\0");
        put(&mut header, 148, b"        ");
        header[156] = self.typeflag;
        put(&mut header, 157, &self.linkname);
        put(&mut header, 257, b"ustar\0");
        put(&mut header, 263, b"00");
        put(&mut header, 345, &self.prefix);
        let sum: u32 = header.iter().map(|&byte| u32::from(byte)).sum();
        put(&mut header, 148, format!("{sum:06o}\0 ").as_bytes());
        header
    }
}

/// The archive of `entries` in the order given: each header, its data
/// padded to a block, and the two closing blocks.
fn archive(entries: &[Entry]) -> Vec<u8> {
    let mut tar = Vec::new();
    for entry in entries {
        tar.extend_from_slice(&entry.header());
        tar.extend_from_slice(&entry.data);
        tar.resize(tar.len().div_ceil(512) * 512, 0);
    }
    tar.resize(tar.len() + 1024, 0);
    tar
}

/// The spec's smallest bundle with `more` beside it, sorted by name as a
/// canonical archive is.
fn beside(more: Entry) -> Vec<u8> {
    let mut entries = vec![
        Entry::file("quantforge.toml", MANIFEST),
        Entry::file("strategy.py", STRATEGY),
        more,
    ];
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    archive(&entries)
}

fn read(tar: &[u8]) -> Result<Bundle, BundleError> {
    Bundle::from_canonical_tar(tar, &BundleLimits::default())
}

/// The codes a refused bundle's problems have, sorted.
fn codes(refused: &BundleError) -> Vec<&'static str> {
    let mut codes: Vec<&str> = refused
        .problems()
        .iter()
        .map(|problem| problem.code().as_str())
        .collect();
    codes.sort_unstable();
    codes
}

/// Holds a bundle a reader accepted to what makes it safe to write out:
/// `tar` is exactly the canonical tar of its files, so its hash names what
/// was read, and each path is of plain names alone, on this system as on
/// every other, so that it stays under any directory it is joined to.
fn assert_contained(bundle: &Bundle, tar: &[u8]) {
    assert_eq!(bundle.canonical_tar(), tar, "read back as other bytes");
    assert_eq!(
        bundle.code_hash().to_string(),
        format!("sha256:{}", hex::encode(Sha256::digest(tar)))
    );
    for file in bundle.files() {
        let path = file.path().as_str();
        assert!(BundlePath::new(path).is_ok(), "{path:?} breaks a path rule");
        assert!(
            Path::new(path)
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
            "{path:?} names more than files and directories of the bundle"
        );
        assert!(Path::new("root").join(path).starts_with("root"), "{path:?}");
    }
}

/// Names that would leave the bundle, or mean another file than they say,
/// on some system: each a file of an allowed extension, so that the path
/// rules are what refuses it.
const LEAVING: &[&str] = &[
    // Up and out.
    "../escape.py",
    "../../../../etc/cron.d/job.txt",
    "pkg/../../escape.py",
    "pkg/sub/../../../escape.py",
    "..",
    "../",
    "pkg/..",
    "./escape.py",
    "pkg/./escape.py",
    ".../escape.py",
    // From the root.
    "/etc/passwd.txt",
    "/escape.py",
    "//server/share/escape.py",
    "~/escape.py",
    "~root/escape.py",
    // As Windows reads a path.
    "..\\escape.py",
    "pkg\\..\\..\\escape.py",
    "C:\\escape.py",
    "C:/escape.py",
    "C:escape.py",
    "\\\\server\\share\\escape.py",
    "\\escape.py",
    "escape.py:stream.txt",
    "escape.py::$DATA.txt",
    "nul.py",
    "pkg/COM1.txt",
    "escape.py.",
    "pkg./escape.py",
    // Hidden, or left by a tool.
    ".ssh/authorized_keys.txt",
    ".env.txt",
    ".git/hooks/pre-commit.py",
    "__pycache__/escape.py",
    // Escapes a shell, a URL or a terminal would read.
    "$HOME/escape.py",
    "%2e%2e/escape.py",
    "..%2fescape.py",
    "pkg/%00/escape.py",
    "esc ape.py",
    " escape.py",
    "escape.py ",
    "escape\n.py",
    "escape\r.py",
    "escape\t.py",
    "escape\u{1b}[2J.py",
    "-rf.py",
    "pkg/*.py",
    "pkg/?.py",
    // Not one name on every system.
    "pkg//escape.py",
    "pkg/escape.py/",
    "/",
    // Dots that are not ASCII dots, and letters that are not ASCII's.
    "\u{2025}/escape.py",
    "\u{ff0e}\u{ff0e}/escape.py",
    "esc\u{430}pe.py",
    "escape\u{202e}yp.txt",
];

#[test]
fn a_name_that_would_leave_the_bundle_is_refused_however_it_is_spelled() {
    for name in LEAVING {
        let given = Bundle::from_files(
            [
                ("quantforge.toml", MANIFEST),
                ("strategy.py", STRATEGY),
                (name, "x"),
            ],
            &BundleLimits::default(),
        )
        .expect_err("refused as files");
        assert_eq!(codes(&given), ["path_invalid"], "{name:?}: {given}");

        let uploaded = read(&beside(Entry::file(name, "x"))).expect_err("refused as an upload");
        assert_eq!(codes(&uploaded), ["path_invalid"], "{name:?}: {uploaded}");
    }
    // A name no path rule ever sees: cut short by a NUL, with bytes after
    // it, or of bytes that are no text at all.
    for name in [
        &b"pkg\0/../../escape.py"[..],
        b"\0escape.py",
        b"\xff\xfe.py",
    ] {
        let refused = read(&beside(Entry::file(name, "x"))).expect_err("refused");
        assert_eq!(codes(&refused), ["archive_not_canonical"], "{name:?}");
    }
    // And one longer than a header holds is no entry a tar can carry.
    let long = format!("{}/../escape.py", "d".repeat(100));
    let refused =
        Bundle::from_files([(long.as_str(), "x")], &BundleLimits::default()).expect_err("refused");
    assert!(codes(&refused).contains(&"path_invalid"), "{refused}");
}

#[test]
fn an_entry_that_is_no_regular_file_is_refused_whatever_it_claims_to_be() {
    let outside = [&b"/etc/passwd"[..], b"../../etc/passwd", b"strategy.py"];
    // Every type a tar of any dialect gives an entry, but a regular file's.
    let types = [
        (b'1', "a hard link"),
        (b'2', "a symlink"),
        (b'3', "a character device"),
        (b'4', "a block device"),
        (b'5', "a directory"),
        (b'6', "a FIFO"),
        (b'7', "a contiguous file"),
        (b'x', "a pax header"),
        (b'g', "a global pax header"),
        (b'L', "a GNU long name"),
        (b'K', "a GNU long link"),
        (b'S', "a GNU sparse file"),
        (b'D', "a GNU directory dump"),
        (b'M', "a GNU volume's continuation"),
        (b'V', "a GNU volume label"),
        (b'N', "an old GNU long name"),
        (b'\0', "a file as tars before POSIX's marked one"),
        (b' ', "no type at all"),
    ];
    for (typeflag, what) in types {
        for linkname in outside {
            let entry = Entry {
                typeflag,
                linkname: linkname.to_vec(),
                ..Entry::file("linked.py", "")
            };
            let refused = read(&beside(entry)).expect_err("refused");
            assert_eq!(codes(&refused), ["archive_not_canonical"], "{what}");
            assert!(
                refused
                    .to_string()
                    .contains("a bundle holds only regular files, type '0'"),
                "{what}: {refused}"
            );
        }
    }
    // A regular file that names a link all the same, and one whose name
    // goes on in the field ustar keeps a path's first directories in.
    let linking = Entry {
        linkname: b"/etc/passwd".to_vec(),
        ..Entry::file("linked.py", "")
    };
    let refused = read(&beside(linking)).expect_err("refused");
    assert!(
        refused.to_string().contains("its linkname field differs"),
        "{refused}"
    );
    let prefixed = Entry {
        prefix: b"../..".to_vec(),
        ..Entry::file("prefixed.py", "")
    };
    let refused = read(&beside(prefixed)).expect_err("refused");
    assert!(
        refused.to_string().contains("its prefix field differs"),
        "{refused}"
    );
}

/// The canonical tar of a shared case's files.
fn canonical(case: &str) -> Vec<u8> {
    Bundle::from_files(fixture::case(case).files(), &BundleLimits::default())
        .expect("a bundle")
        .canonical_tar()
}

/// The shared cases the archives below are cut from and mutated.
const BASES: [&str; 3] = [
    "minimal",
    "package with params and deps",
    "files at block edges",
];

#[test]
fn an_archive_cut_short_is_refused_at_every_length() {
    for base in BASES {
        let tar = canonical(base);
        read(&tar).expect("the whole archive");
        for length in 0..tar.len() {
            let refused = match read(&tar[..length]) {
                Ok(bundle) => panic!(
                    "{base} cut to {length} bytes read as {}",
                    bundle.code_hash()
                ),
                Err(refused) => refused,
            };
            assert_eq!(
                codes(&refused),
                ["archive_not_canonical"],
                "{base} cut to {length} bytes: {refused}"
            );
        }
    }
}

#[test]
fn an_archive_that_claims_more_than_it_holds_is_refused_by_the_bytes_it_has() {
    // 8 GiB less a byte, the most a header's eleven octal digits say, in an
    // archive of a few blocks: nothing is sized by what a header claims.
    let claiming = |claimed: &[u8]| Entry {
        claimed: Some(claimed.to_vec()),
        ..Entry::file("zz.txt", "x")
    };
    let refused = read(&beside(claiming(b"77777777777\0"))).expect_err("refused");
    assert!(
        refused
            .to_string()
            .contains("runs past the end of the archive"),
        "{refused}"
    );
    // A size as GNU tar writes one past 8 GiB, in binary; a negative one;
    // and digits no octal number has.
    for claimed in [
        &b"\x80\0\0\0\x7f\xff\xff\xff\xff\xff\xff\xff"[..],
        b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff",
        b"-0000000001\0",
        b"99999999999\0",
        b"0x7fffffff \0",
    ] {
        let refused = read(&beside(claiming(claimed))).expect_err("refused");
        assert!(
            refused
                .to_string()
                .contains("its size is not 11 octal digits and a NUL"),
            "{claimed:?}: {refused}"
        );
    }
    // A file's worth of data less than its header says.
    let mut short = beside(Entry::file("zz.txt", [b'x'; 1024]));
    short.drain(short.len() - 1024 - 512..short.len() - 1024);
    let refused = read(&short).expect_err("refused");
    assert_eq!(codes(&refused), ["archive_not_canonical"], "{refused}");
}

#[test]
fn a_compressed_archive_is_refused_and_never_unpacked() {
    // Eight megabytes of nothing, which pack into a few hundred bytes: a
    // reader that unpacked what it was sent would take them all back.
    let packed = zstd::bulk::compress(&vec![0_u8; 8 << 20], 19).expect("packed");
    assert!(packed.len() < 4096, "{} bytes", packed.len());
    let whole = zstd::bulk::compress(&canonical("minimal"), 3).expect("packed");
    // What other packers begin a file with: gzip, zip, xz, bzip2, 7z.
    let begun: [&[u8]; 5] = [
        b"\x1f\x8b\x08\0\0\0\0\0\0\x03",
        b"PK\x03\x04\x14\0\0\0\x08\0",
        b"\xfd7zXZ\0\0\x04",
        b"BZh91AY&SY",
        b"7z\xbc\xaf\x27\x1c\0\x04",
    ];
    let bodies = [packed.as_slice(), whole.as_slice()]
        .into_iter()
        .chain(begun);
    for body in bodies {
        // As it is, and filled up to whole blocks, as a tar is.
        let mut filled = body.to_vec();
        filled.resize(body.len().div_ceil(512) * 512 + 1024, 0);
        for sent in [body, filled.as_slice()] {
            let refused = read(sent).expect_err("refused");
            assert_eq!(codes(&refused), ["archive_not_canonical"], "{refused}");
        }
    }
}

#[test]
fn more_entries_than_a_bundle_holds_are_refused_whatever_they_weigh() {
    // Empty files weigh a header each: sixteen thousand fit an upload.
    let mut entries = vec![
        Entry::file("quantforge.toml", MANIFEST),
        Entry::file("strategy.py", STRATEGY),
    ];
    entries.extend((0..16_000).map(|index| Entry::file(format!("e{index:05}.txt"), "")));
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let tar = archive(&entries);
    assert!(
        u64::try_from(tar.len()).expect("a size") <= BundleLimits::default().max_archive_bytes()
    );
    let refused = read(&tar).expect_err("refused");
    assert_eq!(codes(&refused), ["too_many_files"], "{refused}");
    // One block more than any bundle within the limits takes is refused
    // by its length, before a header of it is read.
    let most = usize::try_from(BundleLimits::default().max_archive_bytes()).expect("a size");
    let refused = read(&vec![b'x'; most + 512]).expect_err("refused");
    assert_eq!(codes(&refused), ["bundle_too_large"], "{refused}");
}

#[test]
fn a_manifest_nested_as_deep_as_it_is_long_is_refused_without_reading_it_that_deep() {
    let nested = |open: &str, close: &str, depth: usize| {
        format!(
            "{MANIFEST}[params]\nx = {}{}\n",
            open.repeat(depth),
            close.repeat(depth)
        )
    };
    for manifest in [
        nested("[", "]", 30_000),
        nested("{a=", "}", 15_000),
        // And one byte more than a manifest holds, of comment.
        format!("{MANIFEST}#{}\n", "x".repeat(65_536 - MANIFEST.len() - 1)),
    ] {
        assert!(manifest.len() <= 65_537, "{}", manifest.len());
        let refused = Bundle::from_files(
            [
                ("quantforge.toml", manifest.as_str()),
                ("strategy.py", STRATEGY),
            ],
            &BundleLimits::default(),
        )
        .expect_err("refused");
        assert_eq!(codes(&refused), ["manifest_invalid"], "{refused}");
    }
}

/// xorshift64*, from a seed: the same archives every run.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number below `bound`, which is above zero.
    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(bound).expect("a bound")).expect("below it")
    }

    fn of<'a, T>(&mut self, choices: &'a [T]) -> &'a T {
        &choices[self.below(choices.len())]
    }
}

/// Bytes that mean something to a reader of paths, octal or tar.
const BYTES: [u8; 14] = [
    0, 0xff, 0x80, b'/', b'\\', b'.', b' ', b'\n', b'0', b'7', b'8', b'2', b'5', b'x',
];

/// A header's fields, as `(start, length)`, and what a hostile writer puts
/// in one.
const FIELDS: [(usize, usize); 9] = [
    (0, 100),
    (100, 8),
    (124, 12),
    (136, 12),
    (156, 1),
    (157, 100),
    (257, 6),
    (263, 2),
    (345, 155),
];
const TOKENS: [&[u8]; 14] = [
    b"../escape.py",
    b"/etc/passwd",
    b"..\\escape.py",
    b"quantforge.toml",
    b"strategy.py",
    b"77777777777\0",
    b"00000000000\0",
    b"\x80\0\0\0\0\0\0\0\0\0\x10\0",
    b"2",
    b"5",
    b"L",
    b"x",
    b"ustar  \0",
    b"\0",
];

/// Rewrites the checksum of the block at `block`, so that what changed in
/// it is read, and not just refused for its sum.
fn fix_checksum(tar: &mut [u8], block: usize) {
    let Some(header) = tar.get_mut(block * 512..(block + 1) * 512) else {
        return;
    };
    header[148..156].copy_from_slice(b"        ");
    let sum: u32 = header.iter().map(|&byte| u32::from(byte)).sum();
    header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
}

/// Changes `tar` in one of the ways an archive goes wrong, or is made to.
fn mutate(tar: &mut Vec<u8>, random: &mut Random) {
    let blocks = tar.len() / 512;
    match random.below(10) {
        _ if tar.is_empty() => tar.push(*random.of(&BYTES)),
        0 => {
            let at = random.below(tar.len());
            tar[at] ^= 1 << random.below(8);
        }
        1 => {
            let at = random.below(tar.len());
            tar[at] = *random.of(&BYTES);
        }
        2 => tar.truncate(random.below(tar.len())),
        3 => tar.truncate(random.below(blocks + 1) * 512),
        4 => {
            let more = random.below(1025);
            let byte = *random.of(&BYTES);
            tar.resize(tar.len() + more, byte);
        }
        5 if blocks >= 2 => {
            let (a, b) = (random.below(blocks) * 512, random.below(blocks) * 512);
            for offset in 0..512 {
                tar.swap(a + offset, b + offset);
            }
        }
        6 if blocks >= 1 => {
            let from = random.below(blocks) * 512;
            let block = tar[from..from + 512].to_vec();
            let at = random.below(blocks + 1) * 512;
            tar.splice(at..at, block);
        }
        7 if blocks >= 1 => {
            let at = random.below(blocks) * 512;
            tar.drain(at..at + 512);
        }
        8 if blocks >= 1 => {
            // A field of some block overwritten, and the block's sum made
            // right again: only the field is wrong.
            let block = random.below(blocks);
            let &(start, length) = random.of(&FIELDS);
            let token = random.of(&TOKENS);
            let kept = token.len().min(length);
            let at = block * 512 + start;
            tar[at..at + kept].copy_from_slice(&token[..kept]);
            fix_checksum(tar, block);
        }
        _ => fix_checksum(tar, random.below(blocks.max(1))),
    }
}

/// How a reader took one archive: `Some(true)` read, `Some(false)` refused,
/// and a failed test where it panicked.
fn taken(tar: &[u8], what: &str) -> bool {
    let read = std::panic::catch_unwind(|| read(tar));
    match read.unwrap_or_else(|_| panic!("the reader panicked on {what}: {}", hex::encode(tar))) {
        Ok(bundle) => {
            assert_contained(&bundle, tar);
            true
        }
        Err(refused) => {
            assert!(
                !refused.problems().is_empty(),
                "{what}: refused for nothing"
            );
            for problem in refused.problems() {
                assert!(
                    !problem.message().contains('\n'),
                    "{what}: {}",
                    problem.message()
                );
            }
            false
        }
    }
}

#[test]
fn no_mutation_of_a_canonical_tar_is_read_as_anything_but_the_bytes_it_is() {
    let mut random = Random(0x51a7_e5f0_0d5e_ed01);
    let (mut read, mut refused) = (0_u32, 0_u32);
    for base in BASES {
        let canonical = canonical(base);
        for round in 0..5_000 {
            let mut tar = canonical.clone();
            for _ in 0..=random.below(3) {
                mutate(&mut tar, &mut random);
            }
            if taken(&tar, &format!("round {round} of {base}")) {
                read += 1;
            } else {
                refused += 1;
            }
        }
    }
    // A changed byte of a file's own is another bundle, canonical as well;
    // anything else is refused.
    assert!(
        read > 0 && refused > 10 * read,
        "{read} read, {refused} refused"
    );
}

#[test]
fn no_run_of_bytes_is_read_as_a_bundle_unless_it_is_one() {
    let mut random = Random(0x0bad_5eed_0bad_5eed);
    for round in 0..3_000 {
        // Whole blocks more often than not: what gets past the length.
        let length = match random.below(4) {
            0 => random.below(4096),
            _ => random.below(9) * 512,
        };
        let mostly = *random.of(&BYTES);
        let tar: Vec<u8> = (0..length)
            .map(|_| match random.below(4) {
                0 => u8::try_from(random.below(256)).expect("a byte"),
                _ => mostly,
            })
            .collect();
        assert!(
            !taken(&tar, &format!("round {round}")) || tar.iter().all(|&byte| byte == 0),
            "round {round} read as a bundle"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_directory_that_holds_what_is_no_file_is_refused_and_none_of_it_is_opened() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().expect("a directory");
    std::fs::write(root.path().join("quantforge.toml"), MANIFEST).expect("the manifest");
    std::fs::write(root.path().join("strategy.py"), STRATEGY).expect("the strategy");
    // A pipe nobody writes to: a builder that opened it would wait for good.
    let made = std::process::Command::new("mkfifo")
        .arg(root.path().join("pipe.py"))
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "mkfifo made no pipe");
    // A link to itself, one to a file outside, one to the directory above,
    // and one to nothing.
    let outside = tempfile::tempdir().expect("a directory");
    std::fs::write(outside.path().join("secret.py"), "key = 1\n").expect("a file");
    symlink("loop.py", root.path().join("loop.py")).expect("a link");
    symlink(
        outside.path().join("secret.py"),
        root.path().join("secret.py"),
    )
    .expect("a link");
    symlink("..", root.path().join("up")).expect("a link");
    symlink("nowhere.py", root.path().join("dangling.py")).expect("a link");
    let refused = Bundle::from_dir(root.path(), &BundleLimits::default()).expect_err("refused");
    assert_eq!(codes(&refused), ["not_a_regular_file"; 5], "{refused}");
    let said = refused.to_string();
    for name in ["dangling.py", "loop.py", "pipe.py", "secret.py", "up"] {
        assert!(said.contains(&format!("{name:?} is ")), "{name}: {said}");
    }
    assert!(!said.contains("key = 1"), "{said}");
}

/// The threat model lists what is checked before anything runs, each check
/// with the problems it reports (`docs/security/remote-execution.md`):
/// every problem a bundle can have is some check's, and one check's alone,
/// so a problem the spec gains is placed there too.
#[test]
fn the_threat_model_places_every_problem_a_bundle_can_have_in_one_check() {
    let page = include_str!("../../docs/security/remote-execution.md");
    let lines: Vec<&str> = page.lines().collect();
    let marker = |kind: &str| {
        let marker = format!("<!-- {kind}: submission checks -->");
        lines
            .iter()
            .position(|line| line.trim() == marker)
            .unwrap_or_else(|| panic!("the threat model has no {marker}"))
    };
    let rows = &lines[marker("begin") + 3..marker("end")];
    assert!(rows.len() >= 8, "{} checks", rows.len());
    let mut listed: Vec<&str> = rows
        .iter()
        .flat_map(|row| {
            let codes = row.trim().trim_matches('|').rsplit('|').next();
            codes.expect("a row's last cell").split(',')
        })
        .map(|code| code.trim().trim_matches('`'))
        .collect();
    listed.sort_unstable();
    let mut codes: Vec<&str> = ProblemCode::ALL.iter().map(|code| code.as_str()).collect();
    codes.sort_unstable();
    assert_eq!(listed, codes);
}
