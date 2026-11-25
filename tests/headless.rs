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
fn shell_help_lists_one_mount_command_and_keeps_startup_flags_in_cli_help() {
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args(args)
            .output()
            .unwrap()
    };
    let shell = run(&["--headless", "--exec", "HELP"]);
    assert!(shell.status.success(), "{shell:?}");
    let text = String::from_utf8(shell.stdout).unwrap();
    assert!(text.contains("MOUNT drive path"));
    assert!(!text.contains("IMGMOUNT"));
    assert!(!text.contains("--mount-rw"));

    let options = run(&["--headless", "--exec", "HELP MOUNT"]);
    assert!(options.status.success(), "{options:?}");
    let text = String::from_utf8(options.stdout).unwrap();
    for flag in ["/FAT", "/ISO", "/RW", "/SESSION"] {
        assert!(text.contains(flag));
    }
    let cli = run(&["--help"]);
    assert!(
        String::from_utf8(cli.stdout)
            .unwrap()
            .contains("--mount-rw")
    );
}

#[test]
fn shell_directory_commands_work_from_batch_and_prompt() {
    let fixture = Fixture::new();
    let path = fixture.0.display();
    let run = |mount: &str, command: &str| {
        Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([mount, &format!("C={path}"), "--headless", "--exec", command])
            .output()
            .unwrap()
    };
    fs::create_dir(fixture.0.join("Long Folder")).unwrap();
    fs::write(
        fixture.0.join("TREE.BAT"),
        "@echo off\r\ncd \"Long Folder\"\r\nmd CHILD\r\ncd CHILD\r\nmkdir INNER\r\nrd INNER\r\ncd ..\r\nrmdir CHILD\r\n",
    )
    .unwrap();
    let output = run("--mount-rw", "TREE");
    assert!(output.status.success(), "{output:?}");
    assert!(!fixture.0.join("Long Folder/CHILD").exists());

    let output = run("--mount-rw", "MD C:\\NEW");
    assert!(output.status.success(), "{output:?}");
    assert!(fixture.0.join("NEW").is_dir());
    let output = run("--mount", "RD NEW");
    assert!(!output.status.success(), "{output:?}");
    assert!(fixture.0.join("NEW").is_dir());
    let output = run("--mount-rw", "RD NEW");
    assert!(output.status.success(), "{output:?}");
    assert!(!fixture.0.join("NEW").exists());

    fs::write(
        fixture.0.join("ACTIVE.BAT"),
        "@echo off\r\ncd \"Long Folder\"\r\nrd C:\\LONGFO~1\r\n",
    )
    .unwrap();
    let output = run("--mount-rw", "ACTIVE");
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cannot remove the current directory")
    );
    assert!(fixture.0.join("Long Folder").is_dir());

    for command in ["RD \\", "RD MISSING", "MD MISSING\\CHILD", "MD", "MD A B"] {
        let output = run("--mount-rw", command);
        assert!(!output.status.success(), "{command}: {output:?}");
    }
    fs::write(fixture.0.join("Long Folder/FILE.TXT"), b"data").unwrap();
    let output = run("--mount-rw", "RD \"Long Folder\"");
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        fs::read(fixture.0.join("Long Folder/FILE.TXT")).unwrap(),
        b"data"
    );
}

#[test]
fn rename_handles_dos_wildcards_without_overwriting_files() {
    let fixture = Fixture::new();
    let mount = format!("C={}", fixture.0.display());
    let run = |access: &str, command: &str| {
        Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([access, &mount, "--headless", "--exec", command])
            .output()
            .unwrap()
    };
    fs::create_dir(fixture.0.join("FOLDER.TXT")).unwrap();
    fs::write(fixture.0.join("ALPHA.TXT"), b"alpha").unwrap();
    fs::write(fixture.0.join("BETA.TXT"), b"beta").unwrap();
    fs::write(
        fixture.0.join("RENAME.BAT"),
        b"@echo off\r\nren *.txt Z???.BAK\r\n",
    )
    .unwrap();
    let output = run("--mount-rw", "RENAME.BAT");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read(fixture.0.join("ZLPH.BAK")).unwrap(), b"alpha");
    assert_eq!(fs::read(fixture.0.join("ZETA.BAK")).unwrap(), b"beta");
    assert!(fixture.0.join("FOLDER.TXT").is_dir());

    for (access, command) in [
        ("--mount", "ren ZETA.BAK OTHER.BAK"),
        ("--mount-rw", "ren ZETA.BAK C:\\OTHER.BAK"),
        ("--mount-rw", "ren ZETA.BAK ZLPH.BAK"),
    ] {
        let output = run(access, command);
        assert!(!output.status.success(), "{command}: {output:?}");
        assert_eq!(fs::read(fixture.0.join("ZETA.BAK")).unwrap(), b"beta");
        assert_eq!(fs::read(fixture.0.join("ZLPH.BAK")).unwrap(), b"alpha");
    }
    let output = run("--mount-rw", "rename C:\\ZETA.BAK FINAL.");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read(fixture.0.join("FINAL")).unwrap(), b"beta");
    assert!(!fixture.0.join("ZETA.BAK").exists());
}

