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
# builtin, which would run in-process with no separate job to suspend).

isolated-tmux-start

isolated-tmux send-keys "command sleep 10" Enter
tmux-sleep
isolated-tmux send-keys C-z
tmux-sleep
isolated-tmux send-keys "jobs" Enter
tmux-sleep
isolated-tmux capture-pane -p
# CHECK: prompt 0> command sleep 10
# CHECK: fish: Job 1, 'command sleep 10' has stopped
# CHECK: prompt 0> jobs
# CHECK: Job{{.*}}Command
# CHECK: {{.*}}stopped command sleep 10
# CHECK: prompt {{\d+}}>
