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

#[test]
fn headless_hlt_can_wait_for_default_timer_without_spinning() {
    let dir = Fixture::new();
    let program = dir.0.join("WAIT.COM");
    for (opcode, status) in [(0xfb, 7), (0xfa, 1)] {
        // STI or CLI, followed by HLT and a normal DOS exit.
        fs::write(&program, [opcode, 0xf4, 0xb8, 7, 0x4c, 0xcd, 0x21]).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .arg("--mount")
            .arg(format!("C={}", dir.0.display()))
            .args(["--headless", "--exec", "wait"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(status), "{output:?}");
        if status == 1 {
            assert!(String::from_utf8_lossy(&output.stderr).contains("did not resume from HLT"));
        }
    }
}

#[test]
fn guest_writable_mount_persists_file_and_directory_operations() {
    const SAVE_DIR: u16 = 0x300;
    const FIRST: u16 = 0x310;
    const FINAL: u16 = 0x330;
    const DATA: u16 = 0x350;
    const EMPTY: u16 = 0x360;
    const AUXILIARY: u16 = 0x370;
    const BUFFER: u16 = 0x390;

    fn mov(program: &mut Vec<u8>, opcode: u8, value: u16) {
        program.push(opcode);
        program.extend(value.to_le_bytes());
    }

    fn dos(program: &mut Vec<u8>, ax: u16) {
        mov(program, 0xb8, ax);
        // On carry, exit immediately with AL set to the DOS error code.
        program.extend([0xcd, 0x21, 0x73, 4, 0xb4, 0x4c, 0xcd, 0x21]);
    }

    fn place(program: &mut Vec<u8>, address: u16, bytes: &[u8]) {
        assert!(program.len() <= usize::from(address - 0x100));
        program.resize(usize::from(address - 0x100), 0);
        program.extend(bytes);
    }

    let dir = Fixture::new();
    let profile = dir.0.join("saved.toml");
    let cli = dir.0.join("cli");
    let from_profile = dir.0.join("profile");
    fs::create_dir(&cli).unwrap();
    fs::create_dir(&from_profile).unwrap();
    fs::write(
        &profile,
        "exec = 'writer'\n[[mounts]]\ndrive = 'C'\npath = 'profile'\nwritable = true\n",
    )
    .unwrap();

    let mut writer = Vec::new();
    mov(&mut writer, 0xba, SAVE_DIR);
    dos(&mut writer, 0x3900);
    mov(&mut writer, 0xba, FIRST);
    mov(&mut writer, 0xb9, 0);
    dos(&mut writer, 0x3c00);
    writer.push(0x93); // XCHG AX,BX, retaining the handle.
    mov(&mut writer, 0xba, DATA);
    mov(&mut writer, 0xb9, 5);
    dos(&mut writer, 0x4000);
    mov(&mut writer, 0xba, 2);
    mov(&mut writer, 0xb9, 0);
    dos(&mut writer, 0x4200);
    dos(&mut writer, 0x4000); // Zero-length write truncates to the offset.
    mov(&mut writer, 0xba, 4);
    dos(&mut writer, 0x4200);
    mov(&mut writer, 0xba, DATA + 5);
    mov(&mut writer, 0xb9, 1);
    dos(&mut writer, 0x4000); // A hole is filled with zeros.
    dos(&mut writer, 0x6800);
    dos(&mut writer, 0x3e00);

    mov(&mut writer, 0xba, FIRST);
    mov(&mut writer, 0xbf, FINAL); // ES:DI is the rename destination.
    dos(&mut writer, 0x5600);
    mov(&mut writer, 0xba, FINAL);
    dos(&mut writer, 0x3d02);
    writer.push(0x93);
    mov(&mut writer, 0xba, BUFFER);
    mov(&mut writer, 0xb9, 5);
    dos(&mut writer, 0x3f00);
    dos(&mut writer, 0x3e00);
    mov(&mut writer, 0xbb, 1);
    dos(&mut writer, 0x4000); // Report the reopened contents on stdout.

    mov(&mut writer, 0xba, AUXILIARY);
    mov(&mut writer, 0xb9, 0);
    dos(&mut writer, 0x3c00);
    writer.push(0x93);
    dos(&mut writer, 0x3e00);
    dos(&mut writer, 0x4100);
    mov(&mut writer, 0xba, EMPTY);
    dos(&mut writer, 0x3900);
    dos(&mut writer, 0x3a00);
    dos(&mut writer, 0x4c00);

    place(&mut writer, SAVE_DIR, b"SAVE\0");
    place(&mut writer, FIRST, b"SAVE\\RUN.DAT\0");
    place(&mut writer, FINAL, b"SAVE\\FINAL.DAT\0");
    place(&mut writer, DATA, b"abcdez");
    place(&mut writer, EMPTY, b"EMPTY\0");
    place(&mut writer, AUXILIARY, b"SAVE\\TMP.DAT\0");
    for mount in [&cli, &from_profile] {
        fs::write(mount.join("WRITER.COM"), &writer).unwrap();
    }

    let binary = env!("CARGO_BIN_EXE_dosa");
    let profile_refused = Command::new(binary)
        .arg("--profile")
        .arg(&profile)
        .arg("--mount")
        .arg(format!("C={}", from_profile.display()))
        .arg("--headless")
        .output()
        .unwrap();
    assert_eq!(
        profile_refused.status.code(),
        Some(5),
        "{profile_refused:?}"
    );
    assert!(!from_profile.join("SAVE").exists());

    let cli_output = Command::new(binary)
        .arg("--mount-rw")
        .arg(format!("C={}", cli.display()))
        .args(["--headless", "--exec", "writer"])
        .output()
        .unwrap();
    let profile_output = Command::new(binary)
        .arg("--profile")
        .arg(&profile)
        .arg("--headless")
        .output()
        .unwrap();

    for (mount, output) in [(&cli, cli_output), (&from_profile, profile_output)] {
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"ab\0\0z");
        assert_eq!(fs::read(mount.join("SAVE/FINAL.DAT")).unwrap(), b"ab\0\0z");
        assert!(!mount.join("SAVE/RUN.DAT").exists());
        assert!(!mount.join("SAVE/TMP.DAT").exists());
        assert!(!mount.join("EMPTY").exists());

        let reopened = Command::new(binary)
            .arg("--mount")
            .arg(format!("C={}", mount.display()))
            .args(["--headless", "--exec", r"type SAVE\FINAL.DAT"])
            .output()
            .unwrap();
        assert!(reopened.status.success(), "{reopened:?}");
        assert_eq!(reopened.stdout, b"ab\0\0z");
    }

    let rejected_file = Command::new(binary)
        .arg("--mount-rw")
        .arg(format!("C={}", cli.join("WRITER.COM").display()))
        .args(["--headless", "--exec", "writer"])
        .output()
        .unwrap();
    assert_eq!(rejected_file.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&rejected_file.stderr).contains("cannot mount"));
}

