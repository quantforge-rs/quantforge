# Worker isolation

`serve run` runs every backtest job in a worker process of its own: a
process of the same binary, started as `quantforge worker`, that runs the
one job it is handed and is then killed. This page says what a worker gets,
what limits it runs within, what each operating system enforces of them,
and what nothing enforces yet. The code is `src/server/workers/`.

## One process per job

The server keeps as many workers as it runs jobs at once
(`--job-concurrency`), each started ahead of its job so that a job seldom
waits for one. When a job starts, the server writes the job's bundle into
the worker's scratch directory, sends the job and its candles over the
worker's standard input, and reads how the job ended from its standard
output. Once the job has ended, however it ended, the server kills the
worker and starts another in its place. No worker runs a second job, so
nothing one job leaves in memory, in its scratch directory or in its
process ever reaches another.

A worker that crashes, or is killed by anyone, fails its own job and no
other; the server keeps running and starts a fresh worker for the next job.
A cancelled job's worker is killed at once. A worker whose server goes away
exits as soon as it notices that its standard input has ended.

## What a worker gets

Only what its job needs:

- **Its job**: the submission, the bundle's entry and parameters, and the
  candles, which the server read for it from the store, sent as frames over
  its standard input. The bundle's files are in `bundle/` in its scratch
  directory.
- **A scratch directory of its own**, its working directory and its
  temporary directory, readable by its owner only. The server makes one
  directory, `quantforge-jobs-<random>`, in the system's temporary directory
  (`TMPDIR`; `TMP` or `TEMP` on Windows) as it starts, a directory in it for
  each worker, and removes each once its worker has stopped, and its own
  when the server stops. A server that is killed leaves its directory
  behind.
- **An environment with nothing of the server's in it**: only `TMPDIR`
  (`TMP` and `TEMP` on Windows), naming its scratch directory. On macOS,
  CoreFoundation, which a build with PostgreSQL support links, sets one
  more as the worker starts, holding the user's text encoding.
- **Three pipes**: its standard input, output and error, and no other
  descriptor. Every file and socket the server opens, its SQLite database
  and its PostgreSQL connections included, is closed on exec. On Linux and
  macOS a worker also closes, before it reads or runs anything, any other
  descriptor it finds open: one its server was given without close-on-exec,
  or one another of the server's threads was still opening as the worker
  started. It marks them to close on exec and runs itself again in place.

A worker never sees the store, its URL or its credentials. The server reads
everything a job runs on and writes how the job ended; a worker only answers
it.

## Limits

Each job runs within three limits, all flags of `serve run` with an
environment fallback:

| Limit | Flag | Default | How |
| --- | --- | --- | --- |
| CPU time | `--job-cpu-secs` | 600 s | The worker sets `RLIMIT_CPU` on itself as it starts. Past it, the kernel stops it with `SIGXCPU`, and with `SIGKILL` a second later if the job catches that. |
| Memory | `--job-memory-mib` | 4096 MiB | The worker sets `RLIMIT_AS` on itself as it starts: an allocation past it fails, and the worker aborts. This limits address space, which some libraries reserve well beyond what they use. |
| Wall-clock time | `--job-wall-secs` | 1800 s | The server kills the worker once the job has run that long. |

A job that runs into a limit fails as `invalid_request`, and its message
names the limit and its value, for example `the job used the 600 s of CPU
time it may use, and was stopped`. `serve run` prints the limits it runs
with as `job_limits: cpu 600 s, memory 4096 MiB, wall clock 1800 s`.

## What each operating system enforces

A worker puts its limits and sandbox on itself as it starts, before it reads
a job and before it has a second thread, and reports what held. `serve run`
prints it as `worker_isolation:`, with `none` for a guard that does not
hold, and logs why:

| Guard | Linux | macOS | Windows |
| --- | --- | --- | --- |
| CPU time | `rlimit` | `rlimit` | none |
| Memory | `rlimit` | none | none |
| Network | `seccomp` | none | none |
| Files | `landlock`, where the kernel has it | none | none |
| Wall-clock time | the server kills the worker | the server kills the worker | the server kills the worker |

