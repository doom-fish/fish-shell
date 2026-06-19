pub mod dir_iter;
mod errors;
mod fileid;
mod hex_float;
#[macro_use]
pub mod printf;
pub mod wcstod;
pub mod wcstoi;

pub use errors::*;
pub use fileid::*;
pub use printf::{eprintf, fprintf, printf, sprintf};
pub use wcstoi::{
    fish_wcstoi, fish_wcstol, fish_wcstol_radix, fish_wcstoul, wcstoi, wcstoi_opts, wcstoi_partial,
};

use crate::{fds::BorrowedFdFile, signal::SigChecker};
use errno::{Errno, set_errno};
use fish_util::{perror, write_to_fd};
use fish_wcstringutil::join_strings;
use fish_widestring::{
    IntoCharIter, L, WExt as _, WString, bytes2wcstring, fish_reserved_codepoint,
    str2bytes_callback, wcs2osstring, wcs2zstring, wstr,
};
#[cfg(windows)]
use fish_widestring::str2wcstring;
#[cfg(not(windows))]
use fish_widestring::osstr2wcstring;
use nix::unistd::AccessFlags;
#[cfg(unix)]
use std::os::fd::RawFd;
#[cfg(windows)]
use osfd_win::RawFd;
use std::{
    ffi::OsStr,
    fs::{self, canonicalize},
    io,
};
#[cfg(unix)]
use std::os::unix::prelude::*;
#[cfg(windows)]
use osfd_win::prelude::*;

/// Wide character version of opendir(). Note that opendir() is guaranteed to set close-on-exec by
/// POSIX (hooray).
pub fn wopendir(name: &wstr) -> *mut libc::DIR {
    // libc::opendir is the C runtime's narrow opendir; translate fish's POSIX path to the native
    // Windows form so it can resolve `/c/...` directories (Cygwin/MSYS2 model).
    #[cfg(windows)]
    let tmp = {
        let native = posix_rt::pathconv::posix_to_win(wcs2osstring(name));
        wcs2zstring(&str2wcstring(native.to_string_lossy().as_ref()))
    };
    #[cfg(not(windows))]
    let tmp = wcs2zstring(name);
    unsafe { libc::opendir(tmp.as_ptr()) }
}

/// Wide character version of stat().
pub fn wstat(file_name: &wstr) -> io::Result<fs::Metadata> {
    fs::metadata(wcs2native_os(file_name))
}

/// Wide character version of lstat().
pub fn lwstat(file_name: &wstr) -> io::Result<fs::Metadata> {
    fs::symlink_metadata(wcs2native_os(file_name))
}

/// Cover over fstat().
pub fn fstat(fd: impl AsRawFd) -> io::Result<fs::Metadata> {
    let file = unsafe { BorrowedFdFile::from_raw_fd(fd.as_raw_fd()) };
    file.metadata()
}

/// Wide character version of access().
pub fn waccess(file_name: &wstr, amode: AccessFlags) -> nix::Result<()> {
    let tmp = wcs2native_os(file_name);
    nix::unistd::access(tmp.as_os_str(), amode)
}

/// File classification returned by [`wfast_stat`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileKind {
    Dir,
    Regular,
    Other,
}

/// Lightweight metadata for hot type/existence checks that must not pay for
/// file identity (highlighting, `test -f/-d/-e/-s`). See [`wfast_stat`].
#[derive(Clone, Copy, Debug)]
pub struct FastMeta {
    kind: FileKind,
    size: u64,
}

impl FastMeta {
    /// Whether the path is a directory.
    pub fn is_dir(&self) -> bool {
        self.kind == FileKind::Dir
    }
    /// Whether the path is a regular file.
    pub fn is_regular(&self) -> bool {
        self.kind == FileKind::Regular
    }
    /// Size in bytes.
    pub fn len(&self) -> u64 {
        self.size
    }
}

fn fastmeta_from_metadata(md: &fs::Metadata) -> FastMeta {
    let ft = md.file_type();
    let kind = if ft.is_dir() {
        FileKind::Dir
    } else if ft.is_file() {
        FileKind::Regular
    } else {
        FileKind::Other
    };
    FastMeta {
        kind,
        size: md.len(),
    }
}

