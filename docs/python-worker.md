# The Python worker

`quantforge serve run --job-python <path>` runs each backtest job's
[strategy bundle](bundle-spec.md) in a worker that is the Python interpreter
at `<path>`. The worker runs the bundle's strategy in the same engine,
through the same `quantforge_py` bindings, that `run_backtest` uses on a
user's machine: a bundle that runs locally runs on the server, and makes
the same trades. This page says what the server needs, how a job runs,
what a bundle may import, what a job's log holds, how a job fails, and
what a worker keeps from the server. [worker-isolation.md](worker-isolation.md)
is the process every worker runs in, its limits and its sandbox.

## The environment

The interpreter `--job-python` names (`QF_JOB_PYTHON`) is the
environment's: a virtualenv's `bin/python`, say, where are installed

- `quantforge-py`, built from the same release as the server: a worker is
  the package's own module `quantforge_py._worker`, and a server speaks
  only to workers of its own build, so a worker of another release refuses
  the server as it starts, and `serve run` stops before it listens;
- the packages bundles may import, each at the one version the server
  offers. This release allows NumPy and pandas:

```bash
python3.12 -m venv /srv/quantforge-jobs
# The package's wheel of the server's release, as CI builds it (docs/ci.md),
# and one version of each package bundles may import.
/srv/quantforge-jobs/bin/pip install quantforge_py-*.whl "numpy==2.3.5" "pandas==2.3.3"
quantforge serve run --job-python /srv/quantforge-jobs/bin/python
```

Each worker says, as it starts, what it offers: its Python, `MAJOR.MINOR`;
the engine it was built with; and each package of the allowlist its
environment holds, at the version installed. `serve run` prints it before
it listens:

```text
job_runtime: python 3.12, engine 0.3.0, numpy 2.3.5, pandas 2.3.3
```

