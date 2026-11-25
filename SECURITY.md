# Security Policy

## Supported Versions

The latest published `0.x` release receives security fixes.

## Reporting a Vulnerability

Report a vulnerability privately, through GitHub's private vulnerability
reporting: on the repository's **Security** tab choose **Report a
vulnerability**, or open
<https://github.com/quantforge-rs/quantforge/security/advisories/new>. Only
the maintainers see what you send there.

Say what you did, what happened, and on what: the QuantForge version, the
operating system and its kernel, and, for anything about a job's worker, the
`worker_isolation:` line `serve run` printed.

Do not open a public issue for:

- credential leaks
- secret management flaws
- supply-chain security concerns that are not yet patched
- vulnerabilities that could put exchange credentials or user data at risk
- a way for a strategy bundle to leave its worker's sandbox

## What a Job's Sandbox Promises

`quantforge serve run --job-python` runs Python that clients upload. What
stands between that code and the machine is written down, layer by layer,
with what is tested and what nothing stops, in the
[threat model](docs/security/remote-execution.md). In short:

### The process profile

The default, and the only profile there is today: each job in a worker
process of its own, as the server's user. What it promises depends on what
`serve run` prints as it starts.

**On Linux, when the line reads `worker_isolation: cpu rlimit, memory
rlimit, network seccomp, files landlock`**, on a kernel of 6.2 or later, a
bundle cannot:

- write, create, remove or truncate a file outside its scratch directory,
  or read one, but for the files of the Python environment it runs in and
  the system's libraries;
- open a socket of any kind;
- start a process, or outlive its job;
- signal another process, or change another's resource limits;
- read the server's memory or environment, or anything of its store;
- use more CPU time, memory or wall-clock time than the server allows;
- make the server keep or serve anything but checked rows and bounded text,
  whatever it sends in its worker's place.

A way around any of these is a vulnerability. Please report it.

**Wherever a guard reads `none`**, on Linux without Landlock, and on macOS
and Windows, where the process profile has no sandbox, that guard promises
nothing: a bundle that acts as the server's user there is the documented
behaviour, not a vulnerability. Run only bundles you would run by hand.

**The process profile never promises** authentication of requests, a limit
on what a job writes to its scratch directory, protection from the other
processes of the server's user that no rule above names, or anything about
flaws in the kernel or side channels. A result is what its bundle's code
said: no profile makes it honest. The threat model lists each of these, and
what to do about it.

### The container profile

For hosted servers that run bundles from strangers, a container profile is
planned: each job's worker in a container with no network, a read-only
root, and limits on memory, CPU, processes and disk. It is not built yet,
and its promise will be stated here when it is. Until then, do not run
strangers' bundles on a machine that holds anything else.

### Whatever the profile

These are vulnerabilities on every system:

- a bundle the [bundle spec](docs/bundle-spec.md) refuses that a server
  accepts, or a file of a bundle written outside the scratch directory it
  belongs in;
- anything of the store, its URL or its credentials handed to a worker;
- a request that makes the server read or write a file it should not, or
  that crashes it;
- a secret printed or logged.

## Automated Security Checks

Three layers of automated scanning run on this repository:

- **CodeQL** static analysis runs on pull requests targeting `main`, on pushes to `main`, and weekly; alerts appear in the repository Security tab under Code scanning
- **gitleaks** scans the full git history for committed secrets on pull requests targeting `main`, on pushes to `main`, and weekly
- **GitHub secret scanning with push protection** runs server-side and blocks pushes containing known credential patterns

If a credential does leak, rotate it first — rewriting git history does not revoke a secret. Then report the incident privately as described above.
