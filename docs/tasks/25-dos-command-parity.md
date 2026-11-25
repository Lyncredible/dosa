# 25 · MS-DOS 6.22 built-in command parity

Use the Microsoft MS-DOS 6.22 User’s Guide and the Microsoft Help command reference as the compatibility baseline. Track commands implemented by COMMAND.COM separately from the utilities and drivers shipped on its disks. [Microsoft MS-DOS 6.22 User’s Guide](https://www.bitsavers.org/pdf/microsoft/msdos_6.22/DOS_6.22_Users_Manual_1994.pdf) [MS-DOS 6.22 Help command reference](https://dos-help.soulsphere.org/help.hlp/index.html)

| Area | Current behavior and remaining work |
| --- | --- |
| Directory listings | `DIR /W`, `/B`, `/L`, `/?`, and filename patterns work. Default listings show names and sizes. Add pagination, recursion, ordering, attributes, volume and space summaries, timestamps, and the `DIRCMD` environment. [DIR reference](https://dos-help.soulsphere.org/help.hlp/Dir.html) |
| Files and directories | `CD`/`CHDIR`, `TYPE`, `MD`/`MKDIR`, and `RD`/`RMDIR` work. Directory mutations use writable mounts and preserve the current directory and non-empty directories. Add `COPY`, `DEL`/`ERASE`, and `REN`/`RENAME` with wildcard and prompt behavior. |
| Environment and prompt | Add `SET`, `PATH`, `PROMPT`, and executable lookup through PATH. Share the shell environment with new guests and batch expansion. |
| Batch control | `CALL`, `ECHO`, `GOTO`, `REM`, and part of `IF` work. Add the remaining `IF` forms, `FOR`, `PAUSE`, `SHIFT`, and documented batch interactions. See [task 17](17-batch-files.md). |
| System and session | `CLS`, `EXIT`, and drive changes work. Add `BREAK`, `CHCP`, `CTTY`, `DATE`, `LH`/`LOADHIGH`, `TIME`, `VER`, `VERIFY`, and `VOL`. Track any dependencies on emulated devices, code pages, and memory managers. |
| Command I/O | Implement and verify DOS pipes, input/output redirection, and device names with commands and batch files. |

Dosa documents `MOUNT` for files, directories, ZIPs, and disk images. `HELP MOUNT` lists the shell options. The legacy `IMGMOUNT` invocation still opens ISO images, and startup flags are documented in the README and command-line help.

Exercise the shell prompt, startup command, and batch paths with generated fixtures. Validate behavior on read-only and writable mounts before marking a command complete.