Every bundle is admitted against that runtime as it is uploaded, as a job
of it is submitted, and as the job starts ([bundle-spec.md](bundle-spec.md#admission)):
a manifest whose `python` is another version, whose `engine` refuses the
server's, or whose `deps` name a package the server does not offer, or
another version of one, is refused with every reason, before anything
runs. Without `--job-python`, `job_runtime: none`: the server stores
bundles and queues jobs, and fails each job as it starts, `unavailable`.

A package installed beside the allowed ones, such as pandas' own
dependencies or pip, is no package a bundle may import, and so is no part
of the runtime.

## How a job runs

1. **Ahead of the job**, the server starts the worker: the interpreter,
   with `quantforge_py` loaded. The worker closes every descriptor the
   server gave it but its pipes, moves its frames off standard output,
   reads its setup, and readies the environment: it finds what it offers,
   reads what the environment installs, which the rules below are made of,
   and imports NumPy, where it offers it, so that a job that uses NumPy
   finds it loaded. Then it puts its limits and its sandbox on itself and
   says it is ready.
2. **The job** comes over standard input with its candles, which the
   server read from its store. Its bundle's files are in `bundle/` in the
   worker's scratch directory.
3. **The bundle's root** becomes the worker's working directory, so a
   strategy reads its own files by their paths in the bundle, and goes
   first on the import path. A module of the bundle named like one the
   server's Python already has, of the standard library or of an installed
   package, is refused: it would stand in for the server's own wherever
   that is imported, or, where that is loaded already, never be imported
   at all.
4. **The entry's module is imported**, its imports held to the rules
   below, and the entry's class is taken from it. It must be a class
   derived from `quantforge_py.Strategy`.
5. **The class is built** from the manifest's `params`, as keyword
   arguments: `SmaCross(fast=20, slow=50)` for `fast = 20` and
   `slow = 50`. A value reaches it as JSON does: an `int`, a `float`, a
   `str`, a `bool`, a `list` or a `dict`.
6. **The engine runs the strategy** over the candles, on the worker's main
   thread, which stays attached to Python throughout, exactly as
   `run_backtest` runs one: the same adapter, each callback handed a fresh
   `StrategyContext` and `Candle`, every amount a `decimal.Decimal`. The
   job's progress is said as its bars go by.
7. **A run that ends** sends its trades, its equity curve and its summary as
   rows, which the server checks and writes as the job's three
   [result files](result-spec.md). Then the server kills the worker, and
   starts another for the next job.

### Alike on every machine

What a strategy computes depends on its bars and its own state alone, and
the worker leaves nothing else to chance:

- every `str` hashes the same on every run (`PYTHONHASHSEED=0`), so a set
  of them is gone through in the same order;
- NumPy's linear algebra runs on one thread (`OPENBLAS_NUM_THREADS`,
  `OMP_NUM_THREADS` and `MKL_NUM_THREADS` are `1`), adding a sum up in one
  order whatever the machine's cores, and starting no thread before the
  sandbox;
- Python runs in UTF-8 mode (`-X utf8`), so a bundle reads its files alike
  everywhere.

A worker writes no bytecode (`-B`), has neither the user's site-packages
(`-s`) nor its working directory (`-P`) on its import path, and writes
Python's traceback to its standard error if it crashes
(`-X faulthandler`).

## What a bundle may import

A bundle's own code may import

- the standard library;
- its own modules;
- `quantforge_py`;
- each package its manifest's `deps` declares, which the server offers, or
  the bundle would not have been admitted.

Any other import its code asks for fails with an `ImportError` that says
what to do:

| The import | Its refusal |
| --- | --- |
| A package the server offers, which the manifest does not declare | `the bundle imports pandas, which its manifest does not declare: add pandas = "2.3.3" to [deps] in quantforge.toml` |
| Anything else, installed or not | `the bundle imports requests, which this server does not offer: a bundle imports the standard library, its own modules, quantforge_py and the packages its manifest declares, and this server offers numpy 2.3.5, pandas 2.3.3` |

The rules hold for an `import` statement, for `importlib.import_module`,
for a module NumPy's import already loaded ahead of the job, and for what
`quantforge_py` imports for the bundle, as `CandleBatch.to_numpy` imports
NumPy: a bundle that uses NumPy declares it. What the standard library and
the packages a bundle may import go on to import is their own: pandas'
dependencies are pandas' to import.

**The rules are best effort.** Code that goes around Python's import system,
or reads modules out of `sys.modules`, is not stopped by them. The boundary
is the worker's process and its sandbox
([worker-isolation.md](worker-isolation.md)).

## What a job's log holds

What a strategy writes is its job's log, a line at a time, as clients read
it from the logs route ([the job protocol](../src/protocol/jobs/mod.rs)):

| Written to | Level |
| --- | --- |
| Standard output: `print` | `info` |
| Standard error, where `logging`'s default handler and `warnings` write too | `warning` |
| A failure's traceback | `error` |

A line waits for its end, or for the job's; one longer than 64 KiB is said
in pieces. A read of standard input reads nothing: a worker's own standard
streams carry its frames, and what anything else writes to standard output,
a process the job starts or a library's own C code, goes to the worker's
standard error, which the server keeps the end of for a worker that stops.

## How a job fails

Every failure of the bundle's is `invalid_request`, its message naming what
failed, and its traceback, where it has one, is in the job's log, written as
Python writes one: the bundle's own frames, its files named by their paths
in the bundle, and the frames of the libraries it called, without the
runtime's own.

| What failed | The job's message |
| --- | --- |
| An import the rules refuse, or the entry's module raising as it is imported | `the bundle's strategy.py could not be imported: ImportError: …; its traceback is in the job's log` |
| No such class in the entry's module | `the bundle's strategy.py has no SmaCross` |
| A class not derived from `quantforge_py.Strategy` | `SmaCross in the bundle's strategy.py is not a strategy: make it a class derived from quantforge_py.Strategy` |
| The class refusing the bundle's `params` | `SmaCross could not be built from the bundle's params: TypeError: …; its traceback is in the job's log` |
| A module of the bundle named like one the server has | `the bundle's module json has the name of a module this server's Python already has: rename it` |
| A callback raising | `the strategy "sma_cross" failed in on_bar, at the bar that opened at 1767269400000: ZeroDivisionError: division by zero; its traceback is in the job's log` |

A run the engine refuses, a market that is not Spot or a range with no
candles, fails in the engine's words, as every backtest job does; a limit a
job runs into and a worker that crashes, as
[worker-isolation.md](worker-isolation.md#how-a-job-ends) says.

## What a worker keeps from its server

Nothing of the store. The server reads every candle a job runs on and
writes how it ended; a worker gets its job and its candles over a pipe:

- **Its environment** holds its scratch directory as its temporary
  directory, the four variables above that make a run alike everywhere,
  and nothing of the server's: no store, no URL, no credential. On Windows
  it holds `SYSTEMROOT` too, where Windows' own libraries find themselves.
- **Its descriptors** are its three pipes alone: it closes every other one
  it was given before it reads its setup.
- **Its files**, where the sandbox confines them, are its scratch directory,
  and, to read and never change, the directories its interpreter reads its
  own files from (its installation, its environment and its import path)
  and the system's libraries (`/lib`, `/lib64`, `/usr/lib`, `/usr/lib64`,
  `/usr/local/lib`).
- **One job each:** a worker is killed once its job ends, and the next job
  has a fresh one, so nothing one job leaves in memory or on disk reaches
  another.

A job's parameters, its traceback and its log hold what the bundle and its
strategy put there and nothing else.

## One runtime for every host

The runtime is one path, which every host that runs a bundle takes: the
bundle's strategy imported, held to the rules and built
(`quantforge_py/_runtime.py`), then driven by the engine through the
adapter, and a failure worded with its traceback (`quantforge-py/src/runtime.rs`).
A server's worker is its first host (`quantforge-py/src/worker.rs`); a
local backtest from the command line follows (QUA-461). It needs neither a
store nor a credential: a host hands it a bundle's files and candles.

## Job start

A job's worker is started ahead of it, with the interpreter up, `quantforge_py`
loaded, what its environment installs read and NumPy imported, so a job
pays for its bundle's import and its own run, not for Python's start or
for reading the environment, which takes tens of milliseconds. `test_worker_jobs.py` measures how long a
job takes from its submission to its strategy's first bar, with and without
NumPy, on every platform CI runs the package's tests on, and prints it: for
a job whose worker was ready, and for one submitted as soon as the job
before it ended, which on a pool of one worker also waits for the worker
that replaces the last one to start.

## Testing it

The package's tests run the worker for real (`quantforge-py/tests/`):

- `test_worker_jobs.py` starts `quantforge serve run` with
  `--job-python` the interpreter the tests run on, on a store of either
  backend, and runs bundles through the job routes: the goldens' SMA cross
  as a bundle, whose trades, equity curve and summary are the
  [goldens'](golden-parity.md) digit for digit on SQLite and PostgreSQL; a
  strategy that raises, whose traceback is read from the logs route; a
  package the server does not offer, refused at upload and at submission;
  imports the rules refuse; what a strategy writes; a worker per job; a
  worker's environment, parameters, traceback and log, which hold no part
  of the store's URL; and how long a job takes to start;
- `test_runtime.py` runs the runtime in the test's own process: every rule
  and every refusal above, and what a worker offers.

They run in CI's `python` job, on the oldest Python and the newest, and on
every `wheel` leg, on Linux, macOS and Windows ([ci.md](ci.md)).
