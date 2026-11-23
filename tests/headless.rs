use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture =
            Self(std::env::temp_dir().join(format!("dosa-{}-{suffix}", std::process::id())));
        fs::create_dir(&fixture.0).unwrap();
        fixture
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn runs_a_dos_program_from_file_directory_and_zip_mounts() {
    let dir = Fixture::new();
    let file = dir.0.join("DEMO.COM");
    // Print a DOS $-terminated string, then exit with status 7.
    let mut program = vec![
        0xba, 0x0c, 0x01, 0xb4, 9, 0xcd, 0x21, 0xb8, 7, 0x4c, 0xcd, 0x21,
    ];
    program.extend_from_slice(b"Hello from DOS!\r\n$");
    fs::write(&file, &program).unwrap();

    let zip = dir.0.join("games.zip");
    let mut archive = zip::ZipWriter::new(fs::File::create(&zip).unwrap());
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    archive.start_file("DEMO.COM", options).unwrap();
    archive.write_all(&program).unwrap();
    archive.finish().unwrap();

    for source in [&file, &dir.0, &zip] {
        let mut mount = std::ffi::OsString::from("C=");
        mount.push(source);
        let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .arg("--mount")
            .arg(mount)
            .args(["--headless", "--exec", "demo"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(7), "{:?}", output);
        assert_eq!(output.stdout, b"Hello from DOS!\r\n");
    }
}

#[test]
fn launch_profile_applies_guest_settings_and_explicit_cli_overrides() {
    let dir = Fixture::new();
    let profiles = dir.0.join("profiles");
    let work = dir.0.join("work");
    fs::create_dir_all(profiles.join("games")).unwrap();
    fs::create_dir_all(work.join("override")).unwrap();

    let profile = profiles.join("launch.toml");
    fs::write(
        &profile,
        r#"
exec = "rows"
cpu = 286
speed = "slow"
text_rows = 43

[[mounts]]
drive = "c"
path = "games"

[[mounts]]
drive = "D"
path = "extra.com"
"#,
    )
    .unwrap();

    // Exit with the configured text rows (BIOS Data Area 40:84 stores rows - 1).
    let check_rows = [
        0xb8, 0x40, 0, 0x8e, 0xd8, 0xa0, 0x84, 0, 0xfe, 0xc0, 0xb4, 0x4c, 0xcd, 0x21,
    ];
    let mut program = vec![0x60, 0x61]; // PUSHA / POPA, available on the 286.
    program.extend(check_rows);
    fs::write(profiles.join("games/ROWS.COM"), program).unwrap();
    let mut program = vec![0x66, 0x90]; // A redundant operand-size prefix requires a 386.
    program.extend(check_rows);
    fs::write(work.join("override/OTHER.COM"), program).unwrap();
    fs::write(profiles.join("extra.com"), [0xb8, 7, 0x4c, 0xcd, 0x21]).unwrap();

    let run = |options: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_dosa"))
            .current_dir(&work)
            .args(options)
            .arg("--profile")
            .arg(&profile)
            .arg("--headless")
            .output()
            .unwrap()
    };
    let output = run(&[]);
    assert_eq!(output.status.code(), Some(43), "{output:?}");

    let output = run(&["--cpu", "8086"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unavailable on the 8086 CPU profile"),
        "{output:?}"
    );
    let output = run(&[
        "--mount",
        "c=override",
        "--exec",
        "other",
        "--cpu",
        "386",
        "--text-rows",
        "50",
        "--speed",
        "maximum",
    ]);
    assert_eq!(output.status.code(), Some(50), "{output:?}");
    let output = run(&["--mount", "C=override", "--exec", r"D:\extra"]);
    assert_eq!(output.status.code(), Some(7), "{output:?}");
}

#[test]
fn invalid_profiles_and_cli_settings_report_what_to_fix() {
    let dir = Fixture::new();
    let profile = dir.0.join("bad.toml");
    for (source, message) in [
        ("text_rows = 60", "text rows must be 25, 43, or 50"),
        (
            "speed = 'turbo'",
            "speed must be slow, normal, fast, or maximum",
        ),
        ("text_row = 25", "unknown field"),
        (
            "[[mounts]]\ndrive = 'c'\npath = 'one'\n[[mounts]]\ndrive = 'C'\npath = 'two'",
            "mount drive C appears more than once",
        ),
    ] {
        fs::write(&profile, source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .arg("--profile")
            .arg(&profile)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("bad.toml") && stderr.contains(message),
            "{stderr}"
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args(["--cpu", "486"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cpu must be 8086, 286, or 386"));
}
