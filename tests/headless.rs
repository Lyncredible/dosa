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

#[test]
fn runs_batch_files_from_directory_and_zip_and_waits_for_guests() {
    let dir = Fixture::new();
    let games = dir.0.join("games");
    fs::create_dir_all(games.join("BIN")).unwrap();
    let extra = dir.0.join("extra");
    fs::create_dir(&extra).unwrap();
    fs::write(extra.join("MARK.TXT"), b"second drive\r\n").unwrap();

    let start = b"@echo off\r\n\
        rem An inert 50% comment can contain > and &.\r\n\
        echo startup %0 %1\r\n\
        cd bin\r\n\
        status\r\n\
        if not errorlevel 7 goto bad\r\n\
        if errorlevel 8 goto bad\r\n\
        call child \"two words\"\r\n\
        if not errorlevel 9 goto bad\r\n\
        if errorlevel 10 goto bad\r\n\
        echo returned %ERRORLEVEL%\r\n\
        call :helper done\r\n\
        if not errorlevel 10 goto bad\r\n\
        D:\r\n\
        type mark.txt\r\n\
        C:\r\n\
        cd\r\n\
        echo escaped 100%%\r\n\
        exit /b 17\r\n\
        :bad\r\n\
        exit /b 99\r\n\
        :HELPER\r\n\
        echo label %*\r\n\
        exit /b 10\r\n\x1a\xffgarbage";
    let child = b"echo child %1\r\nexit /b 9\r\n";
    let mut program = vec![
        0xba, 0x0c, 0x01, 0xb4, 9, 0xcd, 0x21, 0xb8, 7, 0x4c, 0xcd, 0x21,
    ];
    program.extend_from_slice(b"child program\r\n$");
    let files: [(&str, &[u8]); 5] = [
        ("START.BAT", start),
        ("BIN/CHILD.BAT", child),
        ("BIN/STATUS.COM", &program),
        ("BIN/STATUS.EXE", b"should not run"),
        ("BIN/STATUS.BAT", b"should not run"),
    ];
    let zip = dir.0.join("scripts.zip");
    let mut archive = zip::ZipWriter::new(fs::File::create(&zip).unwrap());
    for (name, contents) in files {
        fs::write(games.join(name), contents).unwrap();
        archive
            .start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        archive.write_all(contents).unwrap();
    }
    archive.finish().unwrap();

    for (source, invocation) in [(&games, "start"), (&zip, "start.bat")] {
        let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .arg("--mount")
            .arg(format!("C={}", source.display()))
            .arg("--mount")
            .arg(format!("D={}", extra.display()))
            .args(["--headless", "--exec", &format!("{invocation} arg")])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(17), "{output:?}");
        assert_eq!(
            output.stdout,
            format!(
                "startup {invocation} arg\r\nchild program\r\nchild \"two words\"\r\nreturned 9\r\nlabel done\r\nsecond drive\r\nC:\\bin\r\nescaped 100%\r\n"
            )
            .as_bytes()
        );
    }
}

#[test]
fn batch_chaining_replaces_the_current_frame() {
    let dir = Fixture::new();
    fs::write(
        dir.0.join("FIRST.BAT"),
        "@echo off\nsecond.bat\nexit /b 99\n",
    )
    .unwrap();
    fs::write(dir.0.join("SECOND.BAT"), "echo replacement\nexit /b 23\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .arg("--mount")
        .arg(format!("C={}", dir.0.display()))
        .args(["--headless", "--exec", "first"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(23), "{output:?}");
    assert_eq!(output.stdout, b"replacement\r\n");
}

#[test]
fn batch_errors_keep_line_context_and_bound_calls_and_loops() {
    let dir = Fixture::new();
    fs::write(dir.0.join("CRASH.COM"), [0x0f, 0x0b]).unwrap();
    for (source, exec, expected) in [
        (
            "@echo off\n:again\ngoto again\n",
            "case",
            "batch line limit",
        ),
        ("@echo off\ncall case\n", "case", "batch CALL depth limit"),
        (
            "@echo off\necho hello | second\n",
            "case",
            "pipes, redirection",
        ),
        ("@echo off\ncrash\n", "case", "instruction"),
    ] {
        fs::write(dir.0.join("CASE.BAT"), source).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .arg("--mount")
            .arg(format!("C={}", dir.0.display()))
            .args(["--headless", "--exec", exec])
            .output()
            .unwrap();
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(
            error.contains("C:\\case.BAT:") && error.contains(expected),
            "{error}"
        );
    }

    fs::write(
        dir.0.join("CASE.BAT"),
        format!("@echo off\necho {}\n", "%*".repeat(300)),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .arg("--mount")
        .arg(format!("C={}", dir.0.display()))
        .args(["--headless", "--exec", &format!("case {}", "x".repeat(50))])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("case.BAT:2: batch command exceeds"));
}

#[test]
fn batch_files_cannot_mount_a_host_path_even_from_a_nested_call() {
    let dir = Fixture::new();
    let guest = dir.0.join("guest");
    fs::create_dir(&guest).unwrap();
    fs::write(dir.0.join("OUTSIDE.TXT"), "outside marker").unwrap();
    fs::write(guest.join("START.BAT"), "@echo off\ncall child\n").unwrap();
    fs::write(
        guest.join("CHILD.BAT"),
        format!(
            "if errorlevel 0 mount D \"{}\"\ntype D:\\OUTSIDE.TXT\n",
            dir.0.display()
        ),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .arg("--mount")
        .arg(format!("C={}", guest.display()))
        .args(["--headless", "--exec", "start"])
        .output()
        .unwrap();
    let error = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        error.contains("child.BAT:1:") && error.contains("interactive shell or --mount"),
        "{error}"
    );
    assert!(output.stdout.is_empty());
}
