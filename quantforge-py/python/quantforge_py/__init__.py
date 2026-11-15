"""QuantForge for Python: strategies written in Python, run on the same
deterministic Rust engine as the ``quantforge`` command line.

So far the package only says what it is:

- ``__version__``: the package's own version.
- ``ENGINE_VERSION``: the version of the engine it was built with, which a
  strategy bundle's ``engine`` requirement is held to.
- ``STORAGE_BACKENDS``: the stores this build can open, ``"sqlite"`` and,
  unless it is the SQLite-only build, ``"postgres"``.

The names come from the compiled extension module, ``quantforge_py._native``,
which is private: import them from here.
"""

from ._native import ENGINE_VERSION, STORAGE_BACKENDS, __version__

__all__ = ["__version__", "ENGINE_VERSION", "STORAGE_BACKENDS"]
