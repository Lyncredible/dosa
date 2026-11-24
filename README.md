# dosa

dosa is a Rust DOS emulator with a GPU-rendered desktop window and a built-in DOS shell. It runs supported real-mode COM and MZ programs with text, CGA mode 06h, VGA mode 13h, or planar VGA (Mode X) graphics. The same emulator core drives the desktop and the headless command line.

## Build and run

Install the stable Rust toolchain and build with Cargo.

~~~console
cargo build --release --locked
cargo run --release -- --mount C=path/to/games.zip
~~~

The first mounted drive is selected on startup. Type `DIR` to list its contents and enter a program name to start it. When you omit the extension, the shell checks `.COM`, `.EXE`, and `.BAT` in that order. Type `HELP` for more shell commands.

Mount a directory, a single file, or a ZIP. Quote host paths containing spaces.

Use `--mount-fat A=floppy.img` to read a FAT volume image as a DOS drive. A profile mount can set `fat_image = true`. FAT images are read-only.

Use `--mount-iso D=disc.iso`, `iso_image = true` in a profile, **Drives → Mount ISO image**, or `IMGMOUNT D "path to disc.iso"` at the Dosa prompt for a read-only ISO 9660 data drive. Dosa reads the primary volume with 2,048-byte blocks and single-extent files. DOS programs can use MSCDEX `INT 2Fh` to detect and enumerate optical drives and read data sectors or volume descriptors. Device-driver requests, CD audio, and disc swapping remain planned.

~~~console
cargo run --release -- --mount "C=path to a directory" --mount D=tools.zip --exec "C:\APP.EXE"
cargo run --release -- --mount C=DEMO.COM --headless --exec DEMO
~~~

You can also mount a source after startup from inside the shell.

~~~text
MOUNT D "path to a directory"
D:
DIR
~~~

Mounts are read-only by default. To let DOS programs save files, explicitly mount a host directory with `--mount-rw`.

~~~console
dosa --mount C=programs.zip --mount-rw D=dos-saves
~~~

Create the host directory before mounting it. The guest can create, write, truncate, rename, and delete files there, and can create and remove empty directories. New names must use DOS 8.3 syntax.

Use `--mount-session C=programs.zip` if a program needs to open ZIP files for writing. Dosa mounts the archive's DOS 8.3 names in a temporary writable directory. Changes last until the mount is replaced or Dosa exits, and the source ZIP stays unchanged. The Drives menu offers **Writable ZIP copy** for the same mode.

Headless execution returns the program's DOS exit code. It reports an error if the guest needs keyboard input or reaches the instruction limit.

### Batch files

Run a batch file by name at the DOS prompt or pass its name and arguments with `--exec`. A batch waits for each DOS program to exit before running the next line. Drive and directory changes carry over to later commands and to the prompt.

Batch files support `ECHO`, `@ECHO OFF`, `REM`, labels, `GOTO`, `CALL file.BAT`, `CALL :label`, `EXIT /B [status]`, and `IF [NOT] ERRORLEVEL number command`. Substitutions include `%0` through `%9`, `%*`, `%ERRORLEVEL%`, and `%%` for a literal percent. Use `CALL` to return to a parent script. Invoking a batch file directly replaces the current script.

Scripts accept ASCII or UTF-8 text. Dosa reports unsupported syntax, including pipes, redirection, and environment-variable expansion, with the batch path and line number. Mount host sources with `--mount` or at the interactive prompt before you start a script. Each run allows up to 16 call frames and 10,000 executed lines.

## Launch profiles

Save the settings for a program in a TOML file and load it with `--profile`.
All fields are optional. Mounts are listed in startup order.

~~~toml
exec = "C:\\DEMO.COM"
cpu = 386
speed = "normal"
text_rows = 25
text_columns = 80

[[mounts]]
drive = "C"
path = "games.zip"

[[mounts]]
drive = "D"
path = "tools"
~~~

~~~console
dosa --profile profiles/demo.toml
dosa --profile profiles/demo.toml --headless
dosa --profile profiles/demo.toml --cpu 286 --speed fast --text-rows 43
~~~

