mod profile;

use std::env;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use dosa::display::Desktop;
use dosa::shell::{Command, PAUSE_MESSAGE, Shell};
use dosa_dos::RunState;
use dosa_mount::Mount;

use profile::{
    LaunchProfile, MountAccess, SettingsOverrides, StartupMount, parse_cpu, parse_speed,
    parse_text_columns, parse_text_rows,
};

const USAGE: &str = "\
Usage: dosa [OPTIONS]

  --profile PATH       Load a TOML launch profile
  --mount DRIVE=PATH   Mount a host file, directory, or ZIP (repeatable)
  --mount-rw DRIVE=DIR Mount a writable host directory (repeatable)
  --mount-session DRIVE=ZIP Mount a writable ZIP copy until exit (repeatable)
  --mount-fat DRIVE[:PARTITION]=IMG Mount a read-only FAT volume or MBR partition
  --mount-iso DRIVE=ISO Mount a read-only ISO 9660 data image (repeatable)
  --exec COMMAND       Run a DOS shell command on startup
  --cpu 8086|286|386   Select the guest CPU profile
  --speed SPEED        Select slow, normal, fast, or maximum desktop pacing
  --text-rows ROWS     Select 25, 43, or 50 text rows
  --text-columns COLS  Select 80 or 132 text columns
  --headless           Run without a window (requires a startup command)
  -h, --help           Show usage
  --version            Show the version

Explicit options override values from the launch profile.";
const HEADLESS_IRQ_WAIT: Duration = Duration::from_secs(2);
const HALT_TIMEOUT: &str = "program did not resume from HLT within the headless wait limit";

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("dosa: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<u8, Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let mut mounts = Vec::new();
    let mut command = None;
    let mut profile_path = None;
    let mut overrides = SettingsOverrides::default();
    let mut headless = false;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return Ok(0);
            }
            Some("--version") => {
                println!("dosa {}", env!("CARGO_PKG_VERSION"));
                return Ok(0);
            }
            Some(
                option @ ("--mount" | "--mount-rw" | "--mount-fat" | "--mount-iso"
                | "--mount-session"),
            ) => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("{option} needs DRIVE=PATH"))?;
                let (drive, path, partition) = parse_mount(&value, option == "--mount-fat")?;
                mounts.push(StartupMount {
                    drive,
                    path,
                    access: match option {
                        "--mount-rw" => MountAccess::WritableDirectory,
                        "--mount-fat" => MountAccess::FatImage(partition),
                        "--mount-iso" => MountAccess::IsoImage,
                        "--mount-session" => MountAccess::SessionZip,
                        _ => MountAccess::ReadOnly,
                    },
                });
            }
            Some("--profile") => {
                let path = args.next().ok_or("--profile needs a TOML path")?;
                if profile_path.is_some() {
                    return Err("--profile can only be supplied once".into());
                }
                profile_path = Some(PathBuf::from(path));
            }
            Some("--exec") => {
                let value = args.next().ok_or("--exec needs a DOS command")?;
                if command.is_some() {
                    return Err("--exec can only be supplied once".into());
                }
                command = Some(value.into_string().map_err(|_| "--exec needs UTF-8 text")?);
            }
            Some("--cpu") => {
                overrides.cpu = Some(parse_cpu(&option_value(args.next(), "--cpu")?)?);
            }
            Some("--speed") => {
                overrides.speed = Some(parse_speed(&option_value(args.next(), "--speed")?)?);
            }
            Some("--text-rows") => {
                overrides.text_mode =
                    Some(parse_text_rows(&option_value(args.next(), "--text-rows")?)?);
            }
            Some("--text-columns") => {
                overrides.text_columns = Some(parse_text_columns(&option_value(
                    args.next(),
                    "--text-columns",
                )?)?);
            }
            Some("--headless") => headless = true,
            _ => return Err(format!("unknown option {:?}", arg).into()),
        }
    }

    let mut launch = profile_path
        .as_deref()
        .map(LaunchProfile::load)
        .transpose()?
        .unwrap_or_default();
    launch.apply_mount_overrides(mounts);
    let settings = overrides.apply(launch.settings);
    let command = command.or(launch.command);
    let mut shell = Shell::new();
    for StartupMount {
        drive,
        path,
        access,
    } in launch.mounts
    {
        let mount = match access {
            MountAccess::FatImage(None) => Mount::open_fat_image(&path),
            MountAccess::FatImage(Some(number)) => Mount::open_fat_partition(&path, number),
            MountAccess::IsoImage => Mount::open_iso_image(&path),
            MountAccess::WritableDirectory => Mount::open_writable_directory(&path),
            MountAccess::SessionZip => Mount::open_session_zip(&path),
            MountAccess::ReadOnly => Mount::open(&path),
        };
        shell.mount(
            drive,
            mount
                .map_err(|error| format!("cannot mount {} on {drive}: {error}", path.display()))?,
        )?;
    }
    if headless {
        settings.apply(&mut shell)?;
        return run_headless(
            shell,
            &command
                .ok_or("--headless requires a startup command from --exec or the launch profile")?,
        );
    }
    dosa_desktop::run(Desktop::with_settings(shell, command.as_deref(), settings)?)?;
    Ok(0)
}

