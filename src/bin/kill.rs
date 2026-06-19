//! A native `kill` for fishbowl.
//!
//! fish reports **Win32** process ids (`$last_pid`, `jobs`, `$pid`). The MSYS
//! `/usr/bin/kill` resolves pids in Cygwin's *separate* pid namespace, so it
//! cannot signal a process fish spawned (it reports "No such process"). This
//! `kill` signals by Win32 pid instead, so `command kill -9 $last_pid` works.
//! fish prepends its own program directory to the external-command search path
//! (see `path_try_get_path`) so this shadows the MSYS `kill` without touching
//! `$PATH`.
//!
//! NO FALLBACKS: real `OpenProcess`/`TerminateProcess`, plus ntdll
//! `NtSuspendProcess`/`NtResumeProcess` for `SIGSTOP`/`SIGCONT`.
#![cfg(windows)]

use std::process::exit;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows_sys::Win32::System::Threading::{
    OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SUSPEND_RESUME,
    PROCESS_TERMINATE,
};

/// Map a signal name (with or without the `SIG` prefix) or number to its number.
fn sig_from_spec(s: &str) -> Option<i32> {
    let t = s.trim();
    if let Ok(n) = t.parse::<i32>() {
        return Some(n);
    }
    let u = t.to_ascii_uppercase();
    let name = u.strip_prefix("SIG").unwrap_or(&u);
    Some(match name {
        "HUP" => 1,
        "INT" => 2,
        "QUIT" => 3,
        "ILL" => 4,
        "TRAP" => 5,
        "ABRT" | "IOT" => 6,
        "BUS" => 7,
        "FPE" => 8,
        "KILL" => 9,
        "USR1" => 10,
        "SEGV" => 11,
        "USR2" => 12,
        "PIPE" => 13,
        "ALRM" => 14,
        "TERM" => 15,
        "STKFLT" => 16,
        "CHLD" | "CLD" => 17,
        "CONT" => 18,
        "STOP" => 19,
        "TSTP" => 20,
        "TTIN" => 21,
        "TTOU" => 22,
        "URG" => 23,
        "XCPU" => 24,
        "XFSZ" => 25,
        "VTALRM" => 26,
        "PROF" => 27,
        "WINCH" => 28,
        "IO" | "POLL" => 29,
        "PWR" => 30,
        "SYS" => 31,
        _ => return None,
    })
}

type NtProcFn = unsafe extern "system" fn(HANDLE) -> i32;

/// Resolve an ntdll entry point (`NtSuspendProcess` / `NtResumeProcess`).
fn ntdll_proc(name: &[u8]) -> Option<NtProcFn> {
    unsafe {
        let lib = LoadLibraryA(b"ntdll.dll\0".as_ptr());
        if lib.is_null() {
            return None;
        }
        let proc = GetProcAddress(lib, name.as_ptr());
        proc.map(|p| std::mem::transmute::<_, NtProcFn>(p))
    }
}

/// Deliver `sig` to Win32 pid `pid`. Returns `Ok(())` or an error string.
fn deliver(pid: u32, sig: i32) -> Result<(), String> {
    // Signal 0: existence check only.
    if sig == 0 {
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            return Err("No such process".into());
        }
        unsafe { CloseHandle(h) };
        return Ok(());
    }

    // SIGSTOP / SIGTSTP and SIGCONT: whole-process suspend/resume via ntdll.
    if sig == 19 || sig == 20 || sig == 18 {
        let entry: &[u8] = if sig == 18 {
            b"NtResumeProcess\0"
        } else {
            b"NtSuspendProcess\0"
        };
        let Some(func) = ntdll_proc(entry) else {
            return Err("suspend/resume unavailable".into());
        };
        let h = unsafe { OpenProcess(PROCESS_SUSPEND_RESUME, 0, pid) };
        if h.is_null() {
            return Err("No such process".into());
        }
        let status = unsafe { func(h) };
        unsafe { CloseHandle(h) };
        if status < 0 {
            return Err("operation failed".into());
        }
        return Ok(());
    }

    // Every other signal is fatal on Windows (there is no way to deliver a real
    // POSIX signal to an arbitrary process), so terminate it. The exit code
    // follows the POSIX convention of 128 + signal number.
    let h = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if h.is_null() {
        return Err("No such process".into());
    }
    let ok = unsafe { TerminateProcess(h, (128 + sig) as u32) };
    unsafe { CloseHandle(h) };
    if ok == 0 {
        return Err("operation not permitted".into());
    }
    Ok(())
}

const SIGNAL_NAMES: &[&str] = &[
    "HUP", "INT", "QUIT", "ILL", "TRAP", "ABRT", "BUS", "FPE", "KILL", "USR1", "SEGV", "USR2",
    "PIPE", "ALRM", "TERM", "STKFLT", "CHLD", "CONT", "STOP", "TSTP", "TTIN", "TTOU", "URG",
    "XCPU", "XFSZ", "VTALRM", "PROF", "WINCH", "IO", "PWR", "SYS",
];

fn usage_error(msg: &str) -> ! {
    eprintln!("kill: {msg}");
    exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut sig: i32 = 15; // default SIGTERM
    let mut pids: Vec<String> = Vec::new();
    let mut i = 0;
    let mut seen_dashdash = false;

    while i < args.len() {
        let a = &args[i];
        if seen_dashdash || !a.starts_with('-') || a == "-" {
            pids.push(a.clone());
            i += 1;
            continue;
        }
        if a == "--" {
            seen_dashdash = true;
            i += 1;
            continue;
        }
        if a == "-l" || a == "--list" {
            // List signal names (numbered from 1).
            for (n, name) in SIGNAL_NAMES.iter().enumerate() {
                print!("{:2}) SIG{}  ", n + 1, name);
                if (n + 1) % 5 == 0 {
                    println!();
                }
            }
            println!();
            exit(0);
        }
        if a == "-s" || a == "-n" || a == "--signal" {
            i += 1;
            let Some(spec) = args.get(i) else {
                usage_error("option requires an argument");
            };
            match sig_from_spec(spec) {
                Some(s) => sig = s,
                None => usage_error(&format!("invalid signal: {spec}")),
            }
            i += 1;
            continue;
        }
        // -<sigspec>: -9, -KILL, -SIGKILL, -s... handled above.
        let spec = &a[1..];
        match sig_from_spec(spec) {
            Some(s) => sig = s,
            None => usage_error(&format!("invalid signal: {spec}")),
        }
        i += 1;
    }

    if pids.is_empty() {
        usage_error("usage: kill [-s sigspec | -signum | -sigspec] pid...");
    }

    let mut had_error = false;
    for pidstr in &pids {
        // A negative pid is a process group in POSIX; fishbowl job groups are keyed
        // on the leader's Win32 pid, so target its absolute value.
        let parsed = pidstr.trim_start_matches('-').parse::<u32>();
        let Ok(pid) = parsed else {
            eprintln!("kill: invalid pid: {pidstr}");
            had_error = true;
            continue;
        };
        if let Err(e) = deliver(pid, sig) {
            eprintln!("kill: ({pid}) - {e}");
            had_error = true;
        }
    }
    exit(if had_error { 1 } else { 0 });
}