#[test]
fn copy_from_zip_combines_sources_and_follows_overwrite_rules() {
    use std::process::Stdio;
    use zip::write::SimpleFileOptions;

    let fixture = Fixture::new();
    let host = fixture.0.join("destination");
    fs::create_dir(&host).unwrap();
    let zip = fixture.0.join("input.zip");
    let mut archive = zip::ZipWriter::new(fs::File::create(&zip).unwrap());
    for (name, data) in [
        ("ALPHA.TXT", &b"A\x1aX"[..]),
        ("BETA.TXT", &b"B\x1aY"[..]),
        ("ALPHA.REF", &b"1"[..]),
        ("BETA.REF", &b"2"[..]),
    ] {
        archive
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        archive.write_all(data).unwrap();
    }
    archive.finish().unwrap();
    let c = format!("C={}", host.display());
    let d = format!("D={}", zip.display());
    let run = |command: &str, answers: &[u8]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([
                "--mount-rw",
                &c,
                "--mount",
                &d,
                "--headless",
                "--exec",
                command,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(answers).unwrap();
        child.wait_with_output().unwrap()
    };

    for (command, name, expected) in [
        ("COPY D:ALPHA.TXT", "ALPHA.TXT", &b"A\x1aX"[..]),
        ("COPY /A D:BETA.TXT OUT.TXT /A /V", "OUT.TXT", &b"B\x1a"[..]),
        ("COPY D:*.TXT MERGE.TXT", "MERGE.TXT", &b"AB"[..]),
        (
            "COPY /B D:ALPHA.TXT+D:BETA.TXT MERGE.BIN",
            "MERGE.BIN",
            &b"A\x1aXB\x1aY"[..],
        ),
        ("COPY D:*.TXT+D:*.REF *.DOC", "ALPHA.DOC", &b"A1"[..]),
    ] {
        let result = run(command, b"");
        assert!(result.status.success(), "{command}: {result:?}");
        assert_eq!(fs::read(host.join(name)).unwrap(), expected, "{command}");
    }
    assert_eq!(fs::read(host.join("BETA.DOC")).unwrap(), b"B2");

    let result = run("COPY D:BETA.TXT ALPHA.TXT", b"n");
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        result.stdout,
        b"Overwrite C:\\ALPHA.TXT (Y/N)? N\r\n0 file(s) copied.\r\n"
    );
    assert_eq!(fs::read(host.join("ALPHA.TXT")).unwrap(), b"A\x1aX");
    let result = run("COPY D:BETA.TXT ALPHA.TXT", b"y");
    assert!(result.status.success(), "{result:?}");
    assert_eq!(fs::read(host.join("ALPHA.TXT")).unwrap(), b"B\x1aY");

    for rejected in ["COPY ALPHA.TXT ALPHA.TXT /Y", "COPY ALPHA.TXT D:"] {
        let result = run(rejected, b"");
        assert!(!result.status.success(), "{rejected}: {result:?}");
    }
    fs::write(host.join("COPYING.BAT"), b"@echo off\r\ncopy D:ALPHA.TXT ALPHA.TXT\r\nset COPYCMD=/-Y\r\ncopy D:BETA.TXT ALPHA.TXT\r\ncopy /Y D:BETA.TXT OUT.TXT\r\necho done\r\n").unwrap();
    let result = run("COPYING", b"n");
    assert!(result.status.success(), "{result:?}");
    let output = String::from_utf8(result.stdout).unwrap();
    assert_eq!(output.matches("Overwrite").count(), 1);
    assert!(output.ends_with("done\r\n"), "{output}");
    assert_eq!(fs::read(host.join("ALPHA.TXT")).unwrap(), b"A\x1aX");
    assert_eq!(fs::read(host.join("OUT.TXT")).unwrap(), b"B\x1aY");
}

