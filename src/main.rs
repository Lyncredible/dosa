mod profile;

use std::env;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use dosa::display::Desktop;
use dosa::shell::{Command, Shell};
use dosa_dos::RunState;
use dosa_mount::Mount;

use profile::{LaunchProfile, SettingsOverrides, parse_cpu, parse_speed, parse_text_rows};

const USAGE: &str = "\
Usage: dosa [OPTIONS]

  --profile PATH       Load a TOML launch profile
  --mount DRIVE=PATH   Mount a host file, directory, or ZIP (repeatable)
  --exec COMMAND       Run a DOS shell command on startup
  --cpu 8086|286|386   Select the guest CPU profile
  --speed SPEED        Select slow, normal, fast, or maximum desktop pacing
  --text-rows ROWS     Select 25, 43, or 50 text rows
  --headless           Run without a window (requires a startup command)
  -h, --help           Show usage
  --version            Show the version

Explicit options override values from the launch profile.";

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
            Some("--mount") => {
                let value = args.next().ok_or("--mount needs DRIVE=PATH")?;
                mounts.push(parse_mount(&value)?);
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
    for (drive, path) in launch.mounts {
        shell.mount(
            drive,
            Mount::open(&path)
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

fn parse_mount(value: &std::ffi::OsStr) -> Result<(char, PathBuf), &'static str> {
    // The drive prefix is ASCII. Preserve the native encoding of the host path.
    let bytes = value.as_encoded_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b'=' {
        return Err("--mount needs a letter and a host path, for example C=games");
    }
    // SAFETY: the split immediately follows the ASCII '=' in this OsStr's own encoding.
    let path = unsafe { std::ffi::OsStr::from_encoded_bytes_unchecked(&bytes[2..]) };
    Ok((
        char::from(bytes[0].to_ascii_uppercase()),
        PathBuf::from(path),
    ))
}

fn run_headless(mut shell: Shell, command: &str) -> Result<u8, Box<dyn std::error::Error>> {
    let mut output = io::stdout().lock();
    match shell.execute(command)? {
        Command::Print(text) => output.write_all(&text)?,
        Command::Launched => {
            for _ in 0..1000 {
                let state = shell.machine.run(20_000);
                output.write_all(&shell.machine.take_output())?;
                match state? {
                    RunState::Exited(code) => return Ok(code),
                    RunState::WaitingForKey => {
                        return Err("program is waiting for keyboard input".into());
                    }
                    RunState::Idle => return Ok(0),
                    RunState::Running => {}
                }
            }
            return Err("program exceeded the headless instruction budget".into());
        }
        Command::Clear | Command::Prompt | Command::Exit => {}
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_native_mounts() {
        assert_eq!(
            parse_mount(std::ffi::OsStr::new("d=some path/games.zip")),
            Ok(('D', PathBuf::from("some path/games.zip")))
        );
        assert!(parse_mount(std::ffi::OsStr::new("D=")).is_err());
    }
}
