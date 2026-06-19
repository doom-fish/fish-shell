# localization: skip(private)
function __fish_cygwin_noacl
    # fishbowl is a native Windows port: it models Cygwin/MSYS-style noacl
    # permissions for Windows files (executability comes from suffix/shebang
    # rather than POSIX mode bits). Asking MSYS `mount` + `stat` (or even cached
    # `uname`) for every query is both slow and can leak tool diagnostics into
    # callers. Treat every path as noacl in this native-Windows environment.
    return 0
end
