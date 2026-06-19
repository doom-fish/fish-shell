//! A universal-variable change notifier for native Windows.
//!
//! Linux uses inotify, the BSDs use kqueue and macOS uses notifyd. Windows has
//! none of these, so this backend watches the directory that holds fish's
//! `fish_variables` file with [`ReadDirectoryChangesW`] and translates a real
//! change of that file into readiness on a descriptor the `select`/`pselect`
//! loop already knows how to wait on.
//!
//! `ReadDirectoryChangesW` cannot itself be plugged into fish's descriptor-based
//! event loop, so we bridge it with a self-pipe: a dedicated watcher thread
//! blocks in `ReadDirectoryChangesW`, and whenever the watched file is created,
//! renamed-into-place (uvars are swapped in atomically), or written, it writes a
//! byte to the pipe. The read end of that pipe is a real virtual descriptor
//! (an anonymous pipe in the fishbowl runtime fd table, which `posix-select`
//! probes with `PeekNamedPipe`), so `notification_fd()` exposes it exactly like
//! the inotify backend exposes its inotify fd.

use crate::env_universal_common::default_vars_path;
use crate::prelude::*;
use crate::universal_notifier::UniversalNotifier;
use crate::wutil::{wbasename, wdirname};
use fish_widestring::wcs2osstring;
use osfd_win::{AsFd as _, AsRawFd as _, OwnedFd, RawFd};
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::AsRawHandle as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::CancelSynchronousIo;

/// A notifier based on `ReadDirectoryChangesW`.
pub struct WindowsNotifier {
    /// Read end of the self-pipe; becomes readable when the watched file changes.
    read_fd: OwnedFd,
    /// The watcher thread blocking in `ReadDirectoryChangesW`.
    watcher: Option<JoinHandle<()>>,
    /// Set on drop to tell the watcher thread to stop.
    stop: Arc<AtomicBool>,
    /// The watched directory's Win32 handle, kept open for the thread's lifetime
    /// and closed once the thread has joined.
    dir_handle: isize,
}

impl WindowsNotifier {
    /// Create a notifier at the default fish_variables path.
    pub fn new() -> Option<Self> {
        Self::new_at(&default_vars_path())
    }

    /// Create a notifier at a given path.
    /// The path should be the full path to the fish_variables file.
    /// `WindowsNotifier` watches the parent directory for changes to that file.
    /// It watches the directory rather than the file itself because uvars are
    /// atomically swapped into place (the inode/file is replaced, not modified).
    pub fn new_at(path: &wstr) -> Option<Self> {
        let dirname = wdirname(path);
        let basename = wcs2osstring(wbasename(path))
            .to_string_lossy()
            .into_owned();

        // Open the directory for change notifications. FILE_FLAG_BACKUP_SEMANTICS
        // is required to obtain a handle to a directory, and we omit
        // FILE_FLAG_OVERLAPPED so ReadDirectoryChangesW runs synchronously.
        let mut wide: Vec<u16> = wcs2osstring(dirname).encode_wide().collect();
        wide.push(0);
        let dir_handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if dir_handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let dir_handle = dir_handle as isize;

        // Self-pipe: the watcher thread writes to `write_fd`, fish's event loop
        // watches `read_fd`.
        let (read_fd, write_fd) = match nix::unistd::pipe() {
            Ok(pipes) => pipes,
            Err(_) => {
                unsafe { CloseHandle(dir_handle as HANDLE) };
                return None;
            }
        };

        let stop = Arc::new(AtomicBool::new(false));
        let watcher = {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                watch_loop(dir_handle, write_fd, basename, &stop);
            })
        };

        Some(WindowsNotifier {
            read_fd,
            watcher: Some(watcher),
            stop,
            dir_handle,
        })
    }
}