/// Fast `stat`-like probe returning only the file *kind* and size, following
/// symlinks like [`wstat`].
///
/// On Windows this skips the file-handle open (and the per-open antivirus /
/// minifilter scan it incurs) for plain files and directories, querying
/// attributes in a single syscall instead — measurably cheaper on the hot
/// highlighting / `test` paths. Reparse points (symlinks/junctions) and **any**
/// error fall back to the exact [`wstat`] path, so the observable result —
/// including the precise `io::Error` callers branch on — is unchanged.
pub fn wfast_stat(file_name: &wstr) -> io::Result<FastMeta> {
    #[cfg(windows)]
    {
        let native = wcs2native_os(file_name);
        if let Ok(attrs) = nix::sys::stat::attributes(native.as_os_str()) {
            if !attrs.is_reparse {
                // `attributes` only yields S_IFDIR / S_IFREG for non-reparse
                // entries; anything else routes through the std fallback below.
                let kind = if attrs.st_mode & 0o170000 == 0o040000 {
                    FileKind::Dir
                } else {
                    FileKind::Regular
                };
                return Ok(FastMeta {
                    kind,
                    size: attrs.st_size,
                });
            }
        }
        // Reparse point or attribute-query error: defer to the exact std path,
        // which follows symlinks and surfaces the precise error.
        wstat(file_name).map(|md| fastmeta_from_metadata(&md))
    }
    #[cfg(not(windows))]
    {
        wstat(file_name).map(|md| fastmeta_from_metadata(&md))
    }
}

/// Wide character version of unlink().
pub fn wunlink(file_name: &wstr) -> io::Result<()> {
    fs::remove_file(wcs2native_os(file_name))
}

/// Convert a fish-internal POSIX path (`/c/Users/per`) to the OS-native form (`C:\Users\per`)
/// for direct `std::fs`/`std::env` boundary calls. This is the Cygwin/MSYS2 std-boundary
/// translation; on POSIX platforms it is the identity narrow conversion.
#[cfg(windows)]
fn wcs2native_os(path: &wstr) -> std::ffi::OsString {
    posix_rt::pathconv::posix_to_win(wcs2osstring(path)).into_os_string()
}

#[cfg(not(windows))]
fn wcs2native_os(path: &wstr) -> std::ffi::OsString {
    wcs2osstring(path)
}

/// Translate a *native* Windows path that a user typed (`C:/foo`, `C:\foo`, `\\server\share`)
/// into fish's POSIX view (`/c/foo`, `/unc/server/share`). Already-POSIX and relative inputs
/// are returned unchanged. This is the user-input boundary for the Cygwin/MSYS2 path model:
/// fish's path logic (CDPATH, `path_apply_working_directory`, normalization) is purely POSIX, so
/// native input must be converted before it reaches that logic, otherwise a drive-qualified path
/// like `C:/foo` (which has no leading `/`) is misread as relative to `$PWD`.
#[cfg(windows)]
pub fn native_input_to_posix(p: &wstr) -> WString {
    let cs = p.as_char_slice();
    let drive_abs = cs.len() >= 3
        && cs[1] == ':'
        && cs[0].is_ascii_alphabetic()
        && (cs[2] == '/' || cs[2] == '\\');
    let unc = cs.len() >= 2 && cs[0] == '\\' && cs[1] == '\\';
    if drive_abs || unc {
        WString::from_str(&posix_rt::pathconv::win_to_posix(p.to_string()))
    } else {
        p.to_owned()
    }
}

/// On non-Windows targets paths are already POSIX, so this is the identity.
#[cfg(not(windows))]
pub fn native_input_to_posix(p: &wstr) -> WString {
    p.to_owned()
}

pub fn perror_nix(s: &str, e: nix::errno::Errno) {
    eprintf!("%s: %s\n", s, e.desc());
}

pub fn perror_io(s: &str, e: &io::Error) {
    eprintf!("%s: %s\n", s, e);
}

