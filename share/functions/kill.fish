# localization: tier1
function kill --description 'Send a signal to a process'
    # Expand %n job specifiers to process ids, then hand off to the `kill` builtin which
    # delivers the signal natively (no external kill(1), which cannot address native
    # Windows process ids).
    set -l args (__fish_expand_pid_args $argv)
    and builtin kill $args
end
