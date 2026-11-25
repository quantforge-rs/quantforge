//! The sandbox a worker puts on itself on Linux: closed to the other
//! processes of its user, its files confined to its scratch directory by
//! Landlock, beside the directories its runtime reads, which it may read
//! and never change, and, by seccomp, no socket, no process of its own, no
//! signal or resource limit for any process but itself, and no way out of
//! its process group.
//!
//! The filter names the calls it refuses and lets every other through: it
//! closes the ways out that are known, and is no list of what a job needs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use landlock::{
    ABI, Access, AccessFs, AccessNet, RestrictionStatus, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetError, RulesetStatus, Scope, path_beneath_rules,
};
use rustix::process::{DumpableBehavior, set_dumpable_behavior};
use seccompiler::{
    BackendError, BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition,
    SeccompFilter, SeccompRule, TargetArch,
};

use super::Guard;

/// The Landlock ABI the sandbox is written against: a newer kernel enforces
/// all of it, an older one what it knows, and a kernel without Landlock
/// none.
const LANDLOCK: ABI = ABI::V6;

/// When a system call is refused.
#[derive(Clone, Copy)]
enum When {
    /// Whatever it asks.
    Always,
    /// Unless it starts a thread of this process: `clone` without
    /// `CLONE_THREAD` starts a process.
    UnlessAThread,
    /// When its first argument names anything but this process: another
    /// process or one of its threads, a process group, or every process.
    AimedElsewhere,
    /// The same, where naming no process means this one too.
    AimedAtAnother,
    /// When it names an owner for a descriptor, whom the kernel signals as
    /// the descriptor has something to read: `fcntl`'s `F_SETOWN` for any
    /// process but this one, and `F_SETOWN_EX`, whose owner is in memory.
    NamingAnOwner,
}

/// A system call a worker may not make, and when.
struct Refused {
    call: i64,
    /// The call's x32 number, which a 64-bit process can also make on a
    /// kernel built with x32 (`arch/x86/entry/syscalls/syscall_64.tbl`).
    #[cfg(target_arch = "x86_64")]
    x32: i64,
    when: When,
}

#[cfg(target_arch = "x86_64")]
const X32: i64 = 0x4000_0000;

/// `fcntl`'s `F_SETOWN_EX`, which libc gives no name here: 15 on every
/// architecture a filter is built for (`include/uapi/asm-generic/fcntl.h`).
const F_SETOWN_EX: libc::c_int = 15;

/// A row of `REFUSED`: a call by libc's name for its number, and when it
/// is refused. Most calls have one number for both of x86-64's ABIs, with
/// the x32 bit set for x32; a call x32 numbers apart gives that number.
macro_rules! refused {
    ($call:ident, $when:expr) => {
        Refused {
            call: libc::$call,
            #[cfg(target_arch = "x86_64")]
            x32: X32 | libc::$call,
            when: $when,
        }
    };
    ($call:ident, x32 $number:literal, $when:expr) => {
        Refused {
            call: libc::$call,
            #[cfg(target_arch = "x86_64")]
            x32: X32 | $number,
            when: $when,
        }
    };
}

/// The system calls a worker may not make, each refused with `EACCES`:
///
/// - `socket` and `socketpair`, so that a job reaches no network and no
///   local service, the server's database included: a pair of datagram
///   sockets can be sent to any Unix socket's address. And io_uring's
///   calls, which can open a socket without either;
/// - `ptrace` and `process_vm_*`, which read and write other processes'
///   memory;
/// - `setsid` and `setpgid`, with which a process could leave its worker's
///   process group, which the server kills;
/// - `fork`, `vfork` and a `clone` that starts no thread, so that a job
///   starts no process: a job's limits are its worker's own, and every
///   process a job started would have them over again. `clone3`, whose
///   flags no filter can read, is answered apart ([`UNREADABLE`]);
/// - every way to signal, and `prlimit64`, aimed at anything but the
///   worker itself: a job stops no other process of its user, its server
///   least of all, by a signal, by taking its CPU time away, or by having
///   the kernel signal it as the owner of a descriptor.
const REFUSED: &[Refused] = &[
    refused!(SYS_socket, When::Always),
    refused!(SYS_socketpair, When::Always),
    refused!(SYS_io_uring_setup, When::Always),
    refused!(SYS_io_uring_enter, When::Always),
    refused!(SYS_io_uring_register, When::Always),
    refused!(SYS_ptrace, x32 521, When::Always),
    refused!(SYS_process_vm_readv, x32 539, When::Always),
    refused!(SYS_process_vm_writev, x32 540, When::Always),
    refused!(SYS_setsid, When::Always),
    refused!(SYS_setpgid, When::Always),
    // The architectures after x86-64 have `clone` alone.
    #[cfg(target_arch = "x86_64")]
    refused!(SYS_fork, When::Always),
    #[cfg(target_arch = "x86_64")]
    refused!(SYS_vfork, When::Always),
    refused!(SYS_clone, When::UnlessAThread),
    refused!(SYS_kill, When::AimedElsewhere),
    refused!(SYS_tkill, When::AimedElsewhere),
    refused!(SYS_tgkill, When::AimedElsewhere),
    refused!(SYS_rt_sigqueueinfo, x32 524, When::AimedElsewhere),
    refused!(SYS_rt_tgsigqueueinfo, x32 536, When::AimedElsewhere),
    refused!(SYS_pidfd_send_signal, When::Always),
    refused!(SYS_prlimit64, When::AimedAtAnother),
    refused!(SYS_fcntl, When::NamingAnOwner),
];