#[test]
fn delete_dos_files_with_and_without_confirmation() {
    use std::process::Stdio;

    let fixture = Fixture::new();
    let mount = format!("C={}", fixture.0.display());
    let run = |access: &str, command: &str, answers: &[u8]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([access, &mount, "--headless", "--exec", command])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(answers).unwrap();
        child.wait_with_output().unwrap()
    };
    let folder = fixture.0.join("FILES");
    fs::create_dir(&folder).unwrap();
    fs::create_dir(folder.join("KEEP.TXT")).unwrap();
    fs::write(folder.join("A.TXT"), b"a").unwrap();
    fs::write(folder.join("B.TXT"), b"b").unwrap();
    fs::write(folder.join("C"), b"c").unwrap();
    fs::write(
        fixture.0.join("DELETE.BAT"),
        b"@echo off\r\ndel FILES\\?.TXT /P\r\necho finished\r\n",
    )
    .unwrap();

    let result = run("--mount", "DEL FILES\\A.TXT", b"");
    assert!(!result.status.success(), "{result:?}");
    assert!(folder.join("A.TXT").is_file());

    let result = run("--mount-rw", "DELETE.BAT", b"n\r\ny");
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        result.stdout,
        b"Delete C:\\FILES\\A.TXT (Y/N)? N\r\nDelete C:\\FILES\\B.TXT (Y/N)? Y\r\nfinished\r\n"
    );
    assert!(folder.join("A.TXT").is_file());
    assert!(!folder.join("B.TXT").exists());

    let result = run("--mount-rw", "DEL FILES\\*.*", b"");
    assert!(!result.status.success(), "{result:?}");
    assert!(folder.join("A.TXT").is_file());
    let result = run("--mount-rw", "ERASE FILES", b"n");
    assert!(result.status.success(), "{result:?}");
    assert!(folder.join("C").is_file());

    let result = run("--mount-rw", "ERASE FILES\\*.TXT", b"");
    assert!(result.status.success(), "{result:?}");
    assert!(!folder.join("A.TXT").exists());
    let result = run("--mount-rw", "DEL FILES\\*", b"y");
    assert!(result.status.success(), "{result:?}");
    assert!(!folder.join("C").exists());
    assert!(folder.join("KEEP.TXT").is_dir());
}

#[test]
fn batch_environment_expands_and_reaches_a_guest_through_the_psp() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("ENV.COM"),
        [
            0x8e, 0x1e, 0x2c, 0, // MOV DS,[002Ch], PSP environment segment
            0xbb, 1, 0, // MOV BX,1, standard output
            0xb9, 0x80, 0, // MOV CX,128
            0x31, 0xd2, 0xb4, 0x40, 0xcd, 0x21, // Write from DS:0000
            0xb8, 0, 0x4c, 0xcd, 0x21, // Exit
        ],
    )
    .unwrap();
    fs::write(
        fixture.0.join("SETUP.BAT"),
        b"@echo off\r\nset demo=discarded\r\nset DEMO=one=two  \r\necho [%demo%]\r\ncall CHILD.BAT\r\nenv.com\r\nset demo=\r\necho [%demo%]\r\nset\r\n",
    )
    .unwrap();
    fs::write(fixture.0.join("CHILD.BAT"), b"echo child %DEMO%\r\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args([
            "--mount",
            &format!("C={}", fixture.0.display()),
            "--headless",
            "--exec",
            "SETUP",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(
        output
            .stdout
            .starts_with(b"[one=two  ]\r\nchild one=two\r\n")
    );
    assert!(
        output
            .stdout
            .windows(b"DEMO=one=two  \0".len())
            .any(|part| part == b"DEMO=one=two  \0")
    );
    assert!(
        output
            .stdout
            .windows(b"\0\0\x01\0C:\\ENV.COM\0".len())
            .any(|part| part == b"\0\0\x01\0C:\\ENV.COM\0")
    );
    assert!(
        output
            .stdout
            .ends_with(b"[]\r\nBLASTER=A220 I7 D1 T3\r\nCOMSPEC=COMMAND.COM\r\n")
    );
}

