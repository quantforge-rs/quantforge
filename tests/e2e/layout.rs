//! Keeps `docs/architecture.md` in step with the source tree, so a reader
//! can go from a command's name to its file: the annotated tree lists exactly
//! the `.rs` files under `src/`, every entry carries a note, and each
//! command's module is labeled with the command it implements.

use crate::snapshot::COMMANDS;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The annotated tree in `docs/architecture.md` — the `text` code block
/// whose first line is `src/` — as repository-relative path to note.
/// Directory paths keep their trailing `/`.
fn documented_tree() -> BTreeMap<String, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs")
        .join("architecture.md");
    let doc = std::fs::read_to_string(&path).expect("read docs/architecture.md");
    let lines: Vec<&str> = doc.lines().collect();
    let start = lines
        .windows(2)
        .position(|pair| pair == ["```text", "src/"])
        .expect("docs/architecture.md has a text block starting with src/");

    let mut tree = BTreeMap::new();
    let mut parents: Vec<&str> = Vec::new();
    for line in lines[start + 2..]
        .iter()
        .take_while(|line| !line.starts_with("```"))
    {
        let name_at = line
            .find(|c: char| !matches!(c, '│' | '├' | '└' | '─' | ' '))
            .unwrap_or_else(|| panic!("docs/architecture.md: tree line without a name: {line:?}"));
        let prefix = line[..name_at].chars().count();
        let depth = prefix / 4;
        assert!(
            depth * 4 == prefix && depth >= 1 && depth <= parents.len() + 1,
            "docs/architecture.md: tree line is not one level of four characters below its parent: {line:?}"
        );
        parents.truncate(depth - 1);
        let (name, note) = match line[name_at..].split_once(' ') {
            Some((name, note)) => (name, note.trim()),
            None => (&line[name_at..], ""),
        };
        let dirs: String = parents.iter().map(|dir| format!("{dir}/")).collect();
        let path = format!("src/{dirs}{name}");
        assert!(!note.is_empty(), "docs/architecture.md: {path} has no note");
        assert!(
            tree.insert(path.clone(), note.to_string()).is_none(),
            "docs/architecture.md lists {path} twice"
        );
        if let Some(dir) = name.strip_suffix('/') {
            parents.push(dir);
        }
    }
    tree
}

/// Every `.rs` file under `dir`, as repository-relative paths with `/`
/// separators on every platform.
fn source_files(dir: &Path, relative: &str, files: &mut BTreeSet<String>) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name().into_string().expect("utf-8 file name");
        let path = format!("{relative}{name}");
        if entry.file_type().expect("file type").is_dir() {
            source_files(&entry.path(), &format!("{path}/"), files);
        } else if name.ends_with(".rs") {
            files.insert(path);
        }
    }
}

// The tree is the whole of src/: a new, split, or renamed file lands in the
// doc in the same change, and an old path — such as a pre-split flat module —
// cannot linger in it.
#[test]
fn architecture_doc_lists_every_source_file_and_nothing_else() {
    let tree = documented_tree();
    let mut on_disk = BTreeSet::new();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    source_files(&src, "src/", &mut on_disk);
    assert!(
        on_disk.len() > COMMANDS.len(),
        "expected to find the source files under {}",
        src.display()
    );

    let documented: BTreeSet<String> = tree
        .keys()
        .filter(|path| !path.ends_with('/'))
        .cloned()
        .collect();
    let missing: Vec<&String> = on_disk.difference(&documented).collect();
    let stale: Vec<&String> = documented.difference(&on_disk).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "docs/architecture.md is out of step with src/\n  \
         missing from the tree: {missing:?}\n  \
         in the tree but not in src/: {stale:?}"
    );

    for dir in tree.keys().filter(|path| path.ends_with('/')) {
        assert!(
            documented.iter().any(|file| file.starts_with(dir.as_str())),
            "docs/architecture.md lists {dir} with no files under it"
        );
    }
}

// The point of the doc: from a command's name to its file. COMMANDS is the
// command list snapshot.rs keeps complete and mirrored on disk.
#[test]
fn every_command_module_is_labeled_with_its_command() {
    let tree = documented_tree();
    let mut labeled = BTreeMap::new();
    for (name, args, module) in COMMANDS {
        let command = std::iter::once("quantforge")
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        let flat = format!("src/cli/{module}.rs");
        let file = if module.is_empty() {
            "src/cli/mod.rs".to_string()
        } else if tree.contains_key(&flat) {
            flat
        } else {
            format!("src/cli/{module}/mod.rs")
        };
        let note = tree.get(&file).unwrap_or_else(|| {
            panic!("`{command}` ({name}): docs/architecture.md has no entry for {file}")
        });
        assert!(
            note.strip_prefix(command.as_str())
                .is_some_and(|rest| rest.starts_with(':')),
            "docs/architecture.md: the note for {file} must start with `{command}:`, got `{note}`"
        );
        labeled.insert(file, command);
    }

    // A label on any other entry would send a reader to the wrong file.
    for (path, note) in &tree {
        let looks_like_a_command =
            note.starts_with("quantforge ") || note.starts_with("quantforge:");
        assert!(
            !looks_like_a_command || labeled.contains_key(path),
            "docs/architecture.md: {path} is labeled like a command (`{note}`) but implements none"
        );
    }
}