/// The calls answered `ENOSYS`, as a kernel without them answers: `clone3`
/// takes its flags in memory, where no filter can tell a thread from a
/// process, and a C library that is told the kernel has none starts its
/// threads with `clone`, which the filter can read.
const UNREADABLE: &[Refused] = &[refused!(SYS_clone3, When::Always)];

/// Not dumpable: no other process of its user can attach to it, or read its
/// memory or environment under `/proc`.
pub(super) fn close_to_others() -> rustix::io::Result<()> {
    set_dumpable_behavior(DumpableBehavior::NotDumpable)
}

/// Every right in `scratch`, the rights to read and run what is in each
/// directory of `readable`, and none elsewhere; no TCP bind or connection;
/// and signals and abstract sockets kept inside the sandbox, so that a job
/// cannot signal the server. A kernel enforces what its Landlock knows.
pub(super) fn confine_files(scratch: &Path, readable: &[PathBuf]) -> Guard {
    match restrict(scratch, readable) {
        Ok(status) => match status.ruleset {
            RulesetStatus::NotEnforced => Guard::not("this kernel has no Landlock"),
            _ => Guard::by("landlock"),
        },
        Err(error) => Guard::not(format!("the kernel refused the Landlock sandbox: {error}")),
    }
}

/// A directory of `readable` that cannot be opened is left out, and stays
/// closed like any other.
fn restrict(scratch: &Path, readable: &[PathBuf]) -> Result<RestrictionStatus, RulesetError> {
    Ruleset::default()
        .handle_access(AccessFs::from_all(LANDLOCK))?
        .handle_access(AccessNet::from_all(LANDLOCK))?
        .scope(Scope::from_all(LANDLOCK))?
        .create()?
        .add_rules(path_beneath_rules([scratch], AccessFs::from_all(LANDLOCK)))?
        .add_rules(path_beneath_rules(readable, AccessFs::from_read(LANDLOCK)))?
        .restrict_self()
}

/// Refuses the system calls of [`REFUSED`] and [`UNREADABLE`], on every
/// thread, for this process and whatever it runs in its place.
pub(super) fn refuse_calls() -> Guard {
    let arch = std::env::consts::ARCH;
    let Ok(target) = TargetArch::try_from(arch) else {
        return Guard::not(format!("this build has no seccomp filter for {arch}"));
    };
    let filters = [(UNREADABLE, libc::ENOSYS), (REFUSED, libc::EACCES)]
        .map(|(calls, errno)| filter(calls, errno, target, std::process::id()));
    for filter in filters {
        let program = match filter {
            Ok(program) => program,
            Err(error) => {
                return Guard::not(format!("the seccomp filter cannot be built: {error}"));
            }
        };
        if let Err(error) = seccompiler::apply_filter_all_threads(&program) {
            return Guard::not(format!("the kernel refused the seccomp filter: {error}"));
        }
    }
    Guard::by("seccomp")
}

/// The filter that answers `calls` with `errno` in the process `own`, and
/// leaves every other call alone.
fn filter(
    calls: &[Refused],
    errno: i32,
    target: TargetArch,
    own: u32,
) -> Result<BpfProgram, BackendError> {
    let mut rules = BTreeMap::new();
    for refused in calls {
        rules.insert(refused.call, rule(refused.when, own)?);
        #[cfg(target_arch = "x86_64")]
        rules.insert(refused.x32, rule(refused.when, own)?);
    }
    SeccompFilter::new(
        rules,
        SeccompAction::Allow,
        SeccompAction::Errno(errno.unsigned_abs()),
        target,
    )
    .and_then(BpfProgram::try_from)
}

/// The rules a call is refused by: none refuses it whatever it asks, and
/// one refuses it when every condition of the rule holds.
fn rule(when: When, own: u32) -> Result<Vec<SeccompRule>, BackendError> {
    // A process, as a call names one: the low half of its first argument.
    let not = |process: u32| {
        SeccompCondition::new(
            0,
            SeccompCmpArgLen::Dword,
            SeccompCmpOp::Ne,
            u64::from(process),
        )
    };
    let conditions = match when {
        When::Always => return Ok(Vec::new()),
        // Its flags are its first argument on every architecture a filter
        // is built for.
        When::UnlessAThread => vec![SeccompCondition::new(
            0,
            SeccompCmpArgLen::Qword,
            SeccompCmpOp::MaskedEq(u64::from(libc::CLONE_THREAD.unsigned_abs())),
            0,
        )?],
        When::AimedElsewhere => vec![not(own)?],
        When::AimedAtAnother => vec![not(0)?, not(own)?],
        When::NamingAnOwner => {
            // The command is the call's second argument, and the owner
            // `F_SETOWN` names its third.
            let command = |command: libc::c_int| {
                SeccompCondition::new(
                    1,
                    SeccompCmpArgLen::Dword,
                    SeccompCmpOp::Eq,
                    u64::from(command.unsigned_abs()),
                )
            };
            let another = SeccompCondition::new(
                2,
                SeccompCmpArgLen::Dword,
                SeccompCmpOp::Ne,
                u64::from(own),
            )?;
            return Ok(vec![
                SeccompRule::new(vec![command(libc::F_SETOWN)?, another])?,
                SeccompRule::new(vec![command(F_SETOWN_EX)?])?,
            ]);
        }
    };
    Ok(vec![SeccompRule::new(conditions)?])
}
