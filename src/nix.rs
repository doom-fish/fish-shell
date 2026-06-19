//! Safe wrappers around various libc functions that we might want to reuse across modules.

pub fn isatty(fd: i32) -> bool {
    // This returns false if the fd is valid but not a tty, or is invalid.
    // No place we currently call it really cares about the difference.
    //
    // POSIX only guarantees isatty() returns a nonzero value (not specifically 1)
    // for a terminal. glibc happens to return 1, but the mingw CRT's isatty()
    // returns the FDEV flag bit (0x40 == 64) for a console. Comparing against a
    // specific value would therefore wrongly report every console as a non-tty on
    // Windows, forcing fish into non-interactive mode. Treat any nonzero as a tty.
    (unsafe { libc::isatty(fd) }) != 0
}
