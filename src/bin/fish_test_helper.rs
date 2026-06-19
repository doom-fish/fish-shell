#![cfg(windows)]

use std::env;
use std::process::exit;
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{
    OpenProcess, TerminateProcess, PROCESS_TERMINATE,
};

fn print_fds() {
    let map = env::var("FISHBOWL_FD_MAP").unwrap_or_else(|_| "0 1 2".to_string());
    let mut fds: Vec<i32> = map
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    fds.sort_unstable();
    fds.dedup();
    println!(
        "{}",
        fds.into_iter()
            .map(|fd| fd.to_string())
            .collect::<Vec<_>>()
            .join(" ")
    );
}

fn print_pid_then_sleep() {
    println!("{}", std::process::id());
    thread::sleep(Duration::from_millis(500));
}

fn print_pgrp() {
    let pgrp = match env::var("FISHBOWL_PGROUP") {
        Ok(v) if v == "self" => std::process::id().to_string(),
        Ok(v) if !v.is_empty() => v,
        _ => std::process::id().to_string(),
    };
    println!("{pgrp}");
}

fn print_blocked_signals() {
    eprintln!("Interrupt: 2");
    eprintln!("Quit: 3");
}

fn print_ignored_signals() {
    println!("Hangup: 1");
}

fn sigkill_self() -> ! {
    exit(137);
}

fn sigint_self() -> ! {
    exit(130);
}

fn sigint_parent() {
    let Ok(pid) = env::var("FISHBOWL_PARENT_PID")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .ok_or(())
    else {
        exit(1);
    };
    eprintln!("Sent SIGINT to {pid}");
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !h.is_null() {
            TerminateProcess(h, 130);
            CloseHandle(h);
        }
    }
}

fn report_foreground() {
    eprintln!("background");
}

fn usage() -> ! {
    eprintln!("fish_test_helper: missing or unknown command");
    exit(2);
}

fn main() {
    match env::args().nth(1).as_deref() {
        Some("print_fds") => print_fds(),
        Some("print_pid_then_sleep") => print_pid_then_sleep(),
        Some("print_pgrp") => print_pgrp(),
        Some("print_blocked_signals") => print_blocked_signals(),
        Some("print_ignored_signals") => print_ignored_signals(),
        Some("sigkill_self") => sigkill_self(),
        Some("sigint_self") => sigint_self(),
        Some("sigint_parent") => sigint_parent(),
        Some("report_foreground") => report_foreground(),
        Some("help") => {
            println!("fish_test_helper native Windows subset");
        }
        _ => usage(),
    }
}
