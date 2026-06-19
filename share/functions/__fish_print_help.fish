# localization: skip(private)
function __fish_print_help --description "Print help for the specified fish function or builtin"
    set -l item (__fish_canonicalize_builtin $argv[1])

    function __fish_print_help_man -V item -a man1
        if not path is $man1
            # Trigger the "documentation not be installed" message. Currently
            # only when called from core.
            return 2
        end
        set -l page (path filter -- $man1/$item.1)[1]
        or return 2
        # Render natively (no man/groff/nroff, no MSYS) — see the
        # __fish_render_man builtin (crates/manfmt).
        __fish_render_man $page
    end
    __fish_data_with_directory man/man1 \
        "$(string escape --style=regex $item.1)(?:\.gz)?" \
        __fish_print_help_man
    __fish_with_status functions --erase __fish_print_help_man
end