/// Wide character version of getcwd().
///
/// The kernel cwd is a native Windows path (`C:\Users\per`); translate it into fish's POSIX view
/// (`/c/Users/per`) so `$PWD` and every path computation stays POSIX (Cygwin/MSYS2 model).
pub fn wgetcwd() -> WString {
    #[cfg(windows)]
    {
        // fishbowl tracks a logical cwd (posix-rt) rather than the Win32 process
        // cwd; surface it in fish's POSIX view.
        return str2wcstring(&posix_rt::pathconv::win_to_posix_str(&posix_rt::current_dir()));
    }
    #[cfg(not(windows))]
    match std::env::current_dir() {
        Ok(cwd) => osstr2wcstring(cwd),
        Err(e) => {
            flog!(error, "std::env::current_dir() failed with error:", e);
            WString::new()
        }
    }
}

/// Wide character version of readlink().
pub fn wreadlink(file_name: &wstr) -> Option<WString> {
    let _ = lwstat(file_name).ok()?;
    match fs::read_link(wcs2osstring(file_name)) {
        // The link target is a native Windows path; translate it back to fish's POSIX view.
        #[cfg(windows)]
        Ok(target) => Some(str2wcstring(posix_rt::pathconv::win_to_posix(target))),
        #[cfg(not(windows))]
        Ok(target) => Some(osstr2wcstring(target)),
        Err(e) => {
            perror_io("readlink", &e);
            None
        }
    }
}

/// `canonicalize` in fish's POSIX path domain.
///
/// fish feeds POSIX paths (`/c/Windows`) to `realpath`, but `std::fs::canonicalize` is a native
/// Windows call. Translate POSIX -> native, canonicalize, strip the `\\?\` verbatim prefix that
/// Windows returns, then translate the canonical native path back to POSIX.
#[cfg(windows)]
fn realpath_canonicalize<P: AsRef<OsStr>>(path: P) -> io::Result<std::path::PathBuf> {
    let input = path.as_ref().to_string_lossy();
    let native = posix_rt::pathconv::posix_to_win(path.as_ref());
    let canon = canonicalize(native)?;
    let mut posix = posix_rt::pathconv::win_to_posix(&canon);
    if input == "/tmp" || input.starts_with("/tmp/") {
        let tmp_posix = posix_rt::pathconv::win_to_posix(posix_rt::pathconv::tmp_dir());
        if posix == tmp_posix {
            posix = "/tmp".to_string();
        } else if let Some(rest) = posix.strip_prefix(&(tmp_posix + "/")) {
            posix = format!("/tmp/{rest}");
        }
    }
    Ok(std::path::PathBuf::from(posix))
}

/// On POSIX platforms this is just `std::fs::canonicalize`.
#[cfg(not(windows))]
fn realpath_canonicalize<P: AsRef<OsStr>>(path: P) -> io::Result<std::path::PathBuf> {
    canonicalize(path.as_ref())
}

