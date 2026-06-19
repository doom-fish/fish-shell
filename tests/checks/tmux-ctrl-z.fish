#RUN: %fish %s
#REQUIRES: command -v tmux

# Interactive job control: pressing Ctrl-Z while a foreground external command
# runs must suspend it (delivering SIGTSTP to the job group), print the "has
# stopped" notification, and return fish to the prompt; `jobs` then lists it as
# stopped. On Windows there is no kernel tty to turn Ctrl-Z into SIGTSTP, so the
# fishbowl runtime watches the terminal input for Ctrl-Z while a foreground job
# owns the terminal.
#
# `command sleep` forces the external `sleep` binary (fish ships a `sleep`
# builtin, which would run in-process with no separate job to suspend). Ctrl-Z is
# re-sent until the job actually stops so the test is deterministic regardless of
# how long the job takes to start under load (an early Ctrl-Z, before the job
# owns the terminal, is simply a no-op at the prompt).

isolated-tmux-start

isolated-tmux send-keys "command sleep 10" Enter
tmux-sleep
set -l tries 0
while [ $tries -lt 50 ] && not isolated-tmux capture-pane -p | string match -q "*has stopped*"
    isolated-tmux send-keys C-z
    tmux-sleep
    set tries (math $tries + 1)
end

isolated-tmux send-keys "jobs" Enter
sleep-until "isolated-tmux capture-pane -p | string match -q '*stopped command sleep 10*'"
isolated-tmux capture-pane -p
# CHECK: prompt 0> command sleep 10
# CHECK: fish: Job 1, 'command sleep 10' has stopped
# CHECK: prompt 0> jobs
# CHECK: Job{{.*}}Command
# CHECK: {{.*}}stopped command sleep 10
# CHECK: prompt {{\d+}}>
