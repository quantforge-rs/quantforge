# Release Guide

## Pre-flight

- replace repository placeholders in `Cargo.toml`
- verify the package name is available on crates.io
- update `CHANGELOG.md`
- ensure CI is green

## Local release check

```bash
./scripts/release-check.sh
cargo package --list
cargo publish --dry-run --locked
```

## Publish

Either authenticate locally with `cargo login`, or set `CARGO_REGISTRY_TOKEN`.

```bash
cargo publish --locked
```

This publishes the `quantforge` crate alone. `quantforge-py`, the Python
package's crate, is a workspace member that cargo refuses to publish; its
wheels go to PyPI, which this guide does not cover yet
([docs/python-package.md](docs/python-package.md)). CI's `wheel` legs build
the wheel of each platform as a release would publish it, test it, and keep
each that passed with the run ([docs/ci.md](docs/ci.md#the-wheels)).

## Smoke test

After publishing:

```bash
cargo install quantforge --locked
quantforge --help
```