fn option_value(value: Option<OsString>, option: &str) -> Result<String, String> {
    value
        .ok_or_else(|| format!("{option} needs a value"))?
        .into_string()
        .map_err(|_| format!("{option} needs UTF-8 text"))
}

fn parse_mount(
    value: &std::ffi::OsStr,
    allow_partition: bool,
) -> Result<(char, PathBuf, Option<u16>), &'static str> {
    // The drive prefix is ASCII. Preserve the native encoding of the host path.
    let bytes = value.as_encoded_bytes();
    let invalid = "--mount needs a letter and a host path, for example C=games";
    let separator = bytes.iter().position(|&byte| byte == b'=').ok_or(invalid)?;
    let (drive, partition) = match &bytes[..separator] {
        [drive] if drive.is_ascii_alphabetic() => (*drive, None),
        [drive, b':', digits @ ..] if allow_partition && drive.is_ascii_alphabetic() => {
            let number = std::str::from_utf8(digits)
                .ok()
                .filter(|value| value.bytes().all(|digit| digit.is_ascii_digit()))
                .and_then(|value| value.parse::<u16>().ok())
                .filter(|&number| number != 0)
                .ok_or("FAT partition number must be a positive integer")?;
            (*drive, Some(number))
        }
        _ => return Err(invalid),
    };
    // SAFETY: the split immediately follows the ASCII '=' in this OsStr's own encoding.
    let path = unsafe { std::ffi::OsStr::from_encoded_bytes_unchecked(&bytes[separator + 1..]) };
    if path.is_empty() {
        return Err(invalid);
    }
    Ok((
        char::from(drive.to_ascii_uppercase()),
        PathBuf::from(path),
        partition,
    ))
}

fn run_headless(mut shell: Shell, command: &str) -> Result<u8, Box<dyn std::error::Error>> {
    let mut output = io::stdout().lock();
    let mut remaining_slices = 1000;
    let mut command = shell.execute(command)?;
    loop {
        match command {
            Command::Print(text) => output.write_all(&text)?,
            Command::Pause => {
                output.write_all(PAUSE_MESSAGE)?;
                let mut input = io::stdin().lock();
                while shell.batch_waiting() {
                    output.flush()?;
                    let mut key = [0];
                    input.read_exact(&mut key).map_err(|error| {
                        shell.guest_failed(format!("PAUSE needs a key from stdin: {error}").into())
                    })?;
                    if let Some(response) = shell.resume_batch(key[0]) {
                        output.write_all(response)?;
                    }
                }
            }
            Command::Launched => {
                let mut halted_since = None;
                loop {
                    if remaining_slices == 0 {
                        let message = if halted_since.is_some() {
                            HALT_TIMEOUT
                        } else {
                            "program exceeded the headless instruction budget"
                        };
                        return Err(shell.guest_failed(message.into()));
                    }
                    remaining_slices -= 1;
                    let state = shell.machine.run(20_000);
                    output.write_all(&shell.machine.take_output())?;
                    match state.map_err(|error| shell.guest_failed(error.into()))? {
                        RunState::Exited(code) => {
                            shell.guest_finished(code);
                            break;
                        }
                        RunState::WaitingForKey => {
                            return Err(
                                shell.guest_failed("program is waiting for keyboard input".into())
                            );
                        }
                        RunState::WaitingForInterrupt => {
                            let since = halted_since.get_or_insert_with(Instant::now);
                            if since.elapsed() >= HEADLESS_IRQ_WAIT {
                                return Err(shell.guest_failed(HALT_TIMEOUT.into()));
                            }
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        RunState::Idle => break,
                        RunState::Running => halted_since = None,
                    }
                }
            }
            Command::Clear | Command::Prompt | Command::Exit => {}
        }
        let Some(step) = shell.step_batch()? else {
            return Ok(shell.errorlevel());
        };
        if let Some(echo) = step.echo {
            output.write_all(echo.as_bytes())?;
        }
        command = step.command;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_native_mounts() {
        assert_eq!(
            parse_mount(std::ffi::OsStr::new("d=some path/games.zip"), false),
            Ok(('D', PathBuf::from("some path/games.zip"), None))
        );
        assert!(parse_mount(std::ffi::OsStr::new("D="), false).is_err());
    }
}
