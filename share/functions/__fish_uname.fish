# localization: skip(private)
function __fish_uname
    if not set -q __fish_uname
        # fishbowl is a native Windows port that emulates the Cygwin/MSYS
        # environment fish runs in on Windows: `/c/...` paths, suffix/shebang
        # executability, and `noacl` permissions (see __fish_cygwin_noacl). `uname`
        # itself is not a native Windows command — it only resolves when a Unix
        # toolset (e.g. git-for-Windows' MSYS bin) happens to be on PATH, so calling
        # it per prompt is fragile (a missing `uname` aborts the whole prompt via
        # the fish_git_prompt -> __fish_uname chain). Report the MSYS identity
        # directly: it keeps fish's Cygwin/MSYS code paths (__fish_is_cygwin, the
        # cygwin completion tests) active — which is what this environment is —
        # without spawning anything, matching the __fish_cygwin_noacl patch that
        # likewise avoids per-query `uname`/`mount`.
        set -g __fish_uname MSYS_NT
    end
    echo -- $__fish_uname
end
