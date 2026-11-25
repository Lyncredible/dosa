# 17 · DOS batch files

Add batch execution through the shell and startup command path.

- Resolve explicit .BAT paths and include BAT after COM and EXE when looking up commands.
- Execute lines sequentially, waiting for guest programs before continuing. Preserve drive and directory changes between commands.
- Start with ECHO, REM, labels, GOTO, CALL, EXIT /B, argument expansion, and ERRORLEVEL checks. Specify unsupported command syntax with actionable errors.
- Bound recursive CALL and command execution, and retain useful file and line context on failures.
- PAUSE waits for a keyboard event in the desktop or a byte from stdin in headless mode. The desktop Ctrl+C shortcut or byte 03h in headless input asks whether to terminate the batch.
- Verify generated batch fixtures from directory and ZIP mounts, including nested calls, arguments, failing programs, and return to the interactive prompt.

This is planned compatibility work following the desktop controls milestone.
