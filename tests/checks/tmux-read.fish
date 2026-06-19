#RUN: fish=%fish %fish %s
#REQUIRES: command -v tmux
#REQUIRES: test -z "$CI"

printf >?$__fish_config_dir/functions/fish_greeting.fish %s \
'function fish_greeting
    set -l name (read)
    echo hello $name
end
'

isolated-tmux-start

isolated-tmux send-keys name Enter 'echo foo' Enter
sleep-until "isolated-tmux capture-pane -p | string match -q '*prompt 1>*'"
isolated-tmux capture-pane -p
# CHECK: read> name
# CHECK: hello name
# CHECK: prompt 0> echo foo
# CHECK: foo
# CHECK: prompt 1>

isolated-tmux send-keys C-l 'SHELL_WELCOME=hello $fish -ic "read --prompt-str=R"' Enter
# Wait for the nested fish's read prompt (a line starting with "R") before
# interrupting, so Ctrl-C reaches the read rather than racing its startup.
sleep-until "isolated-tmux capture-pane -p | string match -q 'R*'"
isolated-tmux send-keys C-c
sleep-until "isolated-tmux capture-pane -p | string match -q '*prompt 2>*'"
isolated-tmux capture-pane -p
# CHECK: prompt 1> SHELL_WELCOME=hello $fish -ic "read --prompt-str=R"
# CHECK: R
# CHECK: prompt 2>