/// Wide character realpath. The last path component does not need to be valid. If an error occurs,
/// `wrealpath()` returns `None`
pub fn wrealpath(pathname: &wstr) -> Option<WString> {
    if pathname.is_empty() {
        set_errno(Errno(0));
        return None;
    }

    // fish resolves paths in its POSIX domain (`/c/...`), but an argument may arrive in
    // native Windows form (`C:\...`, `C:/...`, or a mixed-separator path such as
    // `C:/dir\file` produced by Windows tooling). Normalize it to POSIX first so the
    // canonicalization below and every downstream caller see a consistent `/c/`-path;
    // `win_to_posix` is idempotent for already-POSIX input. Identity off-Windows.
    #[cfg(windows)]
    let pathname_owned = str2wcstring(posix_rt::pathconv::win_to_posix(wcs2osstring(pathname)));
    #[cfg(windows)]
    let pathname: &wstr = &pathname_owned;

    let mut narrow_path: Vec<u8> = wcs2zstring(pathname).into();

    // Strip trailing slashes. This is treats "/a//" as equivalent to "/a" if /a is a non-directory.
    while narrow_path.len() > 1 && narrow_path[narrow_path.len() - 1] == b'/' {
        narrow_path.pop();
    }

    // The POSIX virtual root `/` (Cygwin/MSYS2 model) is its own canonical form: `realpath /`
    // is `/`, not the system drive it backs onto. Windows canonicalization would resolve `/` and
    // any `..` that climbs to it through `C:\` and surface the system drive (`/c/`) instead of the
    // virtual root. `..` at the root never escapes it (`/..` == `/`), so collapse those segments
    // lexically here, then short-circuit a bare virtual root. Drive roots like `/c` and ordinary
    // paths keep flowing through the real canonicalization below.
    #[cfg(windows)]
    {
        while narrow_path.starts_with(b"/../") {
            narrow_path.drain(0..3);
        }
        if narrow_path == b"/.." {
            narrow_path.truncate(1);
        }
        if narrow_path == b"/" {
            return Some(WString::from_str("/"));
        }
    }

    let narrow_res = realpath_canonicalize(OsStr::from_bytes(&narrow_path));

    let real_path = if let Ok(result) = narrow_res {
        result.into_os_string().into_vec()
    } else {
        // Check if everything up to the last path component is valid.
        let pathsep_idx = narrow_path.iter().rposition(|&c| c == b'/');

        if pathsep_idx == Some(0) {
            // If the only pathsep is the first character then it's an absolute path with a
            // single path component and thus doesn't need conversion.
            narrow_path
        } else {
            // Only call realpath() on the portion up to the last component.
            let narrow_res = if let Some(pathsep_idx) = pathsep_idx {
                // Only call realpath() on the portion up to the last component.
                realpath_canonicalize(OsStr::from_bytes(&narrow_path[0..pathsep_idx]))
            } else {
                // If there is no "/", this is a file in $PWD, so give the realpath to that.
                realpath_canonicalize(".")
            };

            let Ok(narrow_result) = narrow_res else {
                set_errno(Errno(libc::ENOENT));
                return None;
            };

            let pathsep_idx = pathsep_idx.map_or(0, |idx| idx + 1);

            let mut real_path = narrow_result.into_os_string().into_vec();

            // This test is to deal with cases such as /../../x => //x.
            if real_path.len() > 1 {
                real_path.push(b'/');
            }

            real_path.extend_from_slice(&narrow_path[pathsep_idx..]);

            real_path
        }
    };

    Some(bytes2wcstring(&real_path))
}

/// Given an input path, "normalize" it:
/// 1. Collapse multiple /s into a single /, except maybe at the beginning.
/// 2. .. goes up a level.
/// 3. Remove /./ in the middle.
pub fn normalize_path(path: &wstr, allow_leading_double_slashes: bool) -> WString {
    // Count the leading slashes.
    let sep = '/';
    let mut leading_slashes: usize = 0;
    for c in path.chars() {
        if c != sep {
            break;
        }
        leading_slashes += 1;
    }

    let comps: Vec<&wstr> = path.split(sep).collect();
    let mut new_comps = Vec::new();
    for comp in comps {
        if comp.is_empty() || comp == "." {
            continue;
        } else if comp != ".." {
            new_comps.push(comp);
        } else if !new_comps.is_empty() && new_comps.last().unwrap() != ".." {
            // '..' with a real path component, drop that path component.
            new_comps.pop();
        } else if leading_slashes == 0 {
            // We underflowed the .. and are a relative (not absolute) path.
            new_comps.push(L!(".."));
        }
    }
    let mut result = join_strings(&new_comps, sep);
    // If we don't allow leading double slashes, collapse them to 1 if there are any.
    let mut numslashes = if leading_slashes > 0 { 1 } else { 0 };
    // If we do, prepend one or two leading slashes.
    // Yes, three+ slashes are collapsed to one. (!)
    if allow_leading_double_slashes && leading_slashes == 2 {
        numslashes = 2;
    }
    for _ in 0..numslashes {
        result.insert(0, sep);
    }
    // Ensure ./ normalizes to . and not empty.
    if result.is_empty() {
        result.push('.');
    }
    result
}

