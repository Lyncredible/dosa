use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use dosa_dos::RunState;
use dosa_mount::Mount;

use dosa::display::Desktop;
use dosa::shell::{Command, Shell};

const USAGE: &str = "\
Usage: dosa [--mount DRIVE=PATH]... [--exec COMMAND] [--headless]

Mount a host file, directory, or ZIP on a DOS drive.
Use --exec to enter a DOS shell command on startup.
Use --headless with --exec to run without opening a window.";

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
            Some("--exec") => {
                let value = args.next().ok_or("--exec needs a DOS command")?;
                if command.is_some() {
                    return Err("--exec can only be supplied once".into());
                }
                command = Some(value.into_string().map_err(|_| "--exec needs UTF-8 text")?);
            }
            Some("--headless") => headless = true,
            _ => return Err(format!("unknown option {:?}", arg).into()),
        }
    }

    let mut shell = Shell::new();
    for (drive, path) in mounts {
        shell.mount(
            drive,
            Mount::open(&path)
                .map_err(|error| format!("cannot mount {} on {drive}: {error}", path.display()))?,
        )?;
    }
    if headless {
        return run_headless(shell, &command.ok_or("--headless requires --exec")?);
    }
    dosa_desktop::run(Desktop::new(shell, command.as_deref()))?;
    Ok(0)
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
