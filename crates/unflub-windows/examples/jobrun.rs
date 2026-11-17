//! Run a command inside a Windows Job Object that makes this machine behave like a slow one:
//! a hard CPU cap, a single core and a per-process commit limit. Used by `perf/lowend.ps1`.
//!
//! ```text
//! cargo run --release -p unflub-windows --example jobrun -- [--core-percent 25] [--cores 1]
//!     [--memory-mb 128] -- <program> [args...]
//! ```
//!
//! - `--core-percent`: CPU the job may use, as a percentage of ONE logical core, enforced with
//!   `JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP`. Windows takes the rate in 1/100 % of the whole
//!   machine, so 25 % of one core on 16 logical cores is `CpuRate = 2500 / 16 = 156`.
//! - `--cores`: affinity limited to this many logical cores (the lowest ones this process may use).
//! - `--memory-mb`: each process in the job may commit at most this much
//!   (`JOB_OBJECT_LIMIT_PROCESS_MEMORY`); allocations past it fail.
//!
//! jobrun puts itself in the job and then starts the command, so the command and its children
//! inherit every limit. The job is killed when jobrun exits (`KILL_ON_JOB_CLOSE`). On stderr it
//! prints `jobrun: pid <child pid> ...` once the child starts, and the job's peak commit when it
//! ends. The exit code is the child's.

/// `CpuRate` for `percent_of_core` % of one logical core on a machine with `logical` cores,
/// in the 1/100 % of total machine time that `JOBOBJECT_CPU_RATE_CONTROL_INFORMATION` expects.
#[cfg_attr(not(windows), allow(dead_code))]
fn cpu_rate(percent_of_core: f64, logical: u32) -> u32 {
    let rate = (percent_of_core * 100.0 / f64::from(logical.max(1))).round();
    rate.clamp(1.0, 10_000.0) as u32
}

/// The lowest `cores` set bits of `mask`.
#[cfg_attr(not(windows), allow(dead_code))]
fn lowest_cores(mask: usize, cores: u32) -> usize {
    let mut out = 0usize;
    let mut left = cores;
    for bit in 0..usize::BITS {
        if left == 0 {
            break;
        }
        if mask & (1 << bit) != 0 {
            out |= 1 << bit;
            left -= 1;
        }
    }
    out
}

fn main() {
    #[cfg(windows)]
    win::run();
    #[cfg(not(windows))]
    {
        eprintln!("jobrun uses Windows Job Objects and runs on Windows only");
        std::process::exit(2);
    }
}