#[test]
fn path_search_preserves_the_shell_drive_and_directories() {
    let fixture = Fixture::new();
    for folder in ["C/WORK", "C/TOOLS", "D/BIN", "D/SVC"] {
        fs::create_dir_all(fixture.0.join(folder)).unwrap();
    }
    let mut check = Vec::new();
    let mut failures = Vec::new();
    let mut must_match = |bytes: &[u8]| {
        check.extend_from_slice(bytes);
        check.extend([0x75, 0]); // JNE failure
        failures.push(check.len() - 1);
    };
    must_match(&[0xb4, 0x19, 0xcd, 0x21, 0x3c, 2]); // Default drive C:
    for (drive, prefix) in [(0, *b"WO"), (4, *b"SV")] {
        must_match(&[
            0xbe, 0, 2, 0xb2, drive, 0xb4, 0x47, 0xcd, 0x21, // Query directory
            0x81, 0x3e, 0, 2, prefix[0], prefix[1], // Compare prefix at DS:0200h
        ]);
    }
    check.extend([0xb8, 42, 0x4c, 0xcd, 0x21]);
    let failure = check.len();
    check.extend([0xb8, 1, 0x4c, 0xcd, 0x21]);
    for offset in failures {
        check[offset] = u8::try_from(failure - offset - 1).unwrap();
    }
    fs::write(fixture.0.join("D/BIN/CHECK.COM"), check).unwrap();
    // A matching BAT in a later path entry distinguishes which directory won.
    fs::write(fixture.0.join("C/TOOLS/CHECK.BAT"), b"echo wrong\r\n").unwrap();
    fs::write(fixture.0.join("D/BIN/REPORT.BAT"), b"echo path report\r\n").unwrap();
    fs::write(
        fixture.0.join("C/WORK/LOCAL.BAT"),
        b"echo current directory\r\n",
    )
    .unwrap();
    fs::write(fixture.0.join("D/BIN/LOCAL.BAT"), b"echo path instead\r\n").unwrap();
    fs::write(
        fixture.0.join("C/START.BAT"),
        b"@echo off\r\npath\r\ncd C:\\WORK\r\ncd D:\\SVC\r\npath E:\\MISSING;D:\\BIN;C:\\TOOLS\r\ncall LOCAL\r\ncheck.com\r\necho check %ERRORLEVEL%\r\ncall report\r\npath;\r\npath\r\nset path=D:\\BIN\r\npath\r\ncall report\r\n",
    )
    .unwrap();
    let run = |command: &str| {
        Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([
                "--mount",
                &format!("C={}", fixture.0.join("C").display()),
                "--mount",
                &format!("D={}", fixture.0.join("D").display()),
                "--headless",
                "--exec",
                command,
            ])
            .output()
            .unwrap()
    };
    let output = run("START");
    assert_eq!(output.status.code(), Some(42), "{output:?}");
    assert_eq!(
        output.stdout,
        b"No Path\r\ncurrent directory\r\ncheck 42\r\npath report\r\nNo Path\r\nPATH=D:\\BIN\r\npath report\r\n"
    );

    fs::write(
        fixture.0.join("C/EXPLICIT.BAT"),
        b"@echo off\r\nset path=D:\\BIN\r\nC:\\CHECK.COM\r\n",
    )
    .unwrap();
    let output = run("EXPLICIT");
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("program not found: C:\\CHECK.COM"));
}

#[test]
fn prompt_formats_batch_echo_and_cd_still_reports_the_directory() {
    use dosa::shell::{Command as ShellCommand, Shell};

    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("WORK")).unwrap();
    fs::write(
        fixture.0.join("PROMPT.BAT"),
        b"@cd WORK\r\n@prompt [$p]$g \r\necho one\r\n@SET PROMPT=$N$Q$L$B$$ \r\necho two\r\n@prompt\r\necho three\r\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args([
            "--mount",
            &format!("C={}", fixture.0.display()),
            "--headless",
            "--exec",
            "PROMPT.BAT",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        output.stdout,
        b"[C:\\WORK]> echo one\r\none\r\nC=<|$ echo two\r\ntwo\r\nC:\\WORK> echo three\r\nthree\r\n"
    );

    let mut shell = Shell::new();
    shell
        .mount('C', dosa_mount::Mount::open(&fixture.0).unwrap())
        .unwrap();
    shell.execute("CD WORK").unwrap();
    shell.execute("PROMPT $V$_$E$H").unwrap();
    assert_eq!(shell.prompt(), "MS-DOS Version 5.00\r\n\x1b\x08");
    let ShellCommand::Print(directory) = shell.execute("CD").unwrap() else {
        panic!("CD did not report the current directory");
    };
    assert_eq!(directory, b"C:\\WORK\r\n");
    shell.execute("PROMPT $D $T").unwrap();
    let prompt = shell.prompt();
    assert_eq!(prompt.len(), 26);
    for (index, character) in [
        (3, b' '),
        (6, b'-'),
        (9, b'-'),
        (14, b' '),
        (17, b':'),
        (20, b':'),
        (23, b'.'),
    ] {
        assert_eq!(prompt.as_bytes()[index], character, "{prompt}");
    }
}