Relative TOML mount paths resolve from the profile's directory. Relative CLI mount paths resolve from the working directory. Set `writable = true` for a writable host directory or `session_zip = true` for a writable ZIP copy. A CLI mount flag replaces a profile mount on the same drive, including its access mode. Explicit `--exec`, `--cpu`, `--speed`, `--text-rows`, and `--text-columns` override the corresponding saved values.

CPU profiles are `8086`, `286`, and `386`. Text modes have 80 or 132 columns and 25, 43, or 50 rows. Desktop execution speeds are `slow`, `normal`, `fast`, and `maximum` (`max` also works). Headless mode uses the same mounts, CPU, text rows, and command with its fixed instruction limit. Its command can come from `--exec` or the profile's `exec` field.

## Desktop controls

Use Drives to select a drive letter, see its source type and access mode, and mount or unmount it while the shell is idle. Removing the current drive switches the prompt to the first remaining drive. CPU selects the 8086, 286, or 386 instruction profile while the shell is idle. Speed adjusts the guest instruction budget during execution.

**Drives → Quick launch** opens a COM, EXE, or BAT and temporarily mounts its folder read-only on an unused DOS drive so it can use sibling files. Dosa restores the previous drive and directories when the program or batch finishes. Use a writable mount for programs that need to save files in their own folder.

View offers pause/resume, fullscreen, scaling, and 80- or 132-column text presets with 25, 43, or 50 rows. Fit to window corrects the guest aspect ratio. Integer scaling uses whole physical pixels for each source pixel and falls back to fit when the display area is too small. DOS mouse support maps the host pointer into the guest viewport and releases held buttons when leaving the viewport or opening a menu.

The desktop plays PC speaker sound, AdLib OPL2 music, and 8-bit mono Sound Blaster 2.0 PCM. For Sound Blaster setup, use port `220h`, IRQ `7`, and DMA channel `1`. DOS programs also receive these settings in `BLASTER`.

Audio offers master volume and mute while a guest runs. Unmuting restores the selected level. Keyboard menu access uses Alt+D, Alt+C, Alt+S, Alt+V, and Alt+A, followed by arrow keys and Enter.

**Edit** (`Alt+E`) copies the visible DOS text screen or pastes up to 4,096 characters. At the shell prompt, pasted line breaks become spaces and Enter submits the edited command. In a running program, paste feeds the DOS BIOS keyboard queue one character at a time, including Enter for line breaks. Choose **Stop pasting** to discard the remaining paste.

The desktop speed presets allow 10,000, 50,000, 100,000, or 250,000 guest instructions per display tick. Full native picker interaction and macOS desktop checks remain open.

## Compatibility and verification

Windows is the first desktop target and has been tested with Vulkan and Direct3D 12. The Linux workspace suite and WSLg Vulkan window smoke passed on Ubuntu 24.04. The workspace type-checks for Apple Silicon macOS. Additional desktop coverage is tracked in the [task list](TASKS.md).

The built-in DOS services provide a subset of DOS and BIOS behavior. Compatibility depends on each program's CPU, video, file system, sound, and peripheral requirements.

## Development

~~~console
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked
~~~

See [TASKS.md](TASKS.md) for the architecture and work in progress.

Set `DOSA_SMOKE_GENERATED=1` and run `cargo test --test desktop_smoke --locked` to exercise a live window. The test builds a temporary ZIP with a DOS program that updates VGA graphics after Enter. To use your own ZIP, set `DOSA_SMOKE_ARCHIVE`, `DOSA_SMOKE_PROGRAM`, and optionally `DOSA_SMOKE_ENTER_COUNT`. Set `DOSA_SMOKE_SESSION=1` to exercise the writable ZIP copy. The test mounts the ZIP through the application menu action and checks graphics updates in the GPU event loop.

For an animated final prompt, use `DOSA_SMOKE_FINAL_ENTER_DELAY_MS` to schedule the final Enter relative to the previous one. Use `DOSA_SMOKE_FINAL_MIN_COLORS` to require a minimum number of distinct colors in the resulting scene.
