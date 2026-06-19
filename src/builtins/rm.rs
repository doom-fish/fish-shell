//! Native `rm` builtin for fishbowl's Windows test/setup hot path.

use std::fs;

use super::prelude::*;
use crate::err_fmt;
use fish_widestring::wcs2osstring;

#[derive(Default)]
struct Options {
    force: bool,
    recursive: bool,
}

fn parse_options(args: &[&wstr], streams: &mut IoStreams) -> Result<(Options, usize), ErrorCode> {
    let cmd = args[0];
    let mut opts = Options::default();
    let mut i = 1;
    while i < args.len() {
        let chars = args[i].as_char_slice();
        match chars {
            ['-', '-'] => {
                i += 1;
                break;
            }
            ['-', flags @ ..] if !flags.is_empty() => {
                for &c in flags {
                    match c {
                        'f' => opts.force = true,
                        'r' | 'R' => opts.recursive = true,
                        _ => {
                            err_fmt!("unsupported option: %s", args[i]).cmd(cmd).finish(streams);
                            return Err(STATUS_INVALID_ARGS);
                        }
                    }
                }
                i += 1;
            }
            _ => break,
        }
    }
    Ok((opts, i))
}

pub fn rm(_parser: &mut Parser, streams: &mut IoStreams, args: &mut [&wstr]) -> BuiltinResult {
    let cmd = args[0];
    let (opts, first_path) = parse_options(args, streams)?;
    if first_path == args.len() {
        if opts.force {
            return Ok(SUCCESS);
        }
        err_fmt!("missing operand").cmd(cmd).finish(streams);
        return Err(STATUS_INVALID_ARGS);
    }

    let mut failed = false;
    for &path in &args[first_path..] {
        let native = posix_rt::pathconv::posix_to_win(wcs2osstring(path));
        let md = fs::symlink_metadata(&native);
        let res = match md {
            Ok(md) if md.is_dir() && opts.recursive => fs::remove_dir_all(&native),
            Ok(md) if md.is_dir() => fs::remove_dir(&native),
            Ok(_) => fs::remove_file(&native),
            Err(e) => {
                if opts.force && e.kind() == std::io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        };
        if let Err(e) = res {
            if e.kind() == std::io::ErrorKind::NotFound || matches!(e.raw_os_error(), Some(2 | 3)) {
                continue;
            }
            err_fmt!("%s: %s", path, e).cmd(cmd).finish(streams);
            failed = true;
        }
    }

    if failed { Err(STATUS_CMD_ERROR) } else { Ok(SUCCESS) }
}
