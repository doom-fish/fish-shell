//! Native `mktemp` builtin for fishbowl's Windows test/setup hot path.

use std::fs::{self, OpenOptions};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::prelude::*;
use crate::err_fmt;

#[derive(Default)]
struct Options {
    directory: bool,
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
            ['-', 'd'] => {
                opts.directory = true;
                i += 1;
            }
            ['-', flags @ ..] if flags.contains(&'d') => {
                opts.directory = true;
                i += 1;
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

fn next_suffix(len: usize) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut state = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        ^ ((std::process::id() as u64) << 32)
        ^ COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let v = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        out.push(ALPHABET[(v as usize) % ALPHABET.len()] as char);
    }
    out
}

fn instantiate_template(template: &str) -> Option<String> {
    let end = template.len();
    let start = template
        .as_bytes()
        .iter()
        .rposition(|&b| b != b'X')
        .map(|i| i + 1)
        .unwrap_or(0);
    let count = end - start;
    if count == 0 {
        return None;
    }
    let mut out = String::with_capacity(template.len());
    out.push_str(&template[..start]);
    out.push_str(&next_suffix(count));
    Some(out)
}

pub fn mktemp(_parser: &mut Parser, streams: &mut IoStreams, args: &mut [&wstr]) -> BuiltinResult {
    let cmd = args[0];
    let (opts, first_template) = parse_options(args, streams)?;
    if first_template + 1 < args.len() {
        err_fmt!("too many templates").cmd(cmd).finish(streams);
        return Err(STATUS_INVALID_ARGS);
    }
    let template = if first_template < args.len() {
        args[first_template].to_string()
    } else if opts.directory {
        "/tmp/tmp.XXXXXXXXXX".to_string()
    } else {
        "/tmp/tmp.XXXXXXXXXX".to_string()
    };

    for _ in 0..256 {
        let Some(candidate) = instantiate_template(&template) else {
            err_fmt!("too few X's in template: %s", &template).cmd(cmd).finish(streams);
            return Err(STATUS_INVALID_ARGS);
        };
        let native = posix_rt::pathconv::posix_to_win(&candidate);
        let res = if opts.directory {
            fs::create_dir(&native)
        } else {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&native)
                .map(|_| ())
        };
        match res {
            Ok(()) => {
                streams.out.appendln(&WString::from(candidate));
                return Ok(SUCCESS);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                err_fmt!("%s: %s", &template, e).cmd(cmd).finish(streams);
                return Err(STATUS_CMD_ERROR);
            }
        }
    }
    err_fmt!("failed to create a unique temporary path")
        .cmd(cmd)
        .finish(streams);
    Err(STATUS_CMD_ERROR)
}