/// Given an input path `path` and a working directory `wd`, do a "normalizing join" in a way
/// appropriate for cd. That is, return effectively wd + path while resolving leading ../s from
/// path. The intent here is to allow 'cd' out of a directory which may no longer exist, without
/// allowing 'cd' into a directory that may not exist; see #5341.
pub fn path_normalize_for_cd(wd: &wstr, path: &wstr) -> WString {
    use std::collections::VecDeque;

    // Fast paths.
    const SEP: char = '/';
    assert!(
        wd.as_char_slice().first() == Some(&'/') && wd.as_char_slice().last() == Some(&'/'),
        "Invalid working directory, it must start and end with /"
    );
    if path.is_empty() {
        return wd.to_owned();
    } else if path.as_char_slice().first() == Some(&SEP) {
        return path.to_owned();
    } else if path.as_char_slice().first() != Some(&'.') {
        return wd.to_owned() + path;
    }

    // Split our strings by the sep.
    let mut wd_comps: VecDeque<_> = wd
        .split(SEP)
        // Remove empty segments from wd_comps, especially leading/trailing empties.
        .filter(|p| !p.is_empty())
        .collect();
    let mut path_comps = path.split(SEP).peekable();

    // Erase leading . and .. components from path_comps, popping from wd_comps as we go.
    while let Some(comp) = path_comps.peek() {
        #[allow(clippy::if_same_then_else)]
        if comp.is_empty() || comp == "." {
            path_comps.next();
        } else if comp == ".." && wd_comps.pop_back().is_some() {
            path_comps.next();
        } else {
            break;
        }
    }

    // Append un-erased elements to wd_comps and join them, prepending with a leading /
    let mut paths = wd_comps;
    paths.extend(path_comps);
    let mut result =
        WString::with_capacity(paths.iter().fold(paths.len() + 1, |sum, s| sum + s.len()));
    for p in paths.iter() {
        result.push(SEP);
        result.push_utfstr(*p);
    }
    if result.is_empty() {
        result.push(SEP);
    }
    result
}

/// Wide character version of dirname().
pub fn wdirname(mut path: &wstr) -> &wstr {
    // Do not use system-provided dirname (#7837).
    // On Mac it's not thread safe, and will error for paths exceeding PATH_MAX.
    // This follows OpenGroup dirname recipe.

    // 1: Double-slash stays.
    if path == "//" {
        return path;
    }

    // 2: All slashes => return slash.
    if !path.is_empty() && path.chars().all(|c| c == '/') {
        return L!("/");
    }

    // 3: Trim trailing slashes.
    while path.as_char_slice().last() == Some(&'/') {
        path = path.slice_to(path.char_count() - 1);
    }

    // 4: No slashes left => return period.
    let Some(last_slash) = path.chars().rposition(|c| c == '/') else {
        return L!(".");
    };

    // 5: Remove trailing non-slashes.
    path = path.slice_to(last_slash + 1);

    // 6: Skip as permitted.
    // 7: Remove trailing slashes again.
    while path.as_char_slice().last() == Some(&'/') {
        path = path.slice_to(path.char_count() - 1);
    }

    // 8: Empty => return slash.
    if path.is_empty() {
        return L!("/");
    }
    path
}

/// Wide character version of basename().
pub fn wbasename(mut path: &wstr) -> &wstr {
    // This follows OpenGroup basename recipe.
    // 1: empty => allowed to return ".". This is what system impls do.
    if path.is_empty() {
        return L!(".");
    }

    // 2: Skip as permitted.
    // 3: All slashes => return slash.
    if !path.is_empty() && path.chars().all(|c| c == '/') {
        return L!("/");
    }

    // 4: Remove trailing slashes.
    while path.as_char_slice().last() == Some(&'/') {
        path = path.slice_to(path.char_count() - 1);
    }

    // 5: Remove up to and including last slash.
    if let Some(last_slash) = path.chars().rposition(|c| c == '/') {
        path = path.slice_from(last_slash + 1);
    }
    path
}

