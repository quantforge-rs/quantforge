//! Unit tests for the root parser: flag parsing, and the drift checks that
//! hold `docs/configuration.md` and the README to the clap definitions.

use super::*;
use std::collections::BTreeSet;

// clap's own consistency self-check, on the command `parse()` runs:
// duplicate flags or ids and malformed argument settings panic here rather
// than at the first user invocation. It does not parse default values;
// clap only does that at run time.
#[test]
fn cli_definition_is_internally_consistent() {
    command().debug_assert();
}

// The variable form is covered end to end in `precedence`, where the
// environment is the test's own.
#[test]
fn the_renamed_base_url_flag_is_refused_without_repeating_its_value() {
    for argv in [
        &[
            "quantforge",
            "--binance-base-url",
            "https://user:secret@example.test/",
            "data",
            "validate",
            "--symbol",
            "BTCUSDT",
        ][..],
        &[
            "quantforge",
            "data",
            "validate",
            "--symbol",
            "BTCUSDT",
            "--binance-base-url",
            "https://user:secret@example.test/",
        ][..],
    ] {
        let matches = command().try_get_matches_from(argv).expect("matches");
        assert_eq!(
            renamed_setting_error(&matches).as_deref(),
            Some("--binance-base-url was renamed to --base-url"),
            "for {argv:?}"
        );
    }
}

// A shell that exports the env fallbacks only turns the absent case's
// source into EnvVariable, which still is not the command line.
#[test]
fn credential_flags_count_as_command_line_on_either_side_of_the_subcommand() {
    let cases: [(&[&str], bool); 3] = [
        (
            &[
                "quantforge",
                "--api-key",
                "k",
                "data",
                "validate",
                "--symbol",
                "BTCUSDT",
            ],
            true,
        ),
        (
            &[
                "quantforge",
                "data",
                "validate",
                "--symbol",
                "BTCUSDT",
                "--api-secret",
                "s",
            ],
            true,
        ),
        (
            &["quantforge", "data", "validate", "--symbol", "BTCUSDT"],
            false,
        ),
    ];
    for (argv, expected) in cases {
        let matches = command().try_get_matches_from(argv).expect("matches");
        assert_eq!(
            credentials_on_command_line(&matches),
            expected,
            "for {argv:?}"
        );
    }
}

#[test]
fn credential_args_debug_never_prints_values() {
    let cli = Cli::try_parse_from([
        "quantforge",
        "--api-key",
        "key-sentinel",
        "--api-secret",
        "secret-sentinel",
        "data",
        "validate",
        "--symbol",
        "BTCUSDT",
    ])
    .expect("cli");

    // Failure messages never include the output itself: a failing run
    // must not print what leaked.
    let debug = format!("{cli:?}");
    assert!(
        debug.contains("<redacted>"),
        "Debug output lacks the redaction marker"
    );
    assert!(
        !debug.contains("key-sentinel"),
        "Debug output contains the key"
    );
    assert!(
        !debug.contains("secret-sentinel"),
        "Debug output contains the secret"
    );
}

const CONFIGURATION_DOC: &str = include_str!("../../docs/configuration.md");
const MATRIX_HEADER: &str = "| Setting | Flag | Env var | Default | Scope |";

/// One flag as one scope sees it, spelled the way the matrix spells it.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Setting {
    scope: String,
    flag: String,
    env: String,
    default: String,
}

/// Every flag clap defines, plus the leaf command paths that `every
/// command` stands for. The command is walked unbuilt, where globals
/// still live only on the root and the generated help and version flags
/// do not exist yet.
fn defined_settings() -> (Vec<Setting>, Vec<String>) {
    fn walk(
        command: &clap::Command,
        path: &str,
        settings: &mut Vec<Setting>,
        leaves: &mut Vec<String>,
    ) {
        for arg in command.get_arguments() {
            let id = arg.get_id().as_str();
            if id == "help" || id == "version" {
                continue;
            }
            let long = arg
                .get_long()
                .unwrap_or_else(|| panic!("argument {id} has no long flag"));
            let defaults: Vec<_> = arg
                .get_default_values()
                .iter()
                .map(|value| value.to_string_lossy())
                .collect();
            let default = if !defaults.is_empty() {
                defaults.join(",")
            } else if arg.is_required_set() {
                "required".to_string()
            } else {
                "none".to_string()
            };
            settings.push(Setting {
                scope: if arg.is_global_set() {
                    "global".to_string()
                } else {
                    path.to_string()
                },
                flag: format!("--{long}"),
                env: arg.get_env().map_or_else(
                    || "—".to_string(),
                    |name| name.to_string_lossy().into_owned(),
                ),
                default,
            });
        }
        let mut subcommands = command.get_subcommands().peekable();
        if subcommands.peek().is_none() && !path.is_empty() {
            leaves.push(path.to_string());
        }
        for subcommand in subcommands {
            let child = if path.is_empty() {
                subcommand.get_name().to_string()
            } else {
                format!("{path} {}", subcommand.get_name())
            };
            walk(subcommand, &child, settings, leaves);
        }
    }

    let (mut settings, mut leaves) = (Vec::new(), Vec::new());
    walk(&Cli::command(), "", &mut settings, &mut leaves);
    (settings, leaves)
}

