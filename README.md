# dosa

dosa is a Rust DOS emulator with a GPU-rendered desktop window and a built-in DOS shell. It runs supported real-mode COM and MZ programs with text, CGA mode 06h, VGA mode 13h, or planar VGA (Mode X) graphics. The same emulator core drives the desktop and the headless command line.

## Build and run

Install the stable Rust toolchain and build with Cargo.

~~~console
cargo build --release --locked
cargo run --release -- --mount C=path/to/games.zip
~~~

The first mounted drive is selected on startup. Type `DIR` to list its contents and enter an executable name to start it. The shell looks for `.COM` and `.EXE` when you omit the extension. Type `HELP` for more shell commands.

Mount a directory, a single file, or a ZIP. Quote host paths containing spaces.

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

Mounts are read-only. Headless execution returns the program's DOS exit code. It reports an error if the guest needs keyboard input or reaches the instruction limit.

## Launch profiles

Save the settings for a program in a TOML file and load it with `--profile`.
All fields are optional. Mounts are listed in startup order.

~~~toml
exec = "C:\\DEMO.COM"
cpu = 386
speed = "normal"
text_rows = 25

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

Relative TOML mount paths resolve from the profile's directory. Relative `--mount` paths resolve from the working directory. `--mount` replaces a profile mount on the same drive and adds mounts on new drives. Explicit `--exec`, `--cpu`, `--speed`, and `--text-rows` override the corresponding saved values.

CPU profiles are `8086`, `286`, and `386`. Text modes have 25, 43, or 50 rows. Desktop execution speeds are `slow`, `normal`, `fast`, and `maximum` (`max` also works). Headless mode uses the same mounts, CPU, text rows, and command with its fixed instruction limit. Its command can come from `--exec` or the profile's `exec` field.

## Compatibility

Windows is the first desktop target and has been tested with Vulkan and Direct3D 12. The workspace type-checks for x86-64 Linux and Apple Silicon macOS. Native desktop testing on those platforms is tracked in the [task list](TASKS.md).

The built-in DOS services provide a subset of DOS and BIOS behavior. Compatibility depends on each program's CPU, video, file system, sound, and peripheral requirements.

## Development

~~~console
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked
~~~

See [TASKS.md](TASKS.md) for the architecture and work in progress.
