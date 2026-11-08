//! The limits and sandbox a worker puts on itself as it starts, before it
//! reads a job and while it has no thread but one, and what it reports of
//! them. Each guard either holds or says why not: an operating system
//! without the mechanism, or a kernel that refused it.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(unix)]
mod unix;

use std::fmt;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// What a worker enforced on itself: a guard each for its CPU time, its
/// memory, the network, and the files outside its scratch directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posture {
    /// The CPU time a job may use.
    pub cpu: Guard,
    /// The memory a job may use.
    pub memory: Guard,
    /// Whether a job can reach the network, or any socket.
    pub network: Guard,
    /// Whether a job can reach files outside its scratch directory.
    pub files: Guard,
}

/// One guard: what enforces it, or why nothing does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Guard {
    /// Enforced, by a mechanism such as `rlimit`, `seccomp` or `landlock`.
    Enforced {
        /// The mechanism.
        by: String,
    },
    /// Not enforced.
    Unenforced {
        /// Why not.
        why: String,
    },
}

impl Guard {
    fn by(mechanism: &str) -> Self {
        Self::Enforced {
            by: mechanism.to_string(),
        }
    }

    fn not(why: impl Into<String>) -> Self {
        Self::Unenforced { why: why.into() }
    }

    /// Whether the guard holds.
    pub fn is_enforced(&self) -> bool {
        matches!(self, Self::Enforced { .. })
    }
}

/// The mechanism, or `none`.
impl fmt::Display for Guard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enforced { by } => f.write_str(by),
            Self::Unenforced { .. } => f.write_str("none"),
        }
    }
}

/// `cpu rlimit, memory rlimit, network seccomp, files landlock`, with
/// `none` for a guard that does not hold.
impl fmt::Display for Posture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cpu {}, memory {}, network {}, files {}",
            self.cpu, self.memory, self.network, self.files
        )
    }
}

/// The limits a worker puts on itself, where its operating system has them.
#[derive(Clone, Copy, Debug)]
pub(super) struct Limits {
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(super) cpu_secs: u64,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(super) memory_bytes: u64,
}

/// Puts the limits and the sandbox on this process, and reports what held.
/// The process must have no thread but the caller: a sandbox covers the
/// thread that sets it up and the threads it starts afterwards. `scratch`
/// is the only directory the sandbox leaves it.
#[cfg(target_os = "linux")]
pub(super) fn isolate(limits: Limits, scratch: &Path) -> Posture {
    unix::no_core_dumps();
    // Failing, it leaves the worker's /proc open to its user's processes,
    // which Landlock closes anyway where the kernel has it.
    let _ = linux::close_to_others();
    let cpu = unix::limit_cpu(limits.cpu_secs);
    let memory = unix::limit_memory(limits.memory_bytes);
    // Landlock before seccomp, which leaves its calls alone anyway.
    let files = linux::confine_files(scratch);
    let network = linux::refuse_sockets();
    Posture {
        cpu,
        memory,
        network,
        files,
    }
}

/// Puts the limits on this process, and reports what held: a CPU-time
/// limit only.
#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn isolate(limits: Limits, _scratch: &Path) -> Posture {
    unix::no_core_dumps();
    let os = std::env::consts::OS;
    Posture {
        cpu: unix::limit_cpu(limits.cpu_secs),
        memory: Guard::not(format!("{os} enforces no address-space limit")),
        network: Guard::not(format!("the process profile has no network guard on {os}")),
        files: Guard::not(format!("the process profile has no file guard on {os}")),
    }
}

/// Reports that nothing holds: the process profile has no guard here but
/// the wall clock, which the server keeps.
#[cfg(not(unix))]
pub(super) fn isolate(_limits: Limits, _scratch: &Path) -> Posture {
    let not = |what: &str| {
        Guard::not(format!(
            "the process profile has no {what} on {}",
            std::env::consts::OS
        ))
    };
    Posture {
        cpu: not("CPU-time limit"),
        memory: not("memory limit"),
        network: not("network guard"),
        files: not("file guard"),
    }
}

/// Closes this process to the other processes of its user on Linux: none
/// can attach to it or read its memory or environment, so a worker cannot
/// read the database URL or credentials a server holds. Elsewhere it does
/// nothing. A server calls it before it starts its workers.
pub fn protect_server() -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::close_to_others().map_err(io::Error::from)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_posture_reads_as_each_mechanism_or_none() {
        let posture = Posture {
            cpu: Guard::by("rlimit"),
            memory: Guard::not("not here"),
            network: Guard::by("seccomp"),
            files: Guard::not("no Landlock"),
        };
        assert_eq!(
            posture.to_string(),
            "cpu rlimit, memory none, network seccomp, files none"
        );
        assert!(posture.cpu.is_enforced());
        assert!(!posture.files.is_enforced());
    }

    #[test]
    fn a_posture_travels_as_tagged_json() {
        let posture = Posture {
            cpu: Guard::by("rlimit"),
            memory: Guard::by("rlimit"),
            network: Guard::by("seccomp"),
            files: Guard::not("this kernel has no Landlock"),
        };
        let json = serde_json::to_string(&posture).expect("json");
        assert_eq!(
            json,
            r#"{"cpu":{"state":"enforced","by":"rlimit"},"memory":{"state":"enforced","by":"rlimit"},"network":{"state":"enforced","by":"seccomp"},"files":{"state":"unenforced","why":"this kernel has no Landlock"}}"#
        );
        assert_eq!(
            serde_json::from_str::<Posture>(&json).expect("a posture"),
            posture
        );
    }
}
