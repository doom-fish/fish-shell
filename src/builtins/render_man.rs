//! Native `__fish_render_man` builtin.
//!
//! Renders a roff man page (the docutils rst2man subset fish generates) to plain
//! text using the in-process [`manfmt`] crate, instead of shelling out to
//! `man`/`groff`/`nroff`. This removes fish's last hard runtime dependency on an
//! external man pipeline (and, on Windows, on MSYS) for showing builtin `--help`
//! and its own man pages, and is roughly an order of magnitude faster than
//! spawning `man`.
//!
//! Usage: `__fish_render_man FILE` — render FILE; width comes from `$MANWIDTH`,
//! else `$COLUMNS`, else 80.

use super::prelude::*;
use crate::env::Environment as _;
use crate::err_fmt;
use fish_widestring::{str2wcstring, wcs2osstring};

fn width_from_vars(parser: &Parser) -> usize {
    for name in [L!("MANWIDTH"), L!("COLUMNS")] {
        if let Some(v) = parser.vars().get(name) {
            if let Ok(w) = v.as_string().to_string().trim().parse::<usize>() {
                if w > 0 {
                    return w;
                }
            }
        }
    }
    80
}

pub fn render_man(parser: &mut Parser, streams: &mut IoStreams, argv: &mut [&wstr]) -> BuiltinResult {
    let cmd = argv[0];
    if argv.len() != 2 {
        err_fmt!("expected a single man-page path").cmd(cmd).finish(streams);
        return Err(STATUS_INVALID_ARGS);
    }
    let path = argv[1];

    // fish paths are POSIX (`/c/...`) on Windows; convert to a native path for
    // the actual filesystem read. On other platforms the path is used as-is.
    #[cfg(windows)]
    let native = posix_rt::pathconv::posix_to_win(wcs2osstring(path));
    #[cfg(not(windows))]
    let native = wcs2osstring(path);

    let bytes = match std::fs::read(&native) {
        Ok(b) => b,
        Err(e) => {
            err_fmt!("%s: %s", path, e).cmd(cmd).finish(streams);
            return Err(STATUS_CMD_ERROR);
        }
    };
    let src = String::from_utf8_lossy(&bytes);
    let width = width_from_vars(parser);
    let rendered = manfmt::render(&src, width);
    streams.out.append(&str2wcstring(&rendered));
    Ok(SUCCESS)
}
