//! The sandbox a worker puts on itself on Linux: closed to the other
//! processes of its user, its files confined to its scratch directory by
//! Landlock, and its sockets and any way out of its process group refused
//! by seccomp.

use std::path::Path;

use landlock::{
    ABI, Access, AccessFs, AccessNet, RestrictionStatus, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetError, RulesetStatus, Scope, path_beneath_rules,
};
use rustix::process::{DumpableBehavior, set_dumpable_behavior};
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, TargetArch};

use super::Guard;

/// The Landlock ABI the sandbox is written against: a newer kernel enforces
/// all of it, an older one what it knows, and a kernel without Landlock
/// none.
const LANDLOCK: ABI = ABI::V6;

/// The system calls a worker may not make: `socket`, so that a job reaches
/// no network and no local service, the server's database included;
/// io_uring's, which can open a socket without it; `ptrace` and
/// `process_vm_*`, which read and write other processes' memory; and
/// `setsid` and `setpgid`, with which a process a job starts could leave
/// its worker's process group, which the server kills, and outlive it.
const REFUSED: [i64; 9] = [
    libc::SYS_socket,
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
    libc::SYS_ptrace,
    libc::SYS_process_vm_readv,
    libc::SYS_process_vm_writev,
    libc::SYS_setsid,
    libc::SYS_setpgid,
];

/// The same calls under their x32 numbers, which a 64-bit process can also
/// make on a kernel built with x32 (`arch/x86/entry/syscalls/syscall_64.tbl`),
/// each beside the call it stands for, in the order of `REFUSED`.
#[cfg(target_arch = "x86_64")]
const REFUSED_X32: [(i64, i64); 9] = [
    (libc::SYS_socket, X32 | 41),
    (libc::SYS_io_uring_setup, X32 | 425),
    (libc::SYS_io_uring_enter, X32 | 426),
    (libc::SYS_io_uring_register, X32 | 427),
    (libc::SYS_ptrace, X32 | 521),
    (libc::SYS_process_vm_readv, X32 | 539),
    (libc::SYS_process_vm_writev, X32 | 540),
    (libc::SYS_setsid, X32 | 112),
    (libc::SYS_setpgid, X32 | 109),
];

// Every call refused has its x32 number refused too, or the build fails.
#[cfg(target_arch = "x86_64")]
const _: () = {
    assert!(
        REFUSED_X32.len() == REFUSED.len(),
        "every call in REFUSED needs its x32 number in REFUSED_X32"
    );
    let mut call = 0;
    while call < REFUSED.len() {
        assert!(
            REFUSED_X32[call].0 == REFUSED[call],
            "REFUSED_X32 pairs each x32 number with its call, in the order of REFUSED"
        );
        call += 1;
    }
};

#[cfg(target_arch = "x86_64")]
const X32: i64 = 0x4000_0000;

/// Not dumpable: no other process of its user can attach to it, or read its
/// memory or environment under `/proc`.
pub(super) fn close_to_others() -> rustix::io::Result<()> {
    set_dumpable_behavior(DumpableBehavior::NotDumpable)
}

/// Every right in `scratch` and none elsewhere; no TCP bind or connection;
/// and signals and abstract sockets kept inside the sandbox, so that a job
/// cannot signal the server. A kernel enforces what its Landlock knows.
pub(super) fn confine_files(scratch: &Path) -> Guard {
    match restrict(scratch) {
        Ok(status) => match status.ruleset {
            RulesetStatus::NotEnforced => Guard::not("this kernel has no Landlock"),
            _ => Guard::by("landlock"),
        },
        Err(error) => Guard::not(format!("the kernel refused the Landlock sandbox: {error}")),
    }
}

fn restrict(scratch: &Path) -> Result<RestrictionStatus, RulesetError> {
    Ruleset::default()
        .handle_access(AccessFs::from_all(LANDLOCK))?
        .handle_access(AccessNet::from_all(LANDLOCK))?
        .scope(Scope::from_all(LANDLOCK))?
        .create()?
        .add_rules(path_beneath_rules([scratch], AccessFs::from_all(LANDLOCK)))?
        .restrict_self()
}

/// Refuses the system calls of `REFUSED` with `EACCES`, on every thread.
pub(super) fn refuse_sockets() -> Guard {
    let arch = std::env::consts::ARCH;
    let Ok(target) = TargetArch::try_from(arch) else {
        return Guard::not(format!("this build has no seccomp filter for {arch}"));
    };
    let refused = REFUSED.iter();
    #[cfg(target_arch = "x86_64")]
    let refused = refused.chain(REFUSED_X32.iter().map(|(_, x32)| x32));
    let filter = SeccompFilter::new(
        refused.map(|&number| (number, Vec::new())).collect(),
        SeccompAction::Allow,
        SeccompAction::Errno(libc::EACCES.unsigned_abs()),
        target,
    )
    .and_then(BpfProgram::try_from);
    let program = match filter {
        Ok(program) => program,
        Err(error) => return Guard::not(format!("the seccomp filter cannot be built: {error}")),
    };
    match seccompiler::apply_filter_all_threads(&program) {
        Ok(()) => Guard::by("seccomp"),
        Err(error) => Guard::not(format!("the kernel refused the seccomp filter: {error}")),
    }
}