#[test]
fn batch_if_compares_strings_and_tests_dos_files() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("Long Folder")).unwrap();
    fs::create_dir(fixture.0.join("FAKE.DAT")).unwrap();
    fs::write(fixture.0.join("Long Folder/MARK.TXT"), b"data").unwrap();
    fs::write(
        fixture.0.join("VERIFY.BAT"),
        b"@echo off\r\n\
          set WORD=two words\r\n\
          if \"%WORD%\"==\"two words\" echo spaces\r\n\
          if not abc==ABC echo case sensitive\r\n\
          if \"abc\"==abc echo wrong quoting\r\n\
          if \"a==b\" == \"a==b\" echo embedded equals\r\n\
          if \"%UNDEFINED%\"==\"\" echo empty variable\r\n\
          if exist \"Long Folder\\M??K.*\" if errorlevel 0 echo wildcard\r\n\
          if exist \"Long Folder\\MARK.TXT\" echo exact file\r\n\
          if not exist \"Long Folder\" echo a directory is not a file\r\n\
          if exist \"Long Folder\\NUL\" echo directory probe\r\n\
          if not exist *.DAT echo wildcard excludes directories\r\n\
          if exist NUL echo null device\r\n\
          if not exist MISSING\\NUL echo missing directory\r\n\
          if not exist D:\\MARK.TXT echo unmounted drive\r\n",
    )
    .unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([
                "--mount",
                &format!("C={}", fixture.0.display()),
                "--headless",
                "--exec",
                "VERIFY",
            ])
            .output()
            .unwrap()
    };
    let output = run();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"spaces\r\ncase sensitive\r\nembedded equals\r\nempty variable\r\nwildcard\r\nexact file\r\na directory is not a file\r\ndirectory probe\r\nwildcard excludes directories\r\nnull device\r\nmissing directory\r\nunmounted drive\r\n");

    fs::write(
        fixture.0.join("VERIFY.BAT"),
        b"@echo off\r\nif exist NUL\r\n",
    )
    .unwrap();
    let output = run();
    assert!(!output.status.success(), "{output:?}");
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(message.contains("VERIFY.BAT:2"), "{message}");
    assert!(message.contains("IF [NOT] EXIST"), "{message}");
}

