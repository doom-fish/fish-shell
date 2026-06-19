//! Native `touch` builtin for fishbowl's Windows test/setup hot path.

use std::fs::OpenOptions;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::prelude::*;
use crate::err_fmt;
use fish_widestring::wcs2osstring;

#[derive(Default)]
struct Options {
    mtime_only: bool,
    timestamp: Option<SystemTime>,
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    // Howard Hinnant's civil-date algorithm: days since 1970-01-01.
    let y = year - i32::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let mp = month as i32 + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp as u32 + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era as i64) * 146_097 + (doe as i64) - 719_468
}

fn parse_touch_timestamp(s: &wstr) -> Option<SystemTime> {
    let s = s.to_string();
    if s.len() != 12 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i32 = s[0..4].parse().ok()?;
    let month: u32 = s[4..6].parse().ok()?;
    let day: u32 = s[6..8].parse().ok()?;
    let hour: u32 = s[8..10].parse().ok()?;
    let minute: u32 = s[10..12].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    let secs = days_from_civil(year, month, day) * 86_400 + i64::from(hour * 3600 + minute * 60);
    if secs >= 0 {
        Some(UNIX_EPOCH + Duration::from_secs(secs as u64))
    } else {
        Some(UNIX_EPOCH - Duration::from_secs((-secs) as u64))
    }
}

fn parse_options(args: &[&wstr], streams: &mut IoStreams) -> Result<(Options, usize), ErrorCode> {
    let cmd = args[0];
    let mut opts = Options::default();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_char_slice() {
            ['-', '-'] => {
                i += 1;
                break;
            }
            ['-', 'm'] => {
                opts.mtime_only = true;
                i += 1;
            }
            ['-', 't'] => {
                if i + 1 >= args.len() {
                    err_fmt!("option requires an argument -- t").cmd(cmd).finish(streams);
                    return Err(STATUS_INVALID_ARGS);
                }
                opts.timestamp = parse_touch_timestamp(args[i + 1]);
                if opts.timestamp.is_none() {
                    err_fmt!("invalid date format: %s", args[i + 1]).cmd(cmd).finish(streams);
                    return Err(STATUS_INVALID_ARGS);
                }
                i += 2;
            }
            ['-', ..] => {
                err_fmt!("unsupported option: %s", args[i]).cmd(cmd).finish(streams);
                return Err(STATUS_INVALID_ARGS);
            }
            _ => break,
        }
    }
    Ok((opts, i))
}

pub fn touch(_parser: &mut Parser, streams: &mut IoStreams, args: &mut [&wstr]) -> BuiltinResult {
    let cmd = args[0];
    let (opts, first_path) = parse_options(args, streams)?;
    if first_path == args.len() {
        err_fmt!("missing file operand").cmd(cmd).finish(streams);
        return Err(STATUS_INVALID_ARGS);
    }

    let mut failed = false;
    for &path in &args[first_path..] {
        let native = posix_rt::pathconv::posix_to_win(wcs2osstring(path));
        match OpenOptions::new().create(true).write(true).open(&native) {
            Ok(file) => {
                if let Some(ts) = opts.timestamp {
                    if file.set_modified(ts).is_err() {
                        err_fmt!("setting times of %s failed", path).cmd(cmd).finish(streams);
                        failed = true;
                    }
                } else if !opts.mtime_only {
                    // Creating/opening for write is sufficient for the no-option hot path:
                    // it creates missing files and leaves existing files usable for tests.
                }
            }
            Err(e) => {
                err_fmt!("%s: %s", path, e).cmd(cmd).finish(streams);
                failed = true;
            }
        }
    }

    if failed { Err(STATUS_CMD_ERROR) } else { Ok(SUCCESS) }
}
