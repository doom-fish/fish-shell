# RUN: %fish %s
# REQUIRES: command -v sphinx-build
# REQUIRES: %fish -c '__fish_data_with_directory man/man1 ".*" ls' | grep -q .

# Override the test-override again.
status get-file functions/__fish_print_help.fish | source

set -lx MANWIDTH 80
# fish renders its own man pages natively (the __fish_render_man builtin, via
# crates/manfmt) with no man/groff/nroff — and so no MSYS — process. The output
# is already plain text, so no `col`/deroff pass is needed.
man abbr | head -n4
# CHECK: ABBR(1) {{ *}} fish-shell {{ *}} ABBR(1)
# CHECK: NAME
# CHECK:        abbr - manage fish abbreviations
man : | head -n4
# CHECK: TRUE(1) {{ *}} fish-shell {{ *}} TRUE(1)
# CHECK: NAME
# CHECK:        true - return a successful result