/// The watcher thread body. Blocks in `ReadDirectoryChangesW` until the watched
/// directory changes, and writes a byte to `write_fd` whenever the file we care
/// about is affected, until asked to stop.
fn watch_loop(dir_handle: isize, write_fd: OwnedFd, basename: String, stop: &AtomicBool) {
    // 4-byte aligned buffer: FILE_NOTIFY_INFORMATION requires DWORD alignment, so
    // back the buffer with `u32`s (2048 * 4 = 8 KiB).
    let mut buffer = vec![0u32; 2048];
    let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE
        | FILE_NOTIFY_CHANGE_SIZE;

    while !stop.load(Ordering::SeqCst) {
        let mut bytes_returned: u32 = 0;
        let ok = unsafe {
            ReadDirectoryChangesW(
                dir_handle as HANDLE,
                buffer.as_mut_ptr() as *mut std::ffi::c_void,
                (buffer.len() * std::mem::size_of::<u32>()) as u32,
                0, // bWatchSubtree = FALSE
                filter,
                &mut bytes_returned,
                std::ptr::null_mut(),
                None,
            )
        };

        if stop.load(Ordering::SeqCst) {
            break;
        }
        if ok == 0 {
            // The call failed, most likely because it was cancelled or the
            // handle was closed during teardown. Stop watching.
            break;
        }

        // A zero byte count means the change buffer overflowed: too many changes
        // to enumerate. Conservatively treat that as a change to our file.
        let changed = if bytes_returned == 0 {
            true
        } else {
            file_changed(&buffer, &basename)
        };

        if changed {
            // A single byte is enough to make the read end readable; the value is
            // irrelevant. Ignore errors (e.g. the read end already closed).
            let _ = nix::unistd::write(write_fd.as_fd(), &[1u8]);
        }
    }
}

/// Scan a `ReadDirectoryChangesW` result buffer for an event naming `basename`.
fn file_changed(buffer: &[u32], basename: &str) -> bool {
    let base = buffer.as_ptr() as *const u8;
    let mut offset = 0usize;
    loop {
        // SAFETY: `offset` always points at the start of a FILE_NOTIFY_INFORMATION
        // record produced by the kernel within our buffer.
        let info = unsafe { &*(base.add(offset) as *const FILE_NOTIFY_INFORMATION) };
        let name_len = info.FileNameLength as usize / std::mem::size_of::<u16>();
        let name_slice = unsafe { std::slice::from_raw_parts(info.FileName.as_ptr(), name_len) };
        let name = String::from_utf16_lossy(name_slice);
        if name.eq_ignore_ascii_case(basename) {
            return true;
        }

        let next = info.NextEntryOffset as usize;
        if next == 0 {
            return false;
        }
        offset += next;
    }
}

impl Drop for WindowsNotifier {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(watcher) = self.watcher.take() {
            // Unblock the watcher: cancel any in-flight synchronous
            // ReadDirectoryChangesW. There is a small window where the thread has
            // observed `!stop` but has not yet entered the blocking call, so retry
            // until it has actually exited.
            let thread_handle = watcher.as_raw_handle() as HANDLE;
            while !watcher.is_finished() {
                unsafe { CancelSynchronousIo(thread_handle) };
                std::thread::sleep(Duration::from_millis(5));
            }
            let _ = watcher.join();
        }
        // The watcher thread has exited, so the directory handle is no longer in
        // use and is safe to close.
        unsafe { CloseHandle(self.dir_handle as HANDLE) };
    }
}

impl UniversalNotifier for WindowsNotifier {
    // Do nothing to trigger a notification.
    // The notifications are generated from changes to the file itself.
    fn post_notification(&self) {}

    // Returns the fd from which to watch for events.
    fn notification_fd(&self) -> Option<RawFd> {
        Some(self.read_fd.as_fd().as_raw_fd())
    }

    // The notification_fd is readable; drain it. Returns true if a notification is
    // considered to have been posted.
    fn notification_fd_became_readable(&self, fd: RawFd) -> bool {
        assert_eq!(fd, self.read_fd.as_fd().as_raw_fd(), "unexpected fd");
        // The fd is readable, so a single read drains all currently-available
        // bytes (a pipe read returns immediately with whatever is buffered) and
        // never blocks here.
        let mut buf = [0u8; 64];
        matches!(nix::unistd::read(self.read_fd.as_fd(), &mut buf), Ok(n) if n > 0)
    }
}
