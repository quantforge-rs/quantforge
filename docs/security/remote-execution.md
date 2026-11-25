# Running uploaded code: the threat model

`quantforge serve run --job-python <path>` runs Python that a client
uploads. That is remote code execution, on purpose: a strategy is a
program, and running it is the feature. This page says what stands between
that program and the machine it runs on, who can attack what, what each
layer stops, which test shows it, and what nothing stops.

It describes the **process profile**, the only one there is today: each job
in a worker process of its own, on the server's machine, as the server's
user ([worker-isolation.md](../worker-isolation.md)). A container profile
for servers that run strangers' bundles comes later, and
[the last layer](#layer-3-the-container-profile-not-built-yet) says what it
is for.

## The short answer

- **On Linux 6.2 or later, with every guard held**, a bundle is confined:
  it changes no file outside its scratch directory, and reads none but
  those of the Python environment it runs in and the system's libraries;
  it reaches no network and no other process, and starts none of its own;
  its CPU time, memory and wall clock are limited. An older kernel can
  print the same line below and still let a job empty a file outside
  ([what Landlock covers, by kernel](#what-landlock-covers-by-kernel)).
  [What that still leaves open](#what-nothing-stops) is on this page, and
  it is not nothing: the disk, the kernel itself, and a few ways to slow
  its neighbours down.
- **On macOS and Windows there is no sandbox.** A bundle runs as the
  server's user and can do what that user can. Run only bundles you would
  run by hand.
- **No request is authenticated yet.** Whoever can reach the listener can
  upload a bundle and run it. `serve run` listens on this machine alone
  unless told otherwise; leave it there.

`serve run` prints what its workers enforce before it listens, with `none`
for a guard that does not hold:

```text
worker_isolation: cpu rlimit, memory rlimit, network seccomp, files landlock
```

Anything but that line, or a kernel before 6.2 under it, means that part
of this page's Linux column does not apply to the server in front of you.

## What is protected

| Asset | What is at stake |
| --- | --- |
| The machine and the server's user | Every file that user reads or writes, and every process it runs |
| The store | The candles, every bundle, job, result, log and provenance record, and the store's own credentials: a PostgreSQL URL with its password, or the SQLite file |
| What the server's process holds | Its environment and command line, and whatever was exported in the shell that started it, an exchange's API key among it; `serve run` needs none |
| Other jobs | Their bundles, the bars they run on, what they write, and their results |
| The server's capacity | Its CPU time, memory, disk, process table and job slots |
| A result's meaning | That a job's files are what its bundle produced from the bars its provenance names |
| The machine's place on its network | What it can reach that the wider network cannot, and a way to send anything out |
| A client's terminal | What a job's log and its failure draw on the screen of whoever reads them |

## Who is trusted

```text
 client ──HTTP──▶ server ──frames over pipes──▶ worker ──▶ the bundle's code
                    │                             │
                    ▼                             ▼
                  store                     scratch directory
```

- **The operator, the machine's kernel and the store** are trusted. Whoever
  can write the store's tables can change any record in them.
- **The environment `--job-python` names** is trusted: its interpreter,
  `quantforge-py`, and every package installed beside them. NumPy is
  imported in every worker before the sandbox is on. What is installed
  there is the operator's to choose and to keep patched.
- **A client is not trusted with what it sends**: every body is read
  strictly and within a size. But a client is not told apart from the
  operator either: there is no authentication, so every client may do
  everything a route offers.
- **A worker is not trusted.** It gets its one job and nothing else, and
  the server checks every frame it sends.
- **A bundle is not trusted, and there is no line between a bundle and its
  worker.** The bundle's code runs in the worker's own process: it holds
  the descriptor the worker's frames go out on, and can write to it. The
  rules on what a bundle may import ([python-worker.md](../python-worker.md))
  keep honest code honest and are no boundary. Everything below treats a
  running bundle as a compromised worker.

## Who attacks

**A malicious bundle** is arbitrary Python in a worker. It wants out: to
read or change files, reach the network or other processes, read what the
server holds, outlive its job, take the machine's resources, damage other
jobs, say that it produced a result it did not, or draw on the screen of
whoever reads its log.

**A malicious client** is anyone who can send the listener a request. It
can use every route: upload bundles, submit, cancel and rerun jobs, and
read every job's status, log, result and provenance. Running code is what
the routes are for, so a malicious client is a malicious bundle too, and
has besides the requests themselves: bodies made to break a parser,
archives made to land outside the place they are written to, paths made to
name a file, and volume.

**A compromised worker** is a worker that says what its server did not
expect: frames no worker sends, more of them than any job has, or none at
all. Since a bundle can speak in its worker's place, this is the same
attacker as the first, seen from the server's side of the pipe.

## Layer 1: what is checked before anything runs

A bundle is checked as it is uploaded, from the bytes received, and again
as a job of it is submitted, as the job starts, and before a rerun, each
time from the bytes the store holds: [bundle-spec.md](../bundle-spec.md) is
the contract, and `quantforge::protocol::bundle` the one implementation
every one of those uses. Each check reports the problem codes beside it:

<!-- begin: submission checks -->
| Check | What it refuses | Problem codes |
| --- | --- | --- |
| The size of the whole | An upload larger than any bundle within the limits, before it is read; and files over 8 MiB together | `bundle_too_large` |
| The archive | Anything that is not exactly the canonical tar of its files: an entry that is no regular file (a link of either kind, a device, a directory, a header of another tar dialect), a field that is not the canonical one, entries out of order or twice, bytes after the end, an archive cut short | `archive_not_canonical` |
| Each path | A path that is absolute, climbs with `..`, uses `\`, is not ASCII, holds a character outside letters, digits, `.`, `_` and `-`, is hidden, is a Windows device name, or is longer than 100 bytes; and two paths one filesystem could not hold side by side | `path_invalid`, `path_collision` |
| What a directory holds, as a client collects one | A symlink, a junction, a socket, a pipe: named and never opened | `not_a_regular_file` |
| Each file's kind | Any extension but `.py`, `.toml`, `.json` and `.txt` | `extension_not_allowed` |
| The size of each part | A file over 1 MiB, and more than 256 files | `file_too_large`, `too_many_files` |
| The manifest | No manifest; one over 64 KiB, not TOML 1.0, nested deeper than 32 levels, or with a key the format does not define; a format other than 1 | `manifest_missing`, `manifest_invalid`, `format_unsupported` |
| The manifest's keys | An entry that names no importable module and class, or no file of the bundle; an engine requirement or a Python version that is not one; parameters JSON cannot hold; a dependency that is not a normalized name pinned to one version | `entry_invalid`, `entry_not_found`, `engine_invalid`, `python_invalid`, `param_not_json`, `dependency_invalid` |
| Admission | A bundle written for another engine or Python than the server's, or one that names a package the server does not offer, or another version of one | `engine_mismatch`, `python_mismatch`, `dependency_not_allowed`, `dependency_version_unavailable` |
<!-- end: submission checks -->

Three more checks are the server's own, around those:

- **An upload is named by the SHA-256 of the bytes received**, and stored
  only under that name. As a job starts, the stored bytes are hashed again:
  a job runs the code its hash names, or none.
- **Nothing is ever unpacked.** No route decodes a compressed body, and a
  bundle's tar holds its files as they are. An archive that claims more
  than it holds is refused by the bytes it has; nothing is sized by what a
  header says.
- **No route reads a file.** A bundle, a segment and a result are rows of
  the store, named by a hash or a job's ID, so a path that climbs names no
  route and no file.

**What this layer does not do: read the code.** Nothing here looks at what
a strategy's Python does. A bundle that passes every check can still try
anything Python can, which is what the next layer is for. The checks keep a
bundle's *files* harmless: they land where they are written, as plain
files, within a size.

## Layer 2: the worker's process

What a worker is given, and what it is held to, whatever its bundle does:

- **A process of its own for one job**, started before the job and killed
  after it, with every process still in its group. Nothing one job leaves
  in memory or on disk reaches another.
- **Nothing of the server's**: an environment of its own, no descriptor but
  its three pipes, and no part of the store. The server reads every bar a
  job runs on and writes how it ended.
- **A scratch directory of its own**, readable by its owner only, removed
  with the worker. The bundle is written into it file by file, each
  directory and file made new: a link standing where the bundle goes fails
  the write, and nothing of a bundle is written through it.
- **Limits**: CPU time, memory and wall-clock time, of which the last is
  the server's own and holds on every system.
- **Nothing a worker says is trusted.** The server:
  - reads no more than eight frames ahead of what it has taken, none over
    16 MiB;
  - keeps a job's newest log lines within 1 MiB, each within 8 KiB, and
    counts what it leaves out;
  - takes rows, never files, and writes the result files itself: no more
    trades or equity rows than the job has bars, every trade of the job's
    own symbol, one equity row per bar, as many trades as the summary
    counts;
  - keeps the first 8 KiB of a failure's words, and takes only the three
    codes a job fails as;
  - ends the job at a frame no worker of its build sends for such a job: a
    result of the worker's own naming among them, which only the tests'
    workloads report.

What the worker puts on itself as it starts, before any code of a job
runs, depends on the operating system. The table is
[worker-isolation.md](../worker-isolation.md#what-each-operating-system-enforces)'s,
with the row this story added:

<!-- begin: guards -->
| Guard | Linux | macOS | Windows |
| --- | --- | --- | --- |
| CPU time | `rlimit` | `rlimit`, which a job can ignore | none |
| Memory | `rlimit` | none | none |
| Network | `seccomp` | none | none |
| Processes and signals | `seccomp`, with the network guard | none | none |
| Files | `landlock`, where the kernel has it | none | none |
| Wall-clock time | the server kills the worker | the server kills the worker | the server kills the worker |
<!-- end: guards -->

On Linux the seccomp filter refuses every socket, a pair of a job's own
included, since one of a pair of datagram sockets can be sent to any Unix
socket's address. Beside them it refuses a process of the job's own
(`fork`, `vfork`, and a `clone` that starts no thread), so that a job's
limits are not a limit for each of as many processes as it likes; and a
signal, or a change of resource limits, aimed at any process but the worker
itself, by a call or by naming that process the owner of a descriptor, so
that a job cannot stop its server. Threads still start.

So on Linux a bundle that starts a process, with `subprocess` or
`multiprocessing`, or opens a socket, as an `asyncio` event loop does for
itself, fails there with `PermissionError`.

### What Landlock covers, by kernel

`files landlock` says that Landlock is on, not how much of it the kernel
has. The worker asks for all of it and gets what the kernel knows:

| Linux | Landlock adds | Until then |
| --- | --- | --- |
| 5.13 | Files: nothing made, changed or removed outside the scratch directory, and nothing read but the Python environment's own files and the system's libraries | No file guard: `files none` |
| 5.19 | Links and renames between two directories of the scratch directory, allowed | Every link or rename between two directories is refused, inside it too |
| 6.2 | Truncation | **A job can empty, by its path, any file the server's user may write**: the SQLite store among them |
| 6.7 | TCP connections and binds | Nothing: seccomp refuses every socket |
| 6.12 | Signals and abstract Unix sockets kept inside the sandbox | Nothing: seccomp refuses both |

So a server that runs bundles it does not trust wants Linux 6.2 or later.

## Layer 3: the container profile (not built yet)

The process profile shares a kernel, a user and a filesystem with the
server. A container profile, which starts each job's worker in a container
with no network, a read-only root, a scratch mount with a size, and limits
on memory, CPU and the number of processes, closes most of
[what nothing stops](#what-nothing-stops) below: it is the profile for a
server that runs strangers' bundles, and has a story of its own (QUA-343).

Until it exists, an operator can have much of it by running the whole
server in a container or a virtual machine that holds nothing else: no
network beyond the listener, no credentials, a store of its own.

## What the tests show

### Bundles that mean harm

`quantforge-py/tests/test_hostile_bundles.py` uploads each of these to a
real `serve run`, on SQLite and on PostgreSQL, and submits a backtest of
it, as any bundle is run. Where the server's workers say they enforce the
guard that holds a bundle, its attempt must fail and the job's state or log
must say so; where they say they enforce none, the attempt goes through,
and the test holds it to that too, so that this page cannot claim a hole
that is not there. After each one the server runs an honest job.

<!-- begin: hostile bundles -->
| Bundle | What it tries | Held by |
| --- | --- | --- |
| `fork-bomb` | To start sixteen processes that wait | network |
| `memory-bomb` | To take a quarter of a gibibyte more address space than its limit | memory |
| `socket` | To connect to a listener on this machine, and to make a pair of sockets of its own | network |
| `outside-read` | To read `/etc/passwd`, and a file the test made outside every scratch directory | files |
| `outside-write` | To write outside its scratch directory: by a path that climbs, by a path from the root, through a link it makes, and through a second name for a file outside | files |
| `server-secrets` | To read its server's environment and command line | server |
| `signal-server` | To signal its server, to read its resource limits, and to name it the owner of a descriptor, whom the kernel signals | network |
| `cpu-spin` | To spin for good | cpu |
| `cpu-spin-ignoring-the-signal` | The same, ignoring the signal its CPU limit sends | cpu |
| `wall-sleep` | To wait for an hour | server |
| `forged-frames` | To say in its worker's place how the job ended: a result of its own naming, a trade of another symbol, more rows than the job has bars, a failure no job fails as, and bytes that are no frame | server |
| `long-failure` | To fail with four megabytes of message, under a name of 200,000 letters | server |
| `log-flood` | To write five megabytes to its log | server |
| `exit` | To end its worker | server |
<!-- end: hostile bundles -->

"Held by" is the guard as `worker_isolation:` names it, or `server` for
what the server holds a job to on every system. The filter that refuses a
socket refuses a process and a signal with it, so those three share the
`network` guard.

How each is reported, where its guard holds:

- **Refused by the kernel** (`fork-bomb`, `socket`, `outside-read`): Python
  raises `PermissionError`, the job fails as `invalid_request` naming it,
  and the traceback is in the job's log. `outside-write` and
  `signal-server` catch each refusal and list them in the log instead, and
  `server-secrets` finds nothing to list: on Linux the server is not
  dumpable, so its environment is not its user's to read.
- **Stopped at a limit** (`memory-bomb`, `cpu-spin`, `wall-sleep`): the job
  fails as `invalid_request`, its message naming the limit and its value.
  A memory limit met inside Python fails the job as a `MemoryError`. A job
  that ignores the CPU limit's signal is killed on Linux once it has used
  one more second of CPU time, and reported as stopped by `SIGKILL`.
- **Refused by the server** (`forged-frames`, `exit`): the job fails as
  `internal`, `the job's worker sent the server something it cannot read`
  or `the job's worker stopped before the job finished`, and has no result.
- **Bounded by the server** (`long-failure`, `log-flood`): the job keeps the
  first 8 KiB of its failure and the newest megabyte of its log.

### Every claim and its test

Each row names a test by its file and function. `tests/workers/docs.rs`
holds every name here to a test that exists, and the table of guards above
to what a worker started on each system says it enforces.

<!-- begin: evidence -->
| Layer | Claim | Test |
| --- | --- | --- |
| Submission | A name that would leave the bundle is refused however it is spelled | `tests/bundle/hostile.rs::a_name_that_would_leave_the_bundle_is_refused_however_it_is_spelled` |
| Submission | An entry that is no regular file is refused: links, devices, directories, other dialects | `tests/bundle/hostile.rs::an_entry_that_is_no_regular_file_is_refused_whatever_it_claims_to_be` |
| Submission | An archive cut short is refused at every length | `tests/bundle/hostile.rs::an_archive_cut_short_is_refused_at_every_length` |
| Submission | An archive that claims more than it holds is refused by the bytes it has | `tests/bundle/hostile.rs::an_archive_that_claims_more_than_it_holds_is_refused_by_the_bytes_it_has` |
| Submission | A compressed archive is refused, and never unpacked | `tests/bundle/hostile.rs::a_compressed_archive_is_refused_and_never_unpacked` |
| Submission | More entries than a bundle holds are refused | `tests/bundle/hostile.rs::more_entries_than_a_bundle_holds_are_refused_whatever_they_weigh` |
| Submission | A manifest nested as deep as it is long is refused | `tests/bundle/hostile.rs::a_manifest_nested_as_deep_as_it_is_long_is_refused_without_reading_it_that_deep` |
| Submission | Of 15,000 mutations of canonical tars, each is refused or read back as exactly its bytes, every path of it inside the bundle | `tests/bundle/hostile.rs::no_mutation_of_a_canonical_tar_is_read_as_anything_but_the_bytes_it_is` |
| Submission | No run of bytes is read as a bundle unless it is one | `tests/bundle/hostile.rs::no_run_of_bytes_is_read_as_a_bundle_unless_it_is_one` |
| Submission | A directory that holds a link or a pipe is refused, and none of it opened | `tests/bundle/hostile.rs::a_directory_that_holds_what_is_no_file_is_refused_and_none_of_it_is_opened` |
| Submission | A hostile upload is refused by the route, and none of it stored | `src/server/tests/hostile.rs::an_upload_whose_entries_would_leave_a_bundle_is_refused_and_none_of_it_is_stored` |
| Submission | A packed upload is read as the bytes sent | `src/server/tests/hostile.rs::an_upload_sent_packed_is_read_as_the_bytes_sent_and_never_unpacked` |
| Submission | A path that climbs names no route and no file | `src/server/tests/hostile.rs::a_path_that_climbs_names_no_route_and_no_file` |
| Worker | A bundle lands under `bundle/` and nowhere else | `src/server/workers/scratch.rs::a_bundle_lands_under_bundle_and_nowhere_else` |
| Worker | A link planted where a bundle goes fails the write | `src/server/workers/scratch.rs::a_link_planted_where_a_bundle_goes_fails_the_write_and_nothing_lands_beyond_it` |
| Worker | A directory of a bundle is made new, or was made for it, and is never a link | `src/server/workers/scratch.rs::a_directory_of_a_bundle_is_made_new_or_was_made_for_it_and_is_never_a_link` |
| Worker | The same, through a real worker's scratch directory | `tests/workers/hostile.rs::a_link_planted_where_a_bundle_goes_takes_no_file_out_of_the_scratch_directory` |
| Worker | Nothing a job writes lands outside its scratch directory | `tests/workers/hostile.rs::nothing_a_job_writes_lands_outside_its_scratch_directory` |
| Worker | A job reads no file outside its scratch directory, another job's included | `tests/workers/isolation.rs::a_job_reads_neither_another_jobs_scratch_directory_nor_any_file_outside_its_own` |
| Worker | A job reaches no network | `tests/workers/isolation.rs::a_job_reaches_no_network` |
| Worker | A job signals no process but itself, takes no other's limits, and starts none | `tests/workers/hostile.rs::a_job_reaches_no_process_but_itself_and_starts_none` |
| Worker | A job cannot leave its worker's process group | `tests/workers/isolation.rs::a_job_cannot_leave_its_workers_process_group_where_seccomp_holds` |
| Worker | A worker gets no environment and no descriptor of its server | `tests/workers/isolation.rs::a_worker_gets_no_environment_and_no_descriptor_of_the_server` |
| Worker | Every job gets a fresh worker | `tests/workers/isolation.rs::every_job_gets_a_fresh_worker_that_holds_nothing_of_the_last` |
| Worker | A job that uses up its CPU time is stopped, and the limit named | `tests/workers/limits.rs::a_job_that_uses_up_its_cpu_time_is_stopped_and_named` |
| Worker | A job that takes more memory than it may is stopped, and the limit named | `tests/workers/limits.rs::a_job_that_takes_more_memory_than_it_may_is_stopped_and_named` |
| Worker | A job past its wall-clock time is killed, and the limit named | `tests/workers/limits.rs::a_job_past_its_wall_clock_time_is_killed_and_named` |
| Worker | A guard that does not hold fails its test where the sandbox has to hold | `tests/workers/hostile.rs::a_test_of_a_guard_that_does_not_hold_fails_only_where_the_sandbox_has_to` |
| Frames | Only a worker started for the tests' workloads reports a result of its own naming | `src/server/workers/job/tests.rs::only_a_worker_started_for_the_synthetic_workloads_reports` |
| Frames | A trade of another symbol than the job's is refused | `src/server/workers/job/tests.rs::a_trade_of_another_symbol_than_the_jobs_is_refused_and_its_symbol_never_said` |
| Frames | A worker sends no more rows than its job has bars | `src/server/workers/job/tests.rs::a_worker_cannot_send_more_rows_than_its_job_has_bars` |
| Frames | A failure keeps the start of what its worker said | `src/server/workers/job/tests.rs::a_failure_keeps_the_start_of_what_its_worker_said_and_says_how_much_it_cut` |
| Frames | A failure no job fails as is refused | `src/server/workers/job/tests.rs::a_failure_no_job_fails_as_is_refused` |
| Client | A job's words are printed with their control characters written out | `src/cli/backtest/remote.rs::a_jobs_words_are_shown_with_their_control_characters_written_out` |
| Bundles | Every hostile bundle above, through a real server on both stores | `quantforge-py/tests/test_hostile_bundles.py::test_the_page_lists_the_suite_and_what_holds_each_bundle` |
| Bundles | A worker holds nothing of its server's store, its URL and password included | `quantforge-py/tests/test_worker_jobs.py::test_a_worker_holds_nothing_of_its_servers_store` |
<!-- end: evidence -->

A test of a guard skips where the workers do not enforce it, and says so.
CI's Linux legs set `QUANTFORGE_TEST_SANDBOX_REQUIRED=1`, which makes such
a test fail instead ([ci.md](../ci.md)): no run there passes for a sandbox
that was never on.

## What nothing stops

Each of these is open today. None is a bug to report: they are the edge of
what the process profile is.

**No authentication.** Every route answers whoever reaches it. Anyone who
can connect to the listener can run code as the server's user, read every
job, and cancel any of them. *Until server authentication lands (QUA-345),
keep `--listen` on this machine, and put any other use behind a proxy that
authenticates.*

**A web page on the same machine.** A page open in a browser on the
server's machine can reach a loopback listener through DNS rebinding, and
the server does not check the `Host` a request names. Recent browsers ask
before a public page reaches this machine; not all do. Authentication
closes this too, and the same story owns it. *Until then, do not browse
from the account that runs a server with `--job-python`.*

**macOS and Windows.** There is no file, network or process guard. A bundle
reads and writes whatever its user can, reaches the network, and on macOS
reads the environment of every process of its user with `ps`, the server's
and its store's URL among them; on Windows it can read the server's memory.
macOS sends the CPU limit's signal once, and a job that ignores it runs on
to its wall clock; neither system limits memory. On Windows a process a job
starts outlives it. *These are for running your own bundles.*

**Linux without Landlock, or with an old one.** A kernel before 5.13, or one
that does not enable Landlock, has no file guard: `files none`. Before 6.2
a job can empty any file its user may write. *Run on 6.2 or later, and read
the `worker_isolation:` line.*

**The disk.** Nothing limits how much a job writes to its scratch directory,
which is in the system's temporary directory. A job can fill it within its
wall clock. *Give the server a `TMPDIR` on a filesystem of its own.*

**The same user, the same kernel.** A worker runs as the server's user, and
the seccomp filter names the calls it refuses: every other call reaches the
kernel. A job can still lower the scheduling priority of its user's other
processes or pin them to a CPU, use System V shared memory and the kernel's
keyrings of its user, and learn the machine's name, kernel and clock. A flaw
in the kernel, in Landlock, or in the interpreter's environment is a way
out. So is running the server as root, which no guard here is written for:
*run `serve run` as a user that owns nothing else.*

**Side channels.** Jobs share a processor and a kernel with each other and
with the server. Nothing here addresses what timing can tell one job of
another.

**The store's growth.** Bundles and results are never deleted, and nothing
limits how many a client uploads or how many jobs it submits. Each is
bounded: a bundle by 8 MiB, a job's log by 1 MiB, its result by its bars.
Their number is not, until quotas land with authentication.

**Held connections.** A client that opens connections and sends nothing
holds them, and their descriptors, for as long as it likes; nothing limits
requests by rate. *A proxy in front limits both.*

**What a result means.** A job's result is what its code said. A bundle
that speaks in its worker's place can report any trades and any equity
curve the result format holds, as long as there is one equity row per bar.
A rerun runs the same code and can be told the same thing: `verified`
means that the bundle gives those files again on those bars, not that an
honest engine computed them. *The bundle is named by the hash of its
files, which anyone it is shared with can read.*

**What a job's log draws.** A job's log and its failure are its bundle's
words. `quantforge backtest --remote` prints them with every control
character written out; any other client that shows them must do its own
escaping.

**The server's command line.** A store's URL given as `--postgres-url` is
in the process list for every user of the machine, and for a job where no
file guard holds. *Give it as `QF_POSTGRES_URL`.*

## Running a server

1. Linux 6.2 or later, and a `worker_isolation:` line with no `none` in it.
2. A user of its own, not root, that owns nothing but the store's file, if
   it is SQLite.
3. `--listen` left on this machine. No authentication exists yet.
4. A clean environment: the store's URL in `QF_POSTGRES_URL`, never the
   flag, and no exchange credentials. A PostgreSQL role for this store
   alone.
5. A `--job-python` environment that holds `quantforge-py`, NumPy and
   pandas, and nothing else.
6. `TMPDIR` on a filesystem of its own, with a size.
7. Bundles from people you do not trust: not on macOS or Windows, and on
   Linux only with the whole server in a container or virtual machine that
   holds nothing else, until the container profile exists.

[SECURITY.md](../../SECURITY.md) says which of this is a vulnerability to
report, and how.