On Linux, `worker_isolation: cpu rlimit, memory rlimit, network seccomp,
files landlock`. In detail:

- **Network** (seccomp): a worker cannot create a socket of any kind, so it
  reaches no network and no local service, a PostgreSQL server listening on
  a Unix socket included. It cannot use io_uring, which can open a socket
  without the `socket` call, or `ptrace` and `process_vm_readv`/`writev`,
  which read and write other processes' memory. Nor can it call `setsid` or
  `setpgid`, so a process a job starts never leaves the worker's process
  group, and goes when the server kills that group. A refused call fails
  with `EACCES`.
- **Files** (Landlock, Linux 5.13 and later, where the kernel enables it):
  a worker can open, create, change or run nothing outside its scratch
  directory: not another job's scratch directory, not the server's database
  file, not `/etc`. On Linux 6.7 and later Landlock also refuses its TCP
  connections and binds, and on 6.12 and later it keeps its signals and
  abstract Unix sockets inside the sandbox, so a job cannot signal the
  server. A kernel without Landlock enforces no file guard, and
  `worker_isolation:` says `files none`.
- **Other processes**: the server and its workers are not dumpable, so no
  other process of their user can attach to them or read their memory or
  environment under `/proc`, the server's database URL included.

macOS enforces a worker's CPU time; it has no address-space limit, and the
process profile has no network or file guard there. Windows enforces
neither limit nor guard; the server still kills a worker past its
wall-clock time.

## How a job ends

| What happened | State | Code | Message |
| --- | --- | --- | --- |
| The job finished | `succeeded` or `failed` | as the job said | as the job said |
| Its CPU time ran out | `failed` | `invalid_request` | `the job used the N s of CPU time it may use, and was stopped` |
| It needed more memory than it may use | `failed` | `invalid_request` | `the job needed more than the N MiB of memory it may use, and was stopped` |
| Its wall-clock time ran out | `failed` | `invalid_request` | `the job ran for the N s of wall-clock time it may run, and was stopped` |
| Its worker crashed or was killed | `failed` | `internal` | `the job's worker stopped before the job finished:` and how, such as `it was stopped by signal 11 (SIGSEGV)` |
| It was cancelled | `cancelled` | | |
| The server stopped while it ran | `failed` | `internal` | `the server stopped while the job ran; submit it again` |

## Not covered yet

The process profile is the default, and it is not airtight. What it leaves
open:

- A worker runs as the server's user. Run `serve run` as an ordinary user:
  a worker running as root can raise its own limits, and do much that no
  guard here refuses.
- On macOS and Windows a job can reach the network and any file its user
  can, and on Windows any process its user runs, the server included. A
  Linux kernel without Landlock leaves files open too.
- Before Linux 6.12 a job can signal any process of its user, and so stop
  the server.
- A job can fill its scratch directory's disk: nothing limits how much it
  writes.
- On macOS a process a job starts can outlive its worker by leaving the
  worker's process group, which seccomp refuses on Linux; on Windows every
  process a job starts outlives its worker.
- A job that catches `SIGXCPU` is killed a second later and reported as
  stopped by `SIGKILL`, not as out of CPU time.

A container profile for hosted servers, which closes these, and the threat
model of executing users' bundles, come later.

## The frames

A server and its workers speak in frames of a four-byte big-endian length
and that many bytes of JSON. They are no part of the protocol: a server only
ever speaks to workers of its own build, and the first frame from each side
names its build, so a worker refuses a server of another. A server sends
`setup`, and the worker answers `ready` with what it enforced; the job
follows as `job` and its `candles`, and the worker answers `finished` once.

## Testing it

The tests in `tests/workers/` start this build's binary as a worker in a
synthetic mode, whose workloads stand in for a strategy: they spin, take
memory, sleep, crash, leave state behind, and try to read files, open
connections and leave their process group, so that each limit and guard is
run into before any user code runs in a worker. `serve run` never starts
its workers in that mode.
