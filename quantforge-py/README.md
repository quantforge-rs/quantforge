# quantforge-py

[QuantForge](https://github.com/quantforge-rs/quantforge) for Python:
strategies written in Python, run on the same deterministic Rust engine as
the `quantforge` command line.

**Not released yet.** The package is being built in the open, and so far it
only says what it is:

```python
import quantforge_py as qf

qf.__version__       # the package's own version
qf.ENGINE_VERSION    # the engine it was built with
qf.STORAGE_BACKENDS  # ("sqlite", "postgres")
```

The package installs as `quantforge-py` and imports as `quantforge_py`.
`quantforge` on PyPI is another project.

One wheel per platform, built against Python's stable ABI, loads on CPython
3.11 and every later version.

[The Python package](https://github.com/quantforge-rs/quantforge/blob/main/docs/python-package.md)
says how it is named, built and tested.
