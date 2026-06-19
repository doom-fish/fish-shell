//! Native `sleep` builtin.
//!
//! `sleep` is one of the most frequently spawned external commands — the tmux
//! tests' `tmux-sleep` (`sleep 0.3`) alone accounts for hundreds of spawns, each
//! a ~60ms Windows process launch on top of the requested delay. Running it
//! in-process removes that per-call spawn overhead while staying interruptible
//! (it polls fish's cancel flag, so Ctrl-C still works).
//!
//! Backgrounding caveat: a backgrounded builtin has no real OS pid on Windows
//! (fish has no fork), which would break `sleep N &` (job control needs a pid).
//! The exec layer special-cases blocking builtins: when one is backgrounded it
//! is launched as a `fish -c` subprocess instead (see exec.rs), so `sleep N &`
//! still becomes a real, stoppable, waitable job. Foreground `sleep` runs here,
//! in-process.
//!
//! GNU-`sleep` compatible: one or more `NUMBER[smhd]` operands (seconds /
//! minutes / hours / days, default seconds), summed.

use std::time::{Duration, Instant};

use super::prelude::*;
use crate::err_fmt;
use crate::signal::signal_check_cancel;

/// Parse a single `NUMBER[smhd]` interval into seconds.
fn parse_interval(arg: &wstr) -> Option<f64> {
    let s = arg.to_string();
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let (num, mult) = match t.as_bytes()[t.len() - 1] {
        b's' => (&t[..t.len() - 1], 1.0),
        b'm' => (&t[..t.len() - 1], 60.0),
        b'h' => (&t[..t.len() - 1], 3600.0),
        b'd' => (&t[..t.len() - 1], 86400.0),
        _ => (t, 1.0),
    };
    let v: f64 = num.parse().ok()?;
    if v.is_finite() && v >= 0.0 {
        Some(v * mult)
    } else {
        None
    }
}

pub fn sleep(_parser: &mut Parser, streams: &mut IoStreams, args: &mut [&wstr]) -> BuiltinResult {
    let cmd = args[0];
    let mut operands = &args[1..];
    if operands.first().map(|a| a.as_char_slice()) == Some(['-', '-'].as_slice()) {
        operands = &operands[1..];
    }
    if operands.is_empty() {
        err_fmt!("missing operand").cmd(cmd).finish(streams);
        return Err(STATUS_INVALID_ARGS);
    }

    let mut total = 0.0;
    for &arg in operands {
        match parse_interval(arg) {
            Some(v) => total += v,
            None => {
                err_fmt!("invalid time interval: %s", arg).cmd(cmd).finish(streams);
                return Err(STATUS_INVALID_ARGS);
            }
        }
    }

    let deadline = Instant::now() + Duration::from_secs_f64(total);
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        if signal_check_cancel() != 0 {
            // Interrupted (e.g. Ctrl-C); stop early and let fish handle the cancel.
            break;
        }
        std::thread::sleep((deadline - now).min(Duration::from_millis(50)));
    }
    Ok(SUCCESS)
}
