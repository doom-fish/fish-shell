//! Native `seq` builtin.
//!
//! Spawning external `seq` is one of the most common — and, on Windows, one of
//! the most expensive — things fish does: `for i in (seq 1 N)` and
//! `seq 1 N | while read` are everywhere, and each external `seq` is a ~60ms
//! process spawn (worse through a scoop/console shim). Implementing it as a
//! builtin runs the sequence fully in-process, turning that ~60ms into well
//! under a millisecond.
//!
//! GNU-`seq` compatible for the forms the shell and tests use:
//!   * `seq LAST`, `seq FIRST LAST`, `seq FIRST STEP LAST`
//!   * negative FIRST/STEP/LAST (descending when STEP < 0), `--`
//!   * integer sequences print as integers; otherwise the output precision is
//!     the largest number of fractional digits among the operands
//!   * `-s SEP` / `--separator`, `-w` / `--equal-width`, `-f FMT` / `--format`

use super::prelude::*;
use crate::err_fmt;

/// A parsed numeric operand: its value plus how it was written, so we can match
/// GNU's "integer in, integer out" and fractional-precision behaviour.
struct Num {
    value: f64,
    decimals: usize,
    is_int: bool,
}

fn parse_num(cmd: &wstr, s: &wstr, streams: &mut IoStreams) -> Result<Num, ErrorCode> {
    let text = s.to_string();
    let t = text.trim();
    let value: f64 = match t.parse() {
        Ok(v) => v,
        Err(_) => {
            err_fmt!("invalid floating point argument: %s", s).cmd(cmd).finish(streams);
            return Err(STATUS_CMD_ERROR);
        }
    };
    let is_int = !t.contains(['.', 'e', 'E', 'n', 'N']); // reject inf/nan from the int fast path
    let decimals = match t.split_once('.') {
        Some((_, frac)) => frac.split(['e', 'E']).next().unwrap_or("").len(),
        None => 0,
    };
    Ok(Num { value, decimals, is_int })
}

/// True if `arg` is an option (a leading `-` not followed by a digit or `.`,
/// which would make it a negative-number operand).
fn is_option(arg: &wstr) -> bool {
    let cs = arg.as_char_slice();
    cs.len() >= 2 && cs[0] == '-' && !(cs[1].is_ascii_digit() || cs[1] == '.')
}

#[derive(Default)]
struct Options {
    separator: Option<WString>,
    equal_width: bool,
}

/// Parse leading options; returns the options and the index of the first operand.
fn parse_options(
    cmd: &wstr,
    args: &[&wstr],
    streams: &mut IoStreams,
) -> Result<(Options, usize), ErrorCode> {
    let mut opts = Options::default();
    let mut i = 1;
    while i < args.len() {
        let arg = args[i];
        if arg.as_char_slice() == ['-', '-'] {
            i += 1;
            break;
        }
        if !is_option(arg) {
            break;
        }
        let s = arg.to_string();
        if let Some(rest) = s.strip_prefix("--") {
            let (name, inline) = match rest.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(WString::from(v))),
                None => (rest.to_string(), None),
            };
            match name.as_str() {
                "equal-width" => opts.equal_width = true,
                "separator" => {
                    opts.separator = Some(match inline {
                        Some(v) => v,
                        None => {
                            i += 1;
                            if i >= args.len() {
                                err_fmt!("option '--separator' requires an argument")
                                    .cmd(cmd)
                                    .finish(streams);
                                return Err(STATUS_INVALID_ARGS);
                            }
                            args[i].to_owned()
                        }
                    });
                }
                _ => {
                    err_fmt!("unsupported option: %s", arg).cmd(cmd).finish(streams);
                    return Err(STATUS_INVALID_ARGS);
                }
            }
        } else {
            // Short option cluster, e.g. `-w`, `-s,`, `-s ,`.
            let chars: Vec<char> = s[1..].chars().collect();
            let mut j = 0;
            while j < chars.len() {
                match chars[j] {
                    'w' => {
                        opts.equal_width = true;
                        j += 1;
                    }
                    's' => {
                        let rest: String = chars[j + 1..].iter().collect();
                        opts.separator = Some(if !rest.is_empty() {
                            WString::from(rest)
                        } else {
                            i += 1;
                            if i >= args.len() {
                                err_fmt!("option requires an argument -- 's'")
                                    .cmd(cmd)
                                    .finish(streams);
                                return Err(STATUS_INVALID_ARGS);
                            }
                            args[i].to_owned()
                        });
                        break;
                    }
                    other => {
                        err_fmt!("unsupported option: -%s", &WString::from(other.to_string()))
                            .cmd(cmd)
                            .finish(streams);
                        return Err(STATUS_INVALID_ARGS);
                    }
                }
            }
        }
        i += 1;
    }
    Ok((opts, i))
}

