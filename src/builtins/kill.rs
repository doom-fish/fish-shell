// Implementation of the kill builtin.
//
// On Windows there is no system `kill(1)` that understands native process ids, so fish
// delivers signals itself: `nix::sys::signal::kill` routes through the posix-signal engine,
// which maps SIGKILL/SIGTERM to `TerminateProcess`, SIGSTOP/SIGTSTP to a real cross-process
// suspend, and SIGCONT to resume. The `kill` function in share/functions expands `%n` job
// specifiers to pids before handing off to this builtin.

use super::prelude::*;
use crate::proc::ProcStatus;
use crate::signal::RawSignal;
use nix::sys::signal::{Signal as NixSignal, kill as nix_kill};
use nix::unistd::Pid as NixPid;

/// Translate a parsed [`RawSignal`] into the engine signal type, if it denotes a signal the
/// runtime can model.
fn to_nix_signal(rs: RawSignal) -> Option<NixSignal> {
    NixSignal::from_raw(rs.code())
}

/// Reflect a stop/continue we just delivered into the owning job's state.
///
/// On Windows a `SIGSTOP`/`SIGCONT` delivered to a child produces no `waitpid`-observable
/// status (and no `SIGCHLD`), so the reaper never learns the job stopped or resumed. When
/// fish itself is the one delivering the signal we therefore update the matching process'
/// state directly, mirroring what `handle_child_status` would do on POSIX. This is what lets
/// `jobs`/`disown` see a job that was stopped via `kill -SIGSTOP`.
fn reflect_stop_state(parser: &mut Parser, pid: i32, signal: NixSignal) {
    let (stopped, status) = match signal {
        NixSignal::SIGSTOP | NixSignal::SIGTSTP => {
            (true, ProcStatus::from_waitpid((signal.as_raw() << 8) | 0x7f))
        }
        NixSignal::SIGCONT => (false, ProcStatus::from_waitpid(0xffff)),
        _ => return,
    };
    for job in parser.jobs().iter() {
        let mut matched = false;
        for proc in job.processes().iter() {
            if proc.pid().is_some_and(|p| p.as_pid_t() == pid) {
                proc.status.set(status);
                proc.stopped.store(stopped);
                matched = true;
            }
        }
        if matched && stopped {
            job.group().set_is_foreground(false);
        }
    }
}

/// Print the list of signal names, or translate the given signal specifications.
fn builtin_kill_list(
    streams: &mut IoStreams,
    cmd: &wstr,
    specs: &[&wstr],
) -> BuiltinResult {
    if specs.is_empty() {
        // List every signal the runtime models.
        let mut names: Vec<&'static wstr> = Vec::new();
        for code in 1..=31 {
            let name = RawSignal::new(code).name();
            if name != L!("Unknown") {
                names.push(name);
            }
        }
        let joined: WString = {
            let mut out = WString::new();
            for (i, n) in names.iter().enumerate() {
                if i != 0 {
                    out.push(' ');
                }
                out.push_utfstr(n);
            }
            out
        };
        streams.out.appendln(&joined);
        return Ok(SUCCESS);
    }

    let mut retval = Ok(SUCCESS);
    for spec in specs {
        match RawSignal::parse(spec) {
            Some(rs) => {
                // If the spec was numeric, print the name; otherwise print the number.
                if fish_wcstoi(spec).is_ok() {
                    streams.out.appendln(rs.name());
                } else {
                    streams.out.appendln(&rs.code().to_wstring());
                }
            }
            None => {
                streams
                    .err
                    .appendln(&wgettext_fmt!("%ls: %ls: unknown signal", cmd, spec));
                retval = Err(STATUS_INVALID_ARGS);
            }
        }
    }
    retval
}

/// Builtin for sending signals to processes.
pub fn kill(parser: &mut Parser, streams: &mut IoStreams, args: &mut [&wstr]) -> BuiltinResult {
    let cmd = args[0];
    let argc = args.len();

    let mut signal = NixSignal::SIGTERM;
    let mut list = false;
    let mut i = 1;

    // Parse leading options. A signal may be given as `-s SIG`, `-SIG`, or `-NUM`. Listing is
    // requested with `-l`/`--list`. `--` ends option parsing.
    while i < argc {
        let arg = args[i];
        if arg == L!("--") {
            i += 1;
            break;
        }
        if arg == L!("-l") || arg == L!("--list") || arg == L!("-L") {
            list = true;
            i += 1;
            break;
        }
        if arg == L!("-s") || arg == L!("--signal") {
            if i + 1 >= argc {
                streams
                    .err
                    .appendln(&wgettext_fmt!("%ls: -s requires a signal name", cmd));
                return Err(STATUS_INVALID_ARGS);
            }
            match RawSignal::parse(args[i + 1]).and_then(to_nix_signal) {
                Some(s) => signal = s,
                None => {
                    streams.err.appendln(&wgettext_fmt!(
                        "%ls: %ls: unknown signal",
                        cmd,
                        args[i + 1]
                    ));
                    return Err(STATUS_INVALID_ARGS);
                }
            }
            i += 2;
            break;
        }
        // `-SIG` / `-NUM` (e.g. -9, -KILL, -SIGKILL). Only treat as a signal if it names a
        // signal the runtime models; otherwise fall through and treat it as an operand.
        if arg.char_at(0) == '-' && arg.char_count() > 1 {
            let rest = arg.slice_from(1);
            if let Some(s) = RawSignal::parse(rest).and_then(to_nix_signal) {
                signal = s;
                i += 1;
                break;
            }
            // Unknown signal spec beginning with '-'.
            streams
                .err
                .appendln(&wgettext_fmt!("%ls: %ls: unknown signal", cmd, rest));
            return Err(STATUS_INVALID_ARGS);
        }
        break;
    }

    let operands = &args[i..];

    if list {
        return builtin_kill_list(streams, cmd, operands);
    }

    if operands.is_empty() {
        streams
            .err
            .appendln(&wgettext_fmt!("%ls: expected at least one process id", cmd));
        return Err(STATUS_INVALID_ARGS);
    }

    let mut retval = Ok(SUCCESS);
    for arg in operands {
        let pid = match fish_wcstoi(arg) {
            Ok(pid) => pid,
            Err(_) => {
                streams
                    .err
                    .appendln(&wgettext_fmt!("%ls: '%ls' is not a valid process id", cmd, arg));
                retval = Err(STATUS_INVALID_ARGS);
                continue;
            }
        };
        if nix_kill(NixPid::from_raw(pid), signal).is_err() {
            streams.err.appendln(&wgettext_fmt!(
                "%ls: Failed to send signal to job %d",
                cmd,
                pid
            ));
            retval = Err(STATUS_CMD_ERROR);
        } else {
            reflect_stop_state(parser, pid, signal);
            #[cfg(windows)]
            if pid == 0 || pid == posix_rt::getpid() as i32 {
                if let Some(sig) = posix_signal::Signal::from_raw(signal as i32) {
                    let _ = posix_signal::dispatch();
                    crate::event::fire_delayed(parser);
                    if posix_signal::default_actions().contains(sig) {
                        std::process::exit(128 + sig.as_raw());
                    }
                }
            }
        }
    }

    retval
}