#[test]
fn guest_truncate_and_write_do_not_change_an_outside_hard_link() {
    let dir = Fixture::new();
    let mounted = dir.0.join("mounted");
    let outside = dir.0.join("outside.dat");
    fs::create_dir(&mounted).unwrap();
    fs::write(&outside, b"remain outside").unwrap();
    fs::hard_link(&outside, mounted.join("LINK.DAT")).unwrap();

    let mut writer = vec![
        0xba, 0x30, 0x01, 0xb9, 0, 0, 0xb8, 0, 0x3c, 0xcd, 0x21, // Create/truncate.
        0x73, 4, 0xb4, 0x4c, 0xcd, 0x21, // Exit with DOS error in AL on carry.
        0x93, 0xba, 0x40, 0x01, 0xb9, 6, 0, 0xb8, 0, 0x40, 0xcd, 0x21, // Write data.
        0x73, 4, 0xb4, 0x4c, 0xcd, 0x21, // Exit on carry.
        0xb8, 0, 0x4c, 0xcd, 0x21,
    ];
    writer.resize(0x30, 0);
    writer.extend(b"LINK.DAT\0");
    writer.resize(0x40, 0);
    writer.extend(b"inside");
    fs::write(mounted.join("LINKTEST.COM"), writer).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .arg("--mount-rw")
        .arg(format!("C={}", mounted.display()))
        .args(["--headless", "--exec", "linktest"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read(mounted.join("LINK.DAT")).unwrap(), b"inside");
    assert_eq!(fs::read(outside).unwrap(), b"remain outside");
}

#[test]
fn simultaneous_read_and_write_handles_see_current_contents() {
    let dir = Fixture::new();
    fs::write(dir.0.join("SHARED.DAT"), b"old").unwrap();
    let mut program = Vec::new();
    fn dos_call(program: &mut Vec<u8>, setup: &[u8]) {
        program.extend_from_slice(setup);
        program.extend_from_slice(&[
            0xcd, 0x21, 0x73, 5, // Continue on success.
            0xb8, 1, 0x4c, 0xcd, 0x21,
        ]);
    }
    dos_call(&mut program, &[0xba, 0, 3, 0xb8, 0, 0x3d]); // Read-only open.
    program.extend_from_slice(&[0x89, 0xc6]); // Preserve handle A in SI.
    dos_call(&mut program, &[0xba, 0, 3, 0xb8, 2, 0x3d]); // Read/write open.
    program.extend_from_slice(&[0x89, 0xc3]); // Handle B in BX.
    dos_call(&mut program, &[0xba, 0x10, 3, 0xb9, 3, 0, 0xb4, 0x40]);
    program.extend_from_slice(&[0x89, 0xf3]); // Read the update through handle A.
    dos_call(&mut program, &[0xba, 0x20, 3, 0xb9, 3, 0, 0xb4, 0x3f]);
    dos_call(
        &mut program,
        &[0xbb, 1, 0, 0xba, 0x20, 3, 0xb9, 3, 0, 0xb4, 0x40],
    );
    program.extend_from_slice(&[
        0x89, 0xf3, 0xba, 0x10, 3, 0xb9, 1, 0, 0xb4, 0x40, 0xcd, 0x21, 0x72,
        5, // A read-only handle must reject the write.
        0xb8, 1, 0x4c, 0xcd, 0x21, 0xb8, 0, 0x4c, 0xcd, 0x21,
    ]);
    program.resize(0x200, 0);
    program.extend_from_slice(b"SHARED.DAT\0");
    program.resize(0x210, 0);
    program.extend_from_slice(b"new");
    fs::write(dir.0.join("SHARE.COM"), program).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .arg("--mount-rw")
        .arg(format!("C={}", dir.0.display()))
        .args(["--headless", "--exec", "share"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"new");
    assert_eq!(fs::read(dir.0.join("SHARED.DAT")).unwrap(), b"new");
}

#[test]
fn host_readonly_file_reports_dos_attributes_and_remains_readable() {
    let dir = Fixture::new();
    let data = dir.0.join("READONLY.DAT");
    fs::write(&data, b"readable").unwrap();
    let original_permissions = fs::metadata(&data).unwrap().permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&data, permissions).unwrap();

    let mut program = Vec::new();
    fn dos_call(program: &mut Vec<u8>, setup: &[u8]) {
        program.extend_from_slice(setup);
        program.extend_from_slice(&[0xcd, 0x21, 0x73, 5, 0xb8, 1, 0x4c, 0xcd, 0x21]);
    }
    dos_call(&mut program, &[0xba, 0, 3, 0xb8, 0, 0x43]);
    program.extend_from_slice(&[
        0x80, 0xe1, 1, 0x80, 0xf9, 1, // Check AH43h's readonly bit.
        0x74, 5, 0xb8, 1, 0x4c, 0xcd, 0x21,
    ]);
    dos_call(&mut program, &[0xba, 0, 3, 0xb9, 0, 0, 0xb8, 0, 0x4e]);
    program.extend_from_slice(&[
        0xa0, 0x95, 0, 0x24, 1, 0x3c, 1, // Check FindFirst's DTA attributes.
        0x74, 5, 0xb8, 1, 0x4c, 0xcd, 0x21,
    ]);
    dos_call(&mut program, &[0xba, 0, 3, 0xb8, 0, 0x3d]);
    program.extend_from_slice(&[0x89, 0xc3]);
    dos_call(&mut program, &[0xba, 0x20, 3, 0xb9, 8, 0, 0xb4, 0x3f]);
    dos_call(
        &mut program,
        &[0xbb, 1, 0, 0xba, 0x20, 3, 0xb9, 8, 0, 0xb4, 0x40],
    );
    program.extend_from_slice(&[
        0xba, 0, 3, 0xb8, 2, 0x3d, 0xcd, 0x21, 0x72, 5, 0xb8, 1, 0x4c, 0xcd, 0x21, 0xb8, 0, 0x4c,
        0xcd, 0x21,
    ]);
    program.resize(0x200, 0);
    program.extend_from_slice(b"READONLY.DAT\0");
    fs::write(dir.0.join("ATTR.COM"), program).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .arg("--mount-rw")
        .arg(format!("C={}", dir.0.display()))
        .args(["--headless", "--exec", "attr"])
        .output()
        .unwrap();
    fs::set_permissions(&data, original_permissions).unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"readable");
    assert_eq!(fs::read(&data).unwrap(), b"readable");
}
