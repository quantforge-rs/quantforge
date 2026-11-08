//! The resource limits a worker puts on itself on Unix. A process can only
//! lower its own hard limits, so a limit it already has below the one asked
//! for stays.

use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

use super::Guard;

/// No core dumps: a job stopped by a signal leaves no image of its memory.
pub(super) fn no_core_dumps() {
    let _ = setrlimit(
        Resource::Core,
        Rlimit {
            current: Some(0),
            maximum: Some(0),
        },
    );
}

/// At most `secs` seconds of CPU time. Past them the kernel sends SIGXCPU,
/// which stops the process, and SIGKILL a second later if the job catches
/// it.
pub(super) fn limit_cpu(secs: u64) -> Guard {
    let hard = lowered(Resource::Cpu, secs.saturating_add(1));
    limit(
        Resource::Cpu,
        Rlimit {
            current: Some(secs.min(hard)),
            maximum: Some(hard),
        },
    )
}

/// At most `bytes` of address space: an allocation past them fails.
#[cfg(target_os = "linux")]
pub(super) fn limit_memory(bytes: u64) -> Guard {
    let hard = lowered(Resource::As, bytes);
    limit(
        Resource::As,
        Rlimit {
            current: Some(hard),
            maximum: Some(hard),
        },
    )
}

/// `wanted`, or the hard limit the process already has when that is lower.
fn lowered(resource: Resource, wanted: u64) -> u64 {
    getrlimit(resource)
        .maximum
        .map_or(wanted, |hard| hard.min(wanted))
}

fn limit(resource: Resource, rlimit: Rlimit) -> Guard {
    match setrlimit(resource, rlimit) {
        Ok(()) => Guard::by("rlimit"),
        Err(error) => Guard::not(format!("the kernel refused the limit: {error}")),
    }
}
