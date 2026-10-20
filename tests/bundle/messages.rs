//! What a refused bundle says. Each message names what is wrong and how to
//! fix it, in one line, so that a client can show it as it is.

use quantforge::protocol::bundle::{Bundle, BundleLimits, Runtime};

use crate::fixture;

const MANIFEST: &str =
    "format = 1\nentry = \"strategy.py:Hold\"\nengine = \">=0.3.0, <0.4.0\"\npython = \"3.12\"\n";
const STRATEGY: &str = "class Hold:\n    pass\n";

fn refusal(files: &[(&str, &str)], limits: &BundleLimits) -> Vec<String> {
    match Bundle::from_files(files.iter().copied(), limits) {
        Ok(bundle) => panic!("accepted as {}", bundle.code_hash()),
        Err(error) => error
            .problems()
            .iter()
            .map(|problem| problem.message().to_string())
            .collect(),
    }
}

fn manifest_refusal(manifest: &str) -> Vec<String> {
    refusal(
        &[("quantforge.toml", manifest), ("strategy.py", STRATEGY)],
        &BundleLimits::default(),
    )
}

#[test]
fn a_bad_entry_says_what_to_write() {
    assert_eq!(
        manifest_refusal(&MANIFEST.replace("strategy.py:Hold", "strategy.py")),
        [
            "invalid entry \"strategy.py\": it must name a file and a class, as in \
          \"strategy.py:MyStrategy\""
        ]
    );
    assert_eq!(
        manifest_refusal(&MANIFEST.replace("strategy.py:Hold", "strat.py:Hold")),
        [
            "the entry names \"strat.py\", which the bundle does not hold; its Python files are \
          strategy.py"
        ]
    );
    assert_eq!(
        manifest_refusal(&MANIFEST.replace("entry = \"strategy.py:Hold\"\n", "")),
        ["quantforge.toml has no entry; add entry = \"strategy.py:MyStrategy\""]
    );
    assert_eq!(
        manifest_refusal(&MANIFEST.replace("strategy.py:Hold", "my-strategy.py:Hold")),
        [
            "invalid entry \"my-strategy.py:Hold\": \"my-strategy.py\" cannot be imported: \
          \"my-strategy\" is not a Python identifier: letters, digits, and _, not starting with \
          a digit"
        ]
    );
}

#[test]
fn a_dependency_the_server_does_not_offer_names_what_it_does() {
    let manifest = format!("{MANIFEST}[deps]\nnumpy = \"2.1.3\"\npolars = \"1.9.0\"\n");
    let bundle = Bundle::from_files(
        [
            ("quantforge.toml", manifest.as_str()),
            ("strategy.py", STRATEGY),
        ],
        &BundleLimits::default(),
    )
    .expect("a bundle");
    let runtime: Runtime = serde_json::from_str(
        r#"{"engine":"0.4.1","python":"3.11","packages":{"numpy":["2.0.2","2.1.2"],"pandas":["2.2.3"]}}"#,
    )
    .expect("a runtime");
    let error = bundle.admit(&runtime).expect_err("not admitted");
    let messages: Vec<&str> = error
        .problems()
        .iter()
        .map(|problem| problem.message())
        .collect();
    assert_eq!(
        messages,
        [
            "the bundle needs engine >=0.3.0, <0.4.0, but this server runs 0.4.1; change engine \
             in the manifest or use a matching server",
            "the bundle was written for Python 3.12, but this server runs Python 3.11",
            "the bundle pins numpy 2.1.3, but this server offers numpy 2.0.2, 2.1.2 only",
            "the bundle imports polars, which this server does not offer; it offers numpy, pandas",
        ]
    );
    let empty = Runtime {
        packages: Default::default(),
        ..runtime
    };
    let error = bundle.admit(&empty).expect_err("not admitted");
    assert!(
        error.problems()[2]
            .message()
            .ends_with("which this server does not offer; it offers no packages"),
        "{error}"
    );
}