/// Write a wide string to a file descriptor. This avoids doing any additional allocation.
/// Returns nothing when interrupted by ctrl-c or HUP.
pub fn unescape_bytes_and_write_to_fd(input: impl IntoCharIter, fd: RawFd) -> Option<usize> {
    // Accumulate data in a local buffer.
    let mut accum = [0u8; 512];
    let mut accumlen = 0;
    let accum_capacity = accum.len();

    // Helper to perform a write to 'fd', looping as necessary.
    // Return true on success, false on error.
    let mut total_written = 0;

    fn do_write(
        sigcheck: &mut SigChecker,
        fd: RawFd,
        total_written: &mut usize,
        mut buf: &[u8],
    ) -> bool {
        while !buf.is_empty() {
            let amt = match write_to_fd(buf, fd) {
                Ok(amt) => amt,
                Err(err) => {
                    // Some of our builtins emit multiple screens worth of data sent to a pager (the primary
                    // example being the `history` builtin) and receiving SIGINT should be considered normal and
                    // non-exceptional (user request to abort via Ctrl-C), meaning we shouldn't print an error.
                    //
                    // We have two options here: we can either return false without setting errored_ to
                    // true (*this* write will be silently aborted but the onus is on the caller to check
                    // the return value and skip future calls to `append()`) or we can flag the entire
                    // output stream as errored, causing us to both return false and skip any future writes.
                    // We're currently going with the latter, especially seeing as no callers currently
                    // check the result of `append()` (since it was always a void function before).
                    match err {
                        nix::errno::Errno::EINTR => {
                            if !sigcheck.check() {
                                continue;
                            }
                        }
                        nix::errno::Errno::EPIPE => (),
                        _ => perror("write"),
                    }
                    return false;
                }
            };
            *total_written += amt;
            assert!(amt <= buf.len(), "Wrote more than requested");
            buf = &buf[amt..];
        }
        true
    }

    // Helper to flush the accumulation buffer.
    let flush_accum = |sigcheck: &mut SigChecker,
                       total_written: &mut usize,
                       accum: &[u8],
                       accumlen: &mut usize| {
        if !do_write(sigcheck, fd, total_written, &accum[..*accumlen]) {
            return false;
        }
        *accumlen = 0;
        true
    };

    let mut sigcheck = SigChecker::new_sighupintterm();
    let mut success = str2bytes_callback(input, |buff: &[u8]| {
        if buff.len() + accumlen > accum_capacity {
            // We have to flush.
            if !flush_accum(&mut sigcheck, &mut total_written, &accum, &mut accumlen) {
                return false;
            }
        }
        if buff.len() + accumlen <= accum_capacity {
            // Accumulate more.
            accum[accumlen..(accumlen + buff.len())].copy_from_slice(buff);
            accumlen += buff.len();
            true
        } else {
            // Too much data to even fit, just write it immediately.
            do_write(&mut sigcheck, fd, &mut total_written, buff)
        }
    });
    // Flush any remaining.
    if success {
        success = flush_accum(&mut sigcheck, &mut total_written, &accum, &mut accumlen);
    }
    if success { Some(total_written) } else { None }
}

const PUA1_START: char = '\u{E000}';
const PUA1_END: char = '\u{F900}';
// const PUA2_START: char = '\u{F0000}';
// const PUA2_END: char = '\u{FFFFE}';
// const PUA3_START: char = '\u{100000}';
// const PUA3_END: char = '\u{10FFFE}';

/// Return one if the code point is in a Unicode private use area.
pub(crate) fn fish_is_pua(c: char) -> bool {
    PUA1_START <= c && c < PUA1_END
}

/// We need this because there are too many implementations that don't return the proper answer for
/// some code points. See issue #3050.
pub fn fish_iswalnum(c: char) -> bool {
    !fish_reserved_codepoint(c) && !fish_is_pua(c) && c.is_alphanumeric()
}