/// The matrix rows, one entry per scope a row names. A row that breaks
/// the cell format documented above the table fails here, quoted.
fn documented_settings(leaves: &[String]) -> Vec<Setting> {
    // A cell holding exactly one backticked span.
    fn code(cell: &str) -> Option<&str> {
        let inner = cell.strip_prefix('`')?.strip_suffix('`')?;
        (!inner.contains('`')).then_some(inner)
    }

    let mut lines = CONFIGURATION_DOC
        .lines()
        .skip_while(|line| line.trim_end() != MATRIX_HEADER);
    assert!(
        lines.next().is_some(),
        "docs/configuration.md has no matrix header {MATRIX_HEADER:?}"
    );

    let mut settings = Vec::new();
    for row in lines.take_while(|line| line.starts_with('|')) {
        let cells: Vec<&str> = row
            .trim_end()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        let is_separator = cells
            .iter()
            .all(|cell| !cell.is_empty() && cell.chars().all(|c| matches!(c, '-' | ':')));
        if is_separator {
            continue;
        }
        let [_, flag, env, default, scope] = cells[..] else {
            panic!("matrix row needs five cells: {row}");
        };

        let flag = code(flag)
            .and_then(|flag| flag.split(' ').next())
            .filter(|name| name.starts_with("--"))
            .unwrap_or_else(|| panic!("flag cell is not one backticked --flag: {row}"));
        let env = match env {
            "—" => "—",
            env => {
                code(env).unwrap_or_else(|| panic!("env var cell is not backticked or —: {row}"))
            }
        };
        let default = match default {
            "required" | "none" => default,
            default => code(default).unwrap_or_else(|| {
                panic!("default cell is not backticked, required, or none: {row}")
            }),
        };
        let scopes: Vec<&str> = match scope {
            "global" => vec!["global"],
            "every command" => leaves.iter().map(String::as_str).collect(),
            scope => scope
                .split(',')
                .map(|path| {
                    code(path.trim()).unwrap_or_else(|| {
                        panic!("scope is not global, every command, or backticked paths: {row}")
                    })
                })
                .collect(),
        };

        settings.extend(scopes.into_iter().map(|scope| Setting {
            scope: scope.to_string(),
            flag: flag.to_string(),
            env: env.to_string(),
            default: default.to_string(),
        }));
    }
    settings
}

// docs/configuration.md is the operator-facing contract for every flag;
// this keeps it from drifting the way --help cannot.
#[test]
fn configuration_doc_matches_every_flag_env_var_default_and_scope() {
    let (defined, leaves) = defined_settings();
    let documented = documented_settings(&leaves);

    let mut listed = BTreeSet::new();
    for setting in &documented {
        assert!(
            listed.insert(setting),
            "docs/configuration.md lists {setting:?} twice"
        );
    }
    let defined: BTreeSet<&Setting> = defined.iter().collect();
    let missing: Vec<_> = defined.difference(&listed).collect();
    let stale: Vec<_> = listed.difference(&defined).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "docs/configuration.md is out of date with the CLI definition\n\
         missing from the doc: {missing:#?}\n\
         stale in the doc: {stale:#?}"
    );
}

const README: &str = include_str!("../../README.md");

/// The logical lines of the README's bash blocks, `\` continuations
/// joined.
fn readme_command_lines() -> Vec<String> {
    let (mut lines, mut pending, mut in_bash) = (Vec::new(), String::new(), false);
    for line in README.lines().map(str::trim_end) {
        if !in_bash {
            in_bash = line == "```bash";
            continue;
        }
        if line == "```" {
            in_bash = false;
            continue;
        }
        match line.strip_suffix('\\') {
            Some(head) => {
                pending.push_str(head.trim());
                pending.push(' ');
            }
            None => {
                pending.push_str(line.trim());
                let logical = std::mem::take(&mut pending);
                if !logical.is_empty() {
                    lines.push(logical);
                }
            }
        }
    }
    lines
}

/// Shell words with quotes grouped and removed, and nothing expanded:
/// `"$(cat key)"` stays one literal word, as clap would receive a value.
fn shell_words(line: &str) -> Vec<String> {
    let (mut words, mut word, mut in_word) = (Vec::new(), String::new(), false);
    let mut quote = None;
    for c in line.chars() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => word.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            None => {
                word.push(c);
                in_word = true;
            }
        }
    }
    assert!(quote.is_none(), "unterminated quote in README line: {line}");
    if in_word {
        words.push(word);
    }
    words
}

// The README is the crates.io front page and the crate docs; a renamed
// flag or variable must fail here instead of in a reader's terminal.
#[test]
fn every_readme_command_parses_and_exports_only_known_variables() {
    let known_vars: BTreeSet<String> = defined_settings()
        .0
        .into_iter()
        .map(|setting| setting.env)
        .filter(|env| env != "—")
        .collect();

    let mut commands = 0;
    for line in readme_command_lines() {
        let words = shell_words(&line);
        match words.iter().map(String::as_str).collect::<Vec<_>>()[..] {
            ["cargo", "run", "--", ref args @ ..] => {
                commands += 1;
                let argv = std::iter::once("quantforge").chain(args.iter().copied());
                if let Err(err) = Cli::try_parse_from(argv) {
                    panic!("README command does not parse: {line}\n{err}");
                }
            }
            ["export", assignment] => {
                let name = assignment
                    .split_once('=')
                    .map_or(assignment, |(name, _)| name);
                assert!(
                    known_vars.contains(name),
                    "README exports {name}, which no flag reads: {line}"
                );
            }
            _ => {}
        }
    }
    assert_eq!(
        commands,
        README.matches("cargo run --").count(),
        "a `cargo run --` command in the README sits outside a bash block"
    );
}