#[test]
fn a_file_over_a_limit_says_how_far_over() {
    let limits = BundleLimits {
        max_file_bytes: 100,
        max_total_bytes: 150,
        max_files: 3,
    };
    let big = "b".repeat(101);
    assert_eq!(
        refusal(
            &[
                ("quantforge.toml", MANIFEST),
                ("strategy.py", STRATEGY),
                ("big.txt", &big),
                ("more.txt", "m"),
            ],
            &limits,
        ),
        [
            "\"big.txt\" is 101 bytes, over the limit of 100 bytes for one file",
            "the bundle holds more than the limit of 3 files; leave out what the strategy does \
             not need",
            "the bundle's files hold 204 bytes, over the limit of 150 bytes in all",
        ]
    );
}

#[test]
fn a_manifest_that_is_not_bundle_toml_says_where() {
    assert_eq!(
        manifest_refusal("format = 1\nentry =\n"),
        ["quantforge.toml line 2, column 8: invalid string; expected `\"`, `'`"]
    );
    assert_eq!(
        manifest_refusal(&format!("\u{feff}{MANIFEST}")),
        [
            "quantforge.toml starts with a byte-order mark, which Python's tomllib refuses; save it \
          as UTF-8 without one"
        ]
    );
    assert_eq!(
        manifest_refusal(&format!("{MANIFEST}[params]\nratio = nan\n")),
        ["quantforge.toml holds NaN at params.ratio, but a manifest's numbers are finite"]
    );
    assert_eq!(
        manifest_refusal(&MANIFEST.replace("format = 1", "format = 2")),
        ["quantforge.toml follows bundle format 2, but this side reads format 1 only"]
    );
    assert_eq!(
        manifest_refusal(&format!("{MANIFEST}parms = 1\n")),
        [
            "quantforge.toml has the key parms, which bundle format 1 does not define; its keys are \
          format, entry, engine, python, params, and deps"
        ]
    );
    assert_eq!(
        refusal(&[("strategy.py", STRATEGY)], &BundleLimits::default()),
        [
            "the bundle has no quantforge.toml at its root; add one naming the entry, the engine, \
          and the python version"
        ]
    );
}

#[test]
fn each_manifest_key_says_what_it_should_hold() {
    assert_eq!(
        manifest_refusal(&MANIFEST.replace(">=0.3.0, <0.4.0", ">=0.3")),
        [
            "invalid engine requirement \">=0.3\": \"0.3\" is not a MAJOR.MINOR.PATCH version; write \
          comparators on MAJOR.MINOR.PATCH versions, such as \">=0.3.0, <0.4.0\""
        ]
    );
    assert_eq!(
        manifest_refusal(&MANIFEST.replace("python = \"3.12\"", "python = 3.12")),
        ["quantforge.toml's python must be a string, such as \"3.12\", not a float"]
    );
    assert_eq!(
        manifest_refusal(&format!("{MANIFEST}[params]\nstart = 2024-01-01\n")),
        ["params.start is a TOML date or time, which JSON cannot hold; write it as a string"]
    );
    assert_eq!(
        manifest_refusal(&format!(
            "{MANIFEST}[deps]\nPyYAML = \"6.0.2\"\nnumpy = \"==2.1.3\"\n"
        )),
        [
            "deps.PyYAML: invalid package name \"PyYAML\": names are compared in their normalized \
             form, so write it as \"pyyaml\"",
            "deps.numpy: invalid package version \"==2.1.3\": expected a canonical version such \
             as \"2.1.3\", \"1.0rc1\", or \"2.0.post1\"",
        ]
    );
}

// Every refusal in the shared cases, so the rule covers each kind of
// problem, not just the ones quoted above.
#[test]
fn every_message_is_one_lowercase_line_without_a_trailing_period() {
    let all = fixture::cases();
    let mut checked = 0;
    for case in all.iter().filter(|case| case.code_hash.is_none()) {
        let Some(files) = std::panic::catch_unwind(|| case.files()).ok() else {
            continue;
        };
        let limits = BundleLimits::default();
        let Err(error) = Bundle::from_files(files, &limits) else {
            continue;
        };
        for problem in error.problems() {
            let message = problem.message();
            let first = message.chars().next().unwrap_or_default();
            assert!(
                !message.contains('\n') && !message.ends_with('.'),
                "{}: {message:?}",
                case.name
            );
            assert!(
                first.is_lowercase() || first == '"',
                "{}: {message:?}",
                case.name
            );
            checked += 1;
        }
    }
    assert!(checked > 40, "checked only {checked} messages");
}