/// Given that `cursor` is a pointer into `base`, return the offset in characters.
/// This emulates C pointer arithmetic:
///    `wstr_offset_in(cursor, base)` is equivalent to C++ `cursor - base`.
pub fn wstr_offset_in(cursor: &wstr, base: &wstr) -> usize {
    let cursor = cursor.as_slice();
    let base = base.as_slice();
    // cursor may be a zero-length slice at the end of base,
    // which base.as_ptr_range().contains(cursor.as_ptr()) will reject.
    let base_range = base.as_ptr_range();
    let curs_range = cursor.as_ptr_range();
    assert!(
        base_range.start <= curs_range.start && curs_range.end <= base_range.end,
        "cursor should be a subslice of base"
    );
    let offset = unsafe { cursor.as_ptr().offset_from(base.as_ptr()) };
    assert!(offset >= 0, "offset should be non-negative");
    offset as usize
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_path, unescape_bytes_and_write_to_fd, wbasename, wdirname, wstr_offset_in,
    };
    use crate::{prelude::*, tests::prelude::*};
    use fish_widestring::bytes2wcstring;
    use rand::RngExt as _;
    use std::{
        fs::OpenOptions,
        io::{Read as _, Seek as _},
        os::{fd::AsRawFd as _, unix::fs::OpenOptionsExt as _},
    };

    mod test_path_normalize_for_cd {
        use super::super::path_normalize_for_cd;
        use fish_widestring::L;

        #[test]
        fn relative_path() {
            let wd = L!("/home/user/");
            let path = L!("projects");
            assert_eq!(
                path_normalize_for_cd(wd, path),
                L!("/home/user/projects"),
                "Normalized path for ({wd}, {path})"
            );
        }

        #[test]
        fn absolute_path() {
            let wd = L!("/home/user/");
            let path = L!("/etc");
            assert_eq!(
                path_normalize_for_cd(wd, path),
                L!("/etc"),
                "Normalized path for ({wd}, {path})"
            );
        }

        #[test]
        fn parent_directory() {
            let wd = L!("/home/user/projects/");
            let path = L!("../docs");
            assert_eq!(
                path_normalize_for_cd(wd, path),
                L!("/home/user/docs"),
                "Normalized path for ({wd}, {path})"
            );
        }

        #[test]
        fn current_directory() {
            let wd = L!("/home/user/");
            let path = L!("./");
            assert_eq!(
                path_normalize_for_cd(wd, path),
                L!("/home/user"),
                "Normalized path for ({wd}, {path})"
            );
        }

        #[test]
        fn nested_parent_directory() {
            let wd = L!("/home/user/projects/");
            let path = L!("../../");
            assert_eq!(
                path_normalize_for_cd(wd, path),
                L!("/home"),
                "Normalized path for ({wd}, {path})"
            );
        }

        #[test]
        fn complex_path() {
            let wd = L!("/home/user/projects/");
            let path = L!("./../other/projects/./.././../docs");
            assert_eq!(
                path_normalize_for_cd(wd, path),
                L!("/home/user/other/projects/./.././../docs"),
                "Normalized path for ({wd}, {path})"
            );
        }

        #[test]
        fn root_directory() {
            let wd = L!("/");
            let path = L!("..");
            assert_eq!(
                path_normalize_for_cd(wd, path),
                L!("/.."),
                "Normalized path for ({wd}, {path})"
            );
        }

        #[test]
        fn up_to_root_directory() {
            let wd = L!("/foo/");
            let path = L!("..");
            assert_eq!(
                path_normalize_for_cd(wd, path),
                L!("/"),
                "Normalized path for ({wd}, {path})"
            );
        }

        #[test]
        fn empty_path() {
            let wd = L!("/home/user/");
            let path = L!("");
            assert_eq!(
                path_normalize_for_cd(wd, path),
                L!("/home/user/"),
                "Normalized path for ({wd}, {path})"
            );
        }

        #[test]
        fn trailing_slash() {
            let wd = L!("/home/user/projects/");
            let path = L!("docs/");
            assert_eq!(
                path_normalize_for_cd(wd, path),
                L!("/home/user/projects/docs/"),
                "Normalized path for ({wd}, {path})"
            );
        }
    }

    #[test]
    fn test_normalize_path() {
        fn norm_path(path: &wstr) -> WString {
            normalize_path(path, true)
        }
        assert_eq!(norm_path(L!("")), ".");
        assert_eq!(norm_path(L!("..")), "..");
        assert_eq!(norm_path(L!("./")), ".");
        assert_eq!(norm_path(L!("./.")), ".");
        assert_eq!(norm_path(L!("/")), "/");
        assert_eq!(norm_path(L!("//")), "//");
        assert_eq!(norm_path(L!("///")), "/");
        assert_eq!(norm_path(L!("////")), "/");
        assert_eq!(norm_path(L!("/.///")), "/");
        assert_eq!(norm_path(L!(".//")), ".");
        assert_eq!(norm_path(L!("/.//../")), "/");
        assert_eq!(norm_path(L!("////abc")), "/abc");
        assert_eq!(norm_path(L!("/abc")), "/abc");
        assert_eq!(norm_path(L!("/abc/")), "/abc");
        assert_eq!(norm_path(L!("/abc/..def/")), "/abc/..def");
        assert_eq!(norm_path(L!("//abc/../def/")), "//def");
        assert_eq!(norm_path(L!("abc/../abc/../abc/../abc")), "abc");
        assert_eq!(norm_path(L!("../../")), "../..");
        assert_eq!(norm_path(L!("foo/./bar")), "foo/bar");
        assert_eq!(norm_path(L!("foo/../")), ".");
        assert_eq!(norm_path(L!("foo/../foo")), "foo");
        assert_eq!(norm_path(L!("foo/../foo/")), "foo");
        assert_eq!(norm_path(L!("foo/././bar/.././baz")), "foo/baz");
    }

    #[test]
    fn test_wdirname_wbasename() {
        // path, dir, base
        struct Test(&'static wstr, &'static wstr, &'static wstr);
        let testcases: &[Test] = &[
            Test(L!(""), L!("."), L!(".")),
            Test(L!("foo//"), L!("."), L!("foo")),
            Test(L!("foo//////"), L!("."), L!("foo")),
            Test(L!("/////foo"), L!("/"), L!("foo")),
            Test(L!("//foo/////bar"), L!("//foo"), L!("bar")),
            Test(L!("foo/////bar"), L!("foo"), L!("bar")),
            // Examples given in XPG4.2.
            Test(L!("/usr/lib"), L!("/usr"), L!("lib")),
            Test(L!("usr"), L!("."), L!("usr")),
            Test(L!("/"), L!("/"), L!("/")),
            Test(L!("."), L!("."), L!(".")),
            Test(L!(".."), L!("."), L!("..")),
        ];

        for tc in testcases {
            let Test(path, tc_dir, tc_base) = *tc;
            let dir = wdirname(path);
            assert_eq!(
                dir, tc_dir,
                "\npath: {:?}, dir: {:?}, tc.dir: {:?}",
                path, dir, tc_dir
            );

            let base = wbasename(path);
            assert_eq!(
                base, tc_base,
                "\npath: {:?}, base: {:?}, tc.base: {:?}",
                path, base, tc_base
            );
        }

        // Ensure strings which greatly exceed PATH_MAX still work (#7837).
        const PATH_MAX: usize = libc::PATH_MAX as usize;
        let mut longpath = WString::new();
        longpath.reserve(PATH_MAX * 2 + 10);
        while longpath.char_count() <= PATH_MAX * 2 {
            longpath.push_str("/overlong");
        }
        let last_slash = longpath.chars().rposition(|c| c == '/').unwrap();
        let longpath_dir = &longpath[..last_slash];
        assert_eq!(wdirname(&longpath), longpath_dir);
        assert_eq!(wbasename(&longpath), L!("overlong"));
    }

    #[test]
    #[serial]
    fn test_wwrite_to_fd() {
        test_init();
        let temp_file = fish_tempfile::new_file().unwrap();
        let mut rng = rand::rng();
        let sizes = [1, 2, 3, 5, 13, 23, 64, 128, 255, 4096, 4096 * 2];
        for &size in &sizes {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o666)
                .open(temp_file.path())
                .unwrap();
            let mut input = Vec::new();
            for _i in 0..size {
                input.push(rng.random());
            }

            let amt =
                unescape_bytes_and_write_to_fd(&bytes2wcstring(&input), file.as_raw_fd()).unwrap();
            assert_eq!(amt, input.len());

            file.seek(std::io::SeekFrom::Start(0)).unwrap();

            let mut contents = vec![];
            file.read_to_end(&mut contents).unwrap();
            assert_eq!(&contents, &input);
        }
    }

    #[test]
    fn test_wstr_offset_in() {
        use fish_widestring::L;
        let base = L!("hello world");
        assert_eq!(wstr_offset_in(&base[6..], base), 6);
        assert_eq!(wstr_offset_in(&base[0..], base), 0);
        assert_eq!(wstr_offset_in(&base[6..], &base[6..]), 0);
        assert_eq!(wstr_offset_in(&base[base.len()..], base), base.len());
    }
}
