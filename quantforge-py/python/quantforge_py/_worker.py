"""The worker a QuantForge server runs Python bundles in, one job each:
`python -m quantforge_py._worker`, which `quantforge serve run --job-python`
starts with an environment of its own and hands one job over standard input
(docs/python-worker.md). Never run by hand. Private."""

import os
import sys


def shed():
    """Closes every descriptor this process was given but its three pipes,
    before anything else runs: one the server held without close-on-exec as
    it started the worker, or that another of its threads was opening then,
    would reach the job past the sandbox."""
    if os.name != "posix":
        return
    listed = "/proc/self/fd" if sys.platform.startswith("linux") else "/dev/fd"
    for name in os.listdir(listed):
        descriptor = int(name)
        if descriptor > 2:
            try:
                os.close(descriptor)
            except OSError:
                # The listing's own, which closed as the listing ended.
                pass


def main():
    shed()
    # The frames go out on a descriptor of their own. What anything else
    # writes to standard output, a process the job starts or a library's
    # own code, goes where standard error goes, and never into the frames.
    frames = os.dup(1)
    os.dup2(2, 1)
    from quantforge_py import _native

    _native._run_worker(frames)


if __name__ == "__main__":
    main()