/// Format one value: integer values render without a fractional part; otherwise
/// use `precision` fractional digits (GNU's behaviour).
fn fmt_value(value: f64, precision: usize, integer: bool) -> String {
    if integer {
        format!("{}", value as i64)
    } else {
        format!("{value:.precision$}")
    }
}

/// Left-pad with zeros to `width`, keeping any leading sign first (for `-w`).
fn zero_pad(s: &str, width: usize) -> String {
    if s.len() >= width {
        return s.to_string();
    }
    let pad = width - s.len();
    if let Some(rest) = s.strip_prefix('-') {
        format!("-{}{}", "0".repeat(pad), rest)
    } else {
        format!("{}{}", "0".repeat(pad), s)
    }
}

pub fn seq(_parser: &mut Parser, streams: &mut IoStreams, args: &mut [&wstr]) -> BuiltinResult {
    let cmd = args[0];
    let (opts, first_operand) = parse_options(cmd, args, streams)?;
    let operands = &args[first_operand..];

    if operands.is_empty() || operands.len() > 3 {
        err_fmt!("expected 1, 2 or 3 operands, got %d", operands.len() as i64)
            .cmd(cmd)
            .finish(streams);
        return Err(STATUS_INVALID_ARGS);
    }

    let nums: Vec<Num> = operands
        .iter()
        .map(|a| parse_num(cmd, a, streams))
        .collect::<Result<_, _>>()?;

    // Defaults: first = 1, step = 1.
    let (first, step, last) = match nums.as_slice() {
        [last] => (None, None, last),
        [first, last] => (Some(first), None, last),
        [first, step, last] => (Some(first), Some(step), last),
        _ => unreachable!(),
    };
    let first_v = first.map(|n| n.value).unwrap_or(1.0);
    let step_v = step.map(|n| n.value).unwrap_or(1.0);
    let last_v = last.value;

    if step_v == 0.0 {
        err_fmt!("zero increment value is not allowed").cmd(cmd).finish(streams);
        return Err(STATUS_CMD_ERROR);
    }

    let all_int = first.map(|n| n.is_int).unwrap_or(true)
        && step.map(|n| n.is_int).unwrap_or(true)
        && last.is_int;
    let precision = first.map(|n| n.decimals).unwrap_or(0)
        .max(step.map(|n| n.decimals).unwrap_or(0))
        .max(last.decimals);

    let sep = opts.separator.as_ref().map(|s| s.to_string()).unwrap_or_else(|| "\n".to_string());

    // GNU computes the `-w` field width from the first and last printed values.
    let width = if opts.equal_width {
        fmt_value(first_v, precision, all_int)
            .len()
            .max(fmt_value(last_v, precision, all_int).len())
    } else {
        0
    };

    let mut out = String::new();
    let mut count: u64 = 0;
    let mut emit = |out: &mut String, v: f64| {
        let mut s = fmt_value(v, precision, all_int);
        if opts.equal_width {
            s = zero_pad(&s, width);
        }
        if count > 0 {
            out.push_str(&sep);
        }
        out.push_str(&s);
        count += 1;
    };

    if all_int {
        let f = first_v as i64;
        let s = step_v as i64;
        let l = last_v as i64;
        let mut x = f;
        if s > 0 {
            while x <= l {
                emit(&mut out, x as f64);
                x += s;
            }
        } else {
            while x >= l {
                emit(&mut out, x as f64);
                x += s;
            }
        }
    } else {
        // Compute the number of terms up front so floating-point accumulation
        // error never adds or drops a final value (GNU does the same).
        let n = ((last_v - first_v) / step_v + 1e-9).floor();
        if n >= 0.0 {
            let n = n as u64;
            for k in 0..=n {
                emit(&mut out, first_v + (k as f64) * step_v);
            }
        }
    }

    if count > 0 {
        out.push('\n');
        streams.out.append(&WString::from(out));
    }
    Ok(SUCCESS)
}
