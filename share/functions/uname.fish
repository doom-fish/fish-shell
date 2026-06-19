# localization: skip(private)
function uname --description "Native-Windows uname shim (fishbowl)"
    # `uname` is not a native Windows command: it only resolves when a Unix
    # toolset (e.g. git-for-Windows' MSYS bin) happens to be on PATH. fish's own
    # bundled completions and functions call `uname`/`uname -s` at load time
    # (ls, cp, mv, rm, date, ping, ... ~25 files), so a missing `uname` spams an
    # "Unknown command" error and aborts whichever file is being sourced. Provide
    # a built-in shim: defer to a real `uname` when one is installed (full
    # fidelity), otherwise synthesise this environment's MSYS identity so those
    # callers keep working. Matches __fish_uname / __fish_cygwin_noacl, which
    # likewise treat fishbowl as the Cygwin/MSYS environment fish runs in here.
    if command -q uname
        command uname $argv
        return
    end

    # No external uname: synthesise. Defaults mirror git-for-Windows' MSYS uname.
    set -l kernel_name MSYS_NT-10.0
    set -l nodename $hostname
    set -l release 3.5.7-fishbowl
    set -l kver "#1 SMP"
    set -l machine x86_64
    set -l os Msys

    if not set -q argv[1]
        echo -- $kernel_name
        return 0
    end

    # Collect requested fields (flags may be bundled, e.g. `uname -sm`).
    set -l want
    for arg in $argv
        switch $arg
            case -a --all
                set want kernel nodename release version machine os
            case --kernel-name
                set -a want kernel
            case --nodename
                set -a want nodename
            case --kernel-release
                set -a want release
            case --kernel-version
                set -a want version
            case --machine
                set -a want machine
            case --processor --hardware-platform
                set -a want machine
            case --operating-system
                set -a want os
            case '--*'
                # Unknown long option: ignore (keep the shim forgiving).
            case '-*'
                for c in (string split '' -- (string sub -s 2 -- $arg))
                    switch $c
                        case s
                            set -a want kernel
                        case n
                            set -a want nodename
                        case r
                            set -a want release
                        case v
                            set -a want version
                        case m p i
                            set -a want machine
                        case o
                            set -a want os
                    end
                end
            case '*'
                # Non-option argument: ignore.
        end
    end

    test -z "$want[1]"; and set want kernel

    set -l out
    for field in $want
        switch $field
            case kernel
                set -a out $kernel_name
            case nodename
                set -a out $nodename
            case release
                set -a out $release
            case version
                set -a out $kver
            case machine
                set -a out $machine
            case os
                set -a out $os
        end
    end
    echo -- (string join ' ' -- $out)
end