#[cfg(windows)]
mod win {
    use std::process::Command;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectCpuRateControlInformation,
        JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
        JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_CPU_RATE_CONTROL_ENABLE, JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
        JOB_OBJECT_LIMIT_AFFINITY, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    };
    use windows::Win32::System::Threading::{
        GetActiveProcessorCount, GetCurrentProcess, GetProcessAffinityMask, ALL_PROCESSOR_GROUPS,
    };

    use super::{cpu_rate, lowest_cores};

    const USAGE: &str =
        "usage: jobrun [--core-percent 25] [--cores 1] [--memory-mb 128] -- <program> [args...]";

    struct Limits {
        core_percent: f64,
        cores: u32,
        memory_mb: usize,
    }

    fn fail(msg: impl std::fmt::Display) -> ! {
        eprintln!("jobrun: {msg}");
        std::process::exit(2);
    }

    fn parse() -> (Limits, Vec<String>) {
        let mut limits = Limits {
            core_percent: 25.0,
            cores: 1,
            memory_mb: 128,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            let mut value = |name: &str| {
                args.next()
                    .unwrap_or_else(|| fail(format!("{name} needs a value\n{USAGE}")))
            };
            match arg.as_str() {
                "--core-percent" => {
                    limits.core_percent = value(&arg).parse().unwrap_or_else(|e| fail(e))
                }
                "--cores" => limits.cores = value(&arg).parse().unwrap_or_else(|e| fail(e)),
                "--memory-mb" => limits.memory_mb = value(&arg).parse().unwrap_or_else(|e| fail(e)),
                "--" => {
                    let command: Vec<String> = args.collect();
                    if command.is_empty() {
                        fail(USAGE);
                    }
                    return (limits, command);
                }
                _ => fail(format!("unknown argument: {arg}\n{USAGE}")),
            }
        }
        fail(USAGE)
    }

    pub fn run() {
        let (limits, command) = parse();
        // SAFETY: plain Win32 calls with valid, correctly sized structs; the job handle is
        // deliberately never closed, so the job (and with KILL_ON_JOB_CLOSE, the child) ends
        // exactly when this process does.
        let (job, rate, affinity) = unsafe {
            let job = CreateJobObjectW(None, None).unwrap_or_else(|e| fail(e));
            let logical = GetActiveProcessorCount(ALL_PROCESSOR_GROUPS);
            let rate = cpu_rate(limits.core_percent, logical);

            let mut process_mask = 0usize;
            let mut system_mask = 0usize;
            GetProcessAffinityMask(GetCurrentProcess(), &mut process_mask, &mut system_mask)
                .unwrap_or_else(|e| fail(format!("GetProcessAffinityMask: {e}")));
            let affinity = lowest_cores(process_mask, limits.cores);

            let mut ext = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            ext.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_AFFINITY
                | JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            ext.BasicLimitInformation.Affinity = affinity;
            ext.ProcessMemoryLimit = limits.memory_mb << 20;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &ext as *const _ as *const _,
                std::mem::size_of_val(&ext) as u32,
            )
            .unwrap_or_else(|e| fail(format!("limits: {e}")));

            let mut cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION {
                ControlFlags: JOB_OBJECT_CPU_RATE_CONTROL_ENABLE
                    | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP,
                ..Default::default()
            };
            cpu.Anonymous.CpuRate = rate;
            SetInformationJobObject(
                job,
                JobObjectCpuRateControlInformation,
                &cpu as *const _ as *const _,
                std::mem::size_of_val(&cpu) as u32,
            )
            .unwrap_or_else(|e| fail(format!("CPU rate: {e}")));

            AssignProcessToJobObject(job, GetCurrentProcess())
                .unwrap_or_else(|e| fail(format!("assign: {e}")));
            (job, rate, affinity)
        };

        let mut child = Command::new(&command[0])
            .args(&command[1..])
            .spawn()
            .unwrap_or_else(|e| fail(format!("{}: {e}", command[0])));
        eprintln!(
            "jobrun: pid {} cpu-rate {rate}/10000 ({} % of one core) affinity {affinity:#x} \
             commit-limit {} MB",
            child.id(),
            limits.core_percent,
            limits.memory_mb
        );
        let status = child.wait().unwrap_or_else(|e| fail(e));
        if let Some(peak) = peak_commit(job) {
            eprintln!(
                "jobrun: peak process commit {:.1} MB",
                peak as f64 / 1_048_576.0
            );
        }
        std::process::exit(status.code().unwrap_or(1));
    }

    fn peak_commit(job: HANDLE) -> Option<usize> {
        let mut ext = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        // SAFETY: querying into a correctly sized struct.
        unsafe {
            QueryInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &mut ext as *mut _ as *mut _,
                std::mem::size_of_val(&ext) as u32,
                None,
            )
        }
        .ok()?;
        Some(ext.PeakProcessMemoryUsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_rate_is_a_share_of_the_whole_machine() {
        assert_eq!(cpu_rate(25.0, 1), 2500);
        assert_eq!(cpu_rate(25.0, 4), 625);
        assert_eq!(cpu_rate(25.0, 16), 156);
        assert_eq!(cpu_rate(25.0, 12), 208);
        assert_eq!(cpu_rate(100.0, 1), 10_000);
        assert_eq!(
            cpu_rate(25.0, 4096),
            1,
            "never zero, which would mean no limit"
        );
        assert_eq!(cpu_rate(250.0, 1), 10_000);
    }

    #[test]
    fn picks_the_lowest_allowed_cores() {
        assert_eq!(lowest_cores(0b1111, 1), 0b0001);
        assert_eq!(lowest_cores(0b1100, 1), 0b0100);
        assert_eq!(lowest_cores(0b1011, 2), 0b0011);
        assert_eq!(lowest_cores(0b0001, 4), 0b0001);
    }
}
