"""The package as Python sees it: what it is called, what it publishes, and
what it says about the build it is."""

import pathlib
import re
import sys

import quantforge_py
from quantforge_py import _native

# MAJOR.MINOR.PATCH without leading zeros: an engine version as a strategy
# bundle's `engine` requirement names one (docs/bundle-spec.md).
ENGINE_VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")


def test_the_package_publishes_its_names_and_holds_no_other_public_one():
    assert quantforge_py.__all__ == ["__version__", "ENGINE_VERSION", "STORAGE_BACKENDS"]
    public = sorted(name for name in vars(quantforge_py) if not name.startswith("_"))
    assert public == ["ENGINE_VERSION", "STORAGE_BACKENDS"]


def test_every_published_name_is_the_extension_modules_own():
    for name in quantforge_py.__all__:
        assert getattr(quantforge_py, name) is getattr(_native, name), name


def test_the_extension_module_is_private_and_built_for_the_stable_abi():
    assert _native.__name__ == "quantforge_py._native"
    extension = pathlib.Path(_native.__file__)
    # Windows has no stable ABI tag for an extension's file name.
    assert extension.name == ("_native.pyd" if sys.platform == "win32" else "_native.abi3.so")
    assert extension.parent == pathlib.Path(quantforge_py.__file__).parent


def test_the_version_is_the_crates_own(crate):
    assert quantforge_py.__version__ == crate["package"]["version"]


def test_the_engine_version_is_the_quantforge_crates(core_crate):
    assert quantforge_py.ENGINE_VERSION == core_crate["package"]["version"]
    assert ENGINE_VERSION.fullmatch(quantforge_py.ENGINE_VERSION)


def test_the_storage_backends_are_the_ones_the_build_holds(storage_backends):
    assert type(quantforge_py.STORAGE_BACKENDS) is tuple
    assert quantforge_py.STORAGE_BACKENDS == storage_backends
