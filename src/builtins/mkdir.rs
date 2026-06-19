//! Native `mkdir` builtin for fishbowl's Windows test/setup hot path.

use std::fs;

use super::prelude::*;
use crate::err_fmt;
use fish_widestring::wcs2osstring;

#[derive(Default)]
struct Options {
    parents: bool,
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
            ['-', 'p'] => {
                opts.parents = true;
                i += 1;
            }
            ['-', 'm'] => {
                // Windows has no POSIX mode bits in fishbowl's noacl model; accept
                // and ignore the mode operand for compatibility with setup code.
                if i + 1 >= args.len() {
                    err_fmt!("option requires an argument -- m").cmd(cmd).finish(streams);
                    return Err(STATUS_INVALID_ARGS);
                }
                i += 2;
            }
            ['-', flags @ ..] if flags.contains(&'p') => {
                // Accept compact forms that include p (rare, but cheap).
                opts.parents = true;
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

pub fn mkdir(_parser: &mut Parser, streams: &mut IoStreams, args: &mut [&wstr]) -> BuiltinResult {
    let cmd = args[0];
    let (opts, first_path) = parse_options(args, streams)?;
    if first_path == args.len() {
        err_fmt!("missing operand").cmd(cmd).finish(streams);
        return Err(STATUS_INVALID_ARGS);
    }

    let mut failed = false;
    for &path in &args[first_path..] {
        let native = posix_rt::pathconv::posix_to_win(wcs2osstring(path));
        let res = if opts.parents {
            fs::create_dir_all(&native)
        } else {
            fs::create_dir(&native)
        };
        if let Err(e) = res {
            err_fmt!("%s: %s", path, e).cmd(cmd).finish(streams);
            failed = true;
        }
    }

    if failed { Err(STATUS_CMD_ERROR) } else { Ok(SUCCESS) }
}
