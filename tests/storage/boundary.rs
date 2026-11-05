//! The boundary around the stores: a selection holds only if nothing can go
//! around it. Engines, the SDK and the server reach storage through the
//! ports; the CLI's context alone asks the factory for a store; and only the
//! factory opens one. A concrete backend named anywhere else could open a
//! store the operator did not select, such as a SQLite file beside PostgreSQL.
//!
//! The scans are textual, as in `tests/exchange_boundary.rs`: comments and
//! test code are skipped, and everything else is checked.

use std::collections::BTreeMap;
use std::path::Path;

/// What only `src/storage/` may name: a backend's store and its driver.
const BACKEND_NAMES: [&str; 6] = [
    "SqliteStore",
    "PostgresStore",
    "storage::sqlite",
    "storage::postgres",
    "rusqlite",
    "tokio_postgres",
];

/// The code outside `src/storage/` that may, as `(file, trimmed line)`: the
/// crate's public name for the default store. Each entry must match exactly
/// once, so a stale one fails as well as a new line.
const NAMES_A_BACKEND: [(&str, &str); 1] = [("src/lib.rs", "pub use storage::SqliteStore;")];

/// What opens a store.
const OPENS_A_STORE: [&str; 3] = [
    "SqliteStore::new(",
    "SqliteStore::with_limits(",
    "PostgresStore::connect(",
];

/// Where a store is defined, and what may call its own constructors.
const ADAPTERS: [&str; 2] = ["src/storage/sqlite/", "src/storage/postgres/"];

/// The opening calls the factory makes, one per backend.
const FACTORY: &str = "src/storage/factory.rs";

/// What a host holds, and how it gets it: nothing below the CLI names them.
const HOST_NAMES: [&str; 2] = ["AnyStore", "StoreFactory"];

/// The one line that asks the factory for a store.
const OPENS_THROUGH_THE_FACTORY: (&str, &str) =
    ("src/cli/context.rs", "let store = StoreFactory::builtin()");

/// Every `.rs` file of the crate outside a test module's own file, keyed by
/// its `/`-joined path from the crate root on every platform.
fn sources() -> BTreeMap<String, String> {
    fn walk(dir: &Path, relative: &str, files: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).expect("read source dir") {
            let entry = entry.expect("dir entry");
            let name = entry.file_name().into_string().expect("utf-8 file name");
            let path = format!("{relative}/{name}");
            if entry.file_type().expect("file type").is_dir() {
                if name != "tests" {
                    walk(&entry.path(), &path, files);
                }
            } else if name.ends_with(".rs") && name != "tests.rs" {
                let text = std::fs::read_to_string(entry.path()).expect("read source file");
                files.insert(path, text);
            }
        }
    }
    let mut files = BTreeMap::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    walk(&root.join("src"), "src", &mut files);
    assert!(files.len() > 40, "scanned only {} files", files.len());
    files
}

/// The trimmed code lines of `text`: no comments, and nothing from an inline
/// test module on. An out-of-line one is only a declaration.
fn code(text: &str) -> Vec<&str> {
    let mut lines = text.lines().map(str::trim).peekable();
    let mut code = Vec::new();
    while let Some(line) = lines.next() {
        if line == "#[cfg(test)]" && lines.peek().is_some_and(|next| next.ends_with('{')) {
            break;
        }
        if !line.starts_with("//") {
            code.push(line);
        }
    }
    code
}

/// `(file, line)` for every code line of `files` that `names` matches.
fn lines_naming(
    files: &BTreeMap<String, String>,
    in_scope: impl Fn(&str) -> bool,
    names: &[&str],
) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for (path, text) in files.iter().filter(|(path, _)| in_scope(path)) {
        for line in code(text) {
            if names.iter().any(|name| line.contains(name)) {
                found.push((path.clone(), line.to_string()));
            }
        }
    }
    found
}

fn owned(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(path, line)| ((*path).to_string(), (*line).to_string()))
        .collect()
}

#[test]
fn only_storage_names_a_backend() {
    let found = lines_naming(
        &sources(),
        |path| !path.starts_with("src/storage/"),
        &BACKEND_NAMES,
    );
    assert_eq!(found, owned(&NAMES_A_BACKEND));
}

#[test]
fn only_the_factory_opens_a_store() {
    let files = sources();
    let found = lines_naming(
        &files,
        |path| !ADAPTERS.iter().any(|adapter| path.starts_with(adapter)),
        &OPENS_A_STORE,
    );
    let opened_by: Vec<&str> = found.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(opened_by, [FACTORY, FACTORY], "{found:?}");
}

#[test]
fn only_the_cli_holds_a_store_and_only_its_context_opens_one() {
    let files = sources();
    let below_the_cli = lines_naming(
        &files,
        |path| {
            !path.starts_with("src/cli/")
                && !path.starts_with("src/storage/")
                && path != "src/lib.rs"
        },
        &HOST_NAMES,
    );
    assert_eq!(below_the_cli, Vec::new());

    let opened = lines_naming(
        &files,
        |path| path.starts_with("src/cli/"),
        &["StoreFactory::"],
    );
    assert_eq!(opened, owned(&[OPENS_THROUGH_THE_FACTORY]));
}

/// The scans would pass on an empty crate: this is what they must see.
#[test]
fn the_scans_skip_comments_and_test_modules_and_nothing_else() {
    let text = "use a;\n// use b;\n/// use c;\n#[cfg(test)]\nmod tests;\nuse d;\n\
                #[cfg(test)]\nmod tests {\nuse e;\n}\n";
    assert_eq!(
        code(text),
        ["use a;", "#[cfg(test)]", "mod tests;", "use d;"]
    );
}
