# Worker isolation

`serve run` runs every backtest job in a worker process of its own, that
runs the one job it is handed and is then killed: the Python interpreter
`--job-python` names, running the worker of the `quantforge_py` installed
beside it, which runs the job's strategy bundle
([python-worker.md](python-worker.md)); or, without one, a process of the
same binary, started as `quantforge worker`, which runs no bundle and fails
each job as it starts. Both are the same worker, compiled into the binary
and into the package: the same frames, limits and sandbox. This page says
what a worker gets, what limits it runs within, what each operating system
enforces of them, and what nothing enforces yet. The code is
`src/server/workers/`. Who a worker is defended against, layer by layer,
and what the tests show of it, is the
[threat model](security/remote-execution.md).

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
  behind. The bundle is written into `bundle/` as the job starts, every
  directory and file of it made new: anything standing where it goes, a
  link above all, fails the job, and nothing is written through it.
- **An environment with nothing of the server's in it**: only `TMPDIR`
  (`TMP` and `TEMP` on Windows), naming its scratch directory. On macOS,
  CoreFoundation, which a build with PostgreSQL support links, sets one
  more as the worker starts, holding the user's text encoding. A Python
  worker also gets the variables that make a job run alike on every
  machine, and on Windows `SYSTEMROOT`
  ([python-worker.md](python-worker.md#what-a-worker-keeps-from-its-server)).
- **Three pipes**: its standard input, output and error, and no other
  descriptor. Every file and socket the server opens, its SQLite database
  and its PostgreSQL connections included, is closed on exec. On Linux and
  macOS a worker also closes, before it reads or runs anything, any other
  descriptor it finds open: one its server was given without close-on-exec,
  or one another of the server's threads was still opening as the worker
  started. It marks them to close on exec and runs itself again in place;
  a Python worker closes them before anything else runs.

A worker never sees the store, its URL or its credentials. The server reads
everything a job runs on and writes how the job ended; a worker only answers
it.

## Limits

Each job runs within three limits, all flags of `serve run` with an
environment fallback:

| Limit | Flag | Default | How |
| --- | --- | --- | --- |
| CPU time | `--job-cpu-secs` | 600 s | The worker sets `RLIMIT_CPU` on itself as it starts. Past it, the kernel stops it with `SIGXCPU`, and on Linux with `SIGKILL` a second later if the job catches or ignores that. |
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
| CPU time | `rlimit` | `rlimit`, which a job can ignore | none |
| Memory | `rlimit` | none | none |
| Network | `seccomp` | none | none |
| Processes and signals | `seccomp`, with the network guard | none | none |
| Files | `landlock`, where the kernel has it | none | none |
| Wall-clock time | the server kills the worker | the server kills the worker | the server kills the worker |

On Linux, `worker_isolation: cpu rlimit, memory rlimit, network seccomp,
files landlock`. In detail:

- **Network** (seccomp): a worker cannot create a socket of any kind, so it
  reaches no network and no local service, a PostgreSQL server listening on
  a Unix socket included. Nor a pair of sockets of its own (`socketpair`):
  one of a pair of datagram sockets can be sent to the address of any Unix
  socket, the system log's among them. It cannot use io_uring, which can
  open a socket without the `socket` call, or `ptrace` and
  `process_vm_readv`/`writev`, which read and write other processes'
  memory. A refused call fails with `EACCES`.
- **Processes and signals** (the same seccomp filter, so the `network`
  guard says whether they hold): a job starts no process. `fork`, `vfork`
  and a `clone` that starts no thread are refused, and `clone3`, whose
  flags a filter cannot read, is answered as a kernel without it answers,
  so that a C library starts its threads with `clone`. A job's CPU time and
  memory are limits on its worker's process, and every process a job
  started would have them over again: without one, the limits are the
  job's. Threads start as ever. Nor can a worker call `setsid` or
  `setpgid`, or send a signal or set a resource limit (`kill`, `tkill`,
  `tgkill`, `rt_sigqueueinfo`, `rt_tgsigqueueinfo`, `pidfd_send_signal`,
  `prlimit64`) for any process but itself, or name another process as the
  owner of a descriptor (`fcntl`'s `F_SETOWN` and `F_SETOWN_EX`), whom the
  kernel signals as the descriptor has something to read: a job stops no
  other process of its user, its server least of all, with a signal or by
  taking its CPU time away.
- **Files** (Landlock, Linux 5.13 and later, where the kernel enables it):
  a worker can open, create, change or run nothing outside its scratch
  directory: not another job's scratch directory, not the server's database
  file, not `/etc`. A Python worker may also read, and never change, the
  files of its interpreter, its environment and its import path, and the
  system's libraries, which a module its job imports may still load. On Linux 6.7 and later Landlock also refuses its TCP
  connections and binds, and on 6.12 and later it keeps its signals and
  abstract Unix sockets inside the sandbox, each of which the seccomp
  filter refuses already. Before Linux 6.2 Landlock does not cover
  truncation: a job can empty, by its path, a file its user may write. A
  kernel without Landlock enforces no file guard, and `worker_isolation:`
  says `files none`.
- **Other processes**: the server and its workers are not dumpable, so no
  other process of their user can attach to them or read their memory or
  environment under `/proc`, the server's database URL included.

macOS signals a worker once when its CPU time is up, which stops a job that
does nothing about it; a job that ignores the signal runs on to its
wall-clock time. It has no address-space limit, and the process profile has
no network, process or file guard there. Windows enforces neither limit nor
guard; the server still kills a worker past its wall-clock time.

## How a job ends

| What happened | State | Code | Message |
| --- | --- | --- | --- |
| The job finished | `succeeded` or `failed` | as the job said | as the job said |
| Its CPU time ran out | `failed` | `invalid_request` | `the job used the N s of CPU time it may use, and was stopped` |
| It needed more memory than it may use | `failed` | `invalid_request` | `the job needed more than the N MiB of memory it may use, and was stopped` |
| Its wall-clock time ran out | `failed` | `invalid_request` | `the job ran for the N s of wall-clock time it may run, and was stopped` |
| Its worker crashed or was killed | `failed` | `internal` | `the job's worker stopped before the job finished:` and how, such as `it was stopped by signal 11 (SIGSEGV)` |
| Its worker sent what no worker sends | `failed` | `internal` | `the job's worker sent the server something it cannot read` |
| It was cancelled | `cancelled` | | |
| The server stopped while it ran | `failed` | `internal` | `the server stopped while the job ran; submit it again` |

## Not covered yet

The process profile is the default, and it is not airtight. What it leaves
open:

- A worker runs as the server's user. Run `serve run` as an ordinary user
  that owns nothing else: a worker running as root can raise its own
  limits, and do much that no guard here refuses.
- On macOS and Windows a job can reach the network, any file its user can,
  and the processes its user runs: on macOS it reads their environments,
  the server's among them, and on Windows the server's memory. A Linux
  kernel without Landlock leaves files open too, and one before 6.2 lets a
  job empty a file its user may write.
- A job can fill its scratch directory's disk: nothing limits how much it
  writes.
- The seccomp filter names the calls it refuses, and every other call
  reaches the kernel: a job can still slow the other processes of its user
  down, and a flaw in the kernel is a way out.
- On macOS, and on Linux where seccomp does not hold (`network none`), a
  job starts processes, each with the limits over again, and one of them
  can outlive its worker by leaving the worker's process group; on Windows
  every process a job starts outlives its worker.
- A job that catches or ignores `SIGXCPU` is killed a second later on
  Linux and reported as stopped by `SIGKILL`, not as out of CPU time; on
  macOS it runs on to its wall-clock time.

The [threat model](security/remote-execution.md) says who each of these
matters to, and what to do about it until a container profile for hosted
servers closes them.

## The frames

A server and its workers speak in frames of a four-byte big-endian length
and that many bytes of JSON. They are no part of the protocol: a server only
ever speaks to workers of its own build, and the first frame from each side
names its build, so a worker refuses a server of another. A server sends
`setup`, naming what its workers run: no runtime, the tests' synthetic
workloads, or Python bundles with the packages a bundle may import. The
worker answers `ready` with what it enforced and the runtime it offers; the
job follows as `job`, with the bundle's entry, parameters and declared
packages, and its `candles`, and the worker answers `finished` once.

A backtest that ran to its end sends its closed trades and its equity curve
before `finished`, as `trades` and `equity` frames of rows, and its summary
in `finished`. A worker never sends a file: the server reads each row,
every decimal as its exact text, keeps no more rows than the job has bars,
refuses a trade of another symbol than the job's, a curve that does not
have one row per bar or a summary that miscounts the trades, and writes the
job's result files itself ([result-spec.md](result-spec.md)). So what a
client downloads is always exactly a result file, whatever the job's code
did to its worker.

A job's code runs in its worker's process, and can write the worker's
frames itself. So the server takes nothing a worker says on trust: a
failure keeps the first 8 KiB of its words, and says how much it left out;
a job fails as its request's fault, the worker's, or for want of a runtime,
and a frame that says otherwise ends the job; and a result of a worker's
own naming, which the tests' workloads report, is taken from no other
worker.

While a job runs its worker sends what the job says: a `log` frame for each
line it writes, with the line's level, and `progress` frames with how many
of its bars the job has run. A worker cuts a line longer than 64 KiB to its
start and says by how much. The server keeps each job's lines in a log of
its own, which holds the newest 10,000 lines, 1 MiB of text in all and
8 KiB of any one line, and counts what it leaves out; it holds the count of
bars a worker sends to the job's own; and it reads no more than eight frames
ahead of what it has taken, so a job that writes without end waits on its
own output and takes no more of the server's memory than a job that writes
nothing. A line at a level the server has no name for ends the job, as
every frame no worker of its build sends does. Clients follow the log while
the job runs, and the server stores it with the job once the job has ended:
the [job protocol](../src/protocol/jobs/mod.rs) says how they read it.

## Testing it

The tests in `tests/workers/` start this build's binary as a worker in a
synthetic mode, whose workloads stand in for a strategy: they spin, take
memory, sleep, crash, leave state behind, and try to read files, open
connections and leave their process group, so that each limit and guard is
run into before any user code runs in a worker; one runs the engine on a
strategy built into QuantForge, so that a job ends with a result as a real
one will; and one writes to its log, a few lines or many times what a log
keeps, as fast as its worker takes them; and two try what a job that means
harm would, every way out of the scratch directory and every reach for the
processes around it. `serve run` never starts its workers in that mode. The
Python package's tests run the Python worker through a real `serve run`
([python-worker.md](python-worker.md#testing-it)), bundles that mean harm
among them ([the threat model](security/remote-execution.md#what-the-tests-show)).

A test of a guard skips where the workers enforce no such guard, and says
so. Where `QUANTFORGE_TEST_SANDBOX_REQUIRED` is `1`, as on CI's Linux legs,
it fails instead, so that no run passes for a sandbox that was never on.