#[test]
fn batch_shift_exposes_later_arguments_and_keeps_call_frames_separate() {
    let fixture = Fixture::new();
    let mut source = String::from(
        "@echo off\r\necho [%0][%1][%9]\r\nshift\r\necho [%0][%1][%9]\r\ncall :INNER sub arg\r\necho [%0][%1][%9]\r\necho [%*]\r\n",
    );
    source.push_str(&"shift\r\n".repeat(12));
    source.push_str(
        "echo [%0][%1][%9]\r\nexit /b\r\n:INNER\r\nshift\r\necho [%0][%1]\r\nexit /b\r\n",
    );
    fs::write(fixture.0.join("SHIFTING.BAT"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args([
            "--mount",
            &format!("C={}", fixture.0.display()),
            "--headless",
            "--exec",
            "SHIFTING 1 \"two words\" 3 4 5 6 7 8 9 10 11",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"[SHIFTING][1][9]\r\n[1][\"two words\"][10]\r\n[sub][arg]\r\n[1][\"two words\"][10]\r\n[1 \"two words\" 3 4 5 6 7 8 9 10 11]\r\n[][][]\r\n");
}

#[test]
fn for_iterates_at_the_prompt_and_expands_batch_file_patterns() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("FILES")).unwrap();
    fs::create_dir(fixture.0.join("FILES/SUB.TXT")).unwrap();
    fs::write(fixture.0.join("FILES/ONE.TXT"), b"one\r\n").unwrap();
    fs::write(fixture.0.join("FILES/TWO.TXT"), b"two\r\n").unwrap();
    fs::write(
        fixture.0.join("SAY.COM"),
        [
            0xb4, 0x02, 0xb2, b'!', 0xcd, 0x21, 0xb8, 0, 0x4c, 0xcd, 0x21,
        ],
    )
    .unwrap();
    fs::write(
        fixture.0.join("LOOP.BAT"),
        b"@echo off\r\nset ITEM=expanded\r\nfor %%A in (%ITEM% \"two words\" %1) do echo [%%A][%%a]\r\n\
          for %%F in (FILES\\*.TXT MISSING\\*.TXT) do type %%F\r\n\
          if exist SAY.COM for %%P in (1 2) do SAY\r\necho done\r\n",
    )
    .unwrap();
    fs::write(fixture.0.join("PRINT.BAT"), b"@echo %1\r\n").unwrap();
    fs::write(
        fixture.0.join("JUMP.BAT"),
        b"@echo off\r\nfor %%A in (right missing) do goto %%A\r\necho skipped\r\n:right\r\necho jumped\r\n",
    )
    .unwrap();
    for (command, expected) in [
        ("JUMP", &b"jumped\r\n"[..]),
        (
            "FOR %A IN (one, two; three) DO ECHO %A",
            &b"one\r\ntwo\r\nthree\r\n"[..],
        ),
        ("FOR %A IN (one two) DO PRINT %A", &b"one\r\ntwo\r\n"[..]),
        (
            "LOOP argument",
            &b"[expanded][%a]\r\n[\"two words\"][%a]\r\n[argument][%a]\r\none\r\ntwo\r\n!!done\r\n"
                [..],
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([
                "--mount",
                &format!("C={}", fixture.0.display()),
                "--headless",
                "--exec",
                command,
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{command}: {output:?}");
        assert_eq!(output.stdout, expected, "{command}");
    }
}

#[test]
fn batch_pause_reads_headless_input_and_confirms_cancellation() {
    use std::process::Stdio;

    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("WAIT.BAT"),
        b"@echo off\r\necho before\r\npause\r\necho after\r\n",
    )
    .unwrap();
    let run = |keys: &[u8]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([
                "--mount",
                &format!("C={}", fixture.0.display()),
                "--headless",
                "--exec",
                "WAIT",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(keys).unwrap();
        child.wait_with_output().unwrap()
    };
    let output = run(b"x");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        output.stdout,
        b"before\r\nPress any key to continue . . .\r\nafter\r\n"
    );
    let output = run(b"\x03n");
    assert!(output.status.success(), "{output:?}");
    assert!(
        output
            .stdout
            .ends_with(b"Terminate batch job (Y/N)? N\r\nafter\r\n")
    );
    let output = run(b"\x03xy");
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(output.stdout.ends_with(b"Terminate batch job (Y/N)? Y\r\n"));
    let output = run(b"");
    assert!(!output.status.success(), "{output:?}");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("WAIT.BAT:3") && error.contains("PAUSE needs a key"),
        "{error}"
    );
}

#[test]
fn dir_formats_wide_bare_and_lowercase_listings() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("AAA")).unwrap();
    for letter in 'B'..='F' {
        fs::write(fixture.0.join(format!("{letter}.TXT")), b"file").unwrap();
    }
    fs::create_dir(fixture.0.join("Long Folder")).unwrap();
    fs::write(fixture.0.join("Long Folder/G.TXT"), b"nested").unwrap();
    let mut mount = std::ffi::OsString::from("C=");
    mount.push(&fixture.0);
    let run = |command| {
        Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args(["--headless", "--mount"])
            .arg(&mount)
            .args(["--exec", command])
            .output()
            .unwrap()
    };
    let output = run("DIR /w");
    assert!(output.status.success(), "{output:?}");
    let wide = String::from_utf8(output.stdout).unwrap();
    let rows: Vec<_> = wide.lines().skip(2).collect();
    assert_eq!(rows[0].split_ascii_whitespace().count(), 5);
    assert_eq!(rows[1].split_ascii_whitespace().count(), 2);
    assert!(rows[0].contains("[AAA]"));
    assert!(rows[0].contains("B.TXT"));

    let output = run("dir/w/b/l c:\\?.txt");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        output.stdout,
        b"b.txt\r\nc.txt\r\nd.txt\r\ne.txt\r\nf.txt\r\n"
    );
    let output = run(r#"DIR "C:\Long Folder\*.TXT" /B"#);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"G.TXT\r\n");
    let output = run("DIR /unknown");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid DIR switch"));
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
fn writable_zip_copy_allows_dos_read_write_access_without_changing_archive() {
    let dir = Fixture::new();
    let zip = dir.0.join("session copy.zip");
    let mut program = vec![
        0xba, 0x30, 0x01, 0xb8, 0x02, 0x3d, 0xcd, 0x21, // Open STATE.DAT read/write
        0x73, 4, 0xb4, 0x4c, 0xcd, 0x21, // On carry, return the DOS error code
        0x89, 0xc3, 0xba, 0x40, 0x01, 0xb9, 1, 0, 0xb4, 0x40, 0xcd, 0x21, // Write one byte
        0x73, 4, 0xb4, 0x4c, 0xcd, 0x21, // On carry, return the DOS error code
        0xb8, 0x00, 0x4c, 0xcd, 0x21,
    ];
    program.resize(0x30, 0);
    program.extend(b"STATE.DAT\0");
    program.resize(0x40, 0);
    program.push(b'Z');
    let mut archive = zip::ZipWriter::new(fs::File::create(&zip).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    archive.start_file("EDIT.COM", options).unwrap();
    archive.write_all(&program).unwrap();
    archive.start_file("STATE.DAT", options).unwrap();
    archive.write_all(b"ABCD").unwrap();
    archive.start_file("START.BAT", options).unwrap();
    archive
        .write_all(b"@echo off\nedit\nif errorlevel 1 exit /b 5\ntype state.dat\n")
        .unwrap();
    archive.finish().unwrap();
    let original = fs::read(&zip).unwrap();

    let profile = dir.0.join("session.toml");
    fs::write(
        &profile,
        "exec = 'start'\n[[mounts]]\ndrive = 'C'\npath = 'session copy.zip'\nsession_zip = true\n",
    )
    .unwrap();
    let mount = format!("C={}", zip.display());
    for args in [
        vec!["--mount", &mount, "--exec", "start"],
        vec!["--mount-session", &mount, "--exec", "start"],
        vec!["--profile", profile.to_str().unwrap()],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .arg("--headless")
            .args(&args)
            .output()
            .unwrap();
        if args[0] == "--mount" {
            assert_eq!(output.status.code(), Some(5), "{output:?}");
        } else {
            assert_eq!(output.status.code(), Some(0), "{output:?}");
            assert_eq!(output.stdout, b"ZBCD");
        }
        assert_eq!(fs::read(&zip).unwrap(), original);
    }
}

#[test]
fn cpu_fault_reports_an_earlier_read_only_open_failure() {
    let dir = Fixture::new();
    let zip = dir.0.join("program.zip");
    let mut program = vec![
        0xba, 0x20, 0x01, 0xb8, 0x02, 0x3d, 0xcd, 0x21, // Open STATE.DAT read/write
        0x73, 2, 0x0f, 0x0b, // Fault only if DOS reports an open failure
        0xb8, 0, 0x4c, 0xcd, 0x21,
    ];
    program.resize(0x20, 0);
    program.extend(b"STATE.DAT\0");

    let mut archive = zip::ZipWriter::new(fs::File::create(&zip).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    archive.start_file("RUN.COM", options).unwrap();
    archive.write_all(&program).unwrap();
    archive.start_file("STATE.DAT", options).unwrap();
    archive.write_all(b"DATA").unwrap();
    archive.finish().unwrap();

    let mount = format!("C={}", zip.display());
    let run = |flag: &str| {
        Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([flag, &mount, "--headless", "--exec", "run"])
            .output()
            .unwrap()
    };
    let output = run("--mount");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("Ud2"), "{output:?}");
    assert!(error.contains("write request to C: failed"), "{output:?}");

    let output = run("--mount-session");
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
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
    for command in ["mount", "imgmount"] {
        fs::write(
            guest.join("CHILD.BAT"),
            format!(
                "if errorlevel 0 {command} D \"{}\"\ntype D:\\OUTSIDE.TXT\n",
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
