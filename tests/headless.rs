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
fn loadhigh_falls_back_to_conventional_memory_with_normal_arguments_and_io() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("BIN")).unwrap();
    fs::write(
        fixture.0.join("BIN/PROBE.COM"),
        [
            0x0e, 0x58, 0x3d, 0x00, 0xa0, 0x72,
            0x05, // Verify CS is below the upper-memory area.
            0xb8, 0x08, 0x4c, 0xcd, 0x21, 0x31, 0xc9, 0x8a, 0x0e, 0x80,
            0x00, // CX = PSP command-tail length
            0xba, 0x81, 0x00, 0xbb, 1, 0, 0xb4, 0x40, 0xcd, 0x21, // Write the tail to stdout.
            0xb8, 7, 0x4c, 0xcd, 0x21,
        ],
    )
    .unwrap();
    fs::write(
        fixture.0.join("START.BAT"),
        b"@ECHO OFF\r\nPATH C:\\BIN\r\nLOADHIGH /L:1,256;2 /S PROBE batch tail>OUT.TXT\r\nECHO %ERRORLEVEL%\r\n",
    )
    .unwrap();
    let mount = format!("C={}", fixture.0.display());
    let run = |command| {
        Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([
                "--mount-rw",
                &mount,
                "--cpu",
                "8086",
                "--headless",
                "--exec",
                command,
            ])
            .output()
            .unwrap()
    };
    let direct = run(r#"LH "C:\BIN\PROBE.COM" one two"#);
    assert_eq!(direct.status.code(), Some(7), "{direct:?}");
    assert_eq!(direct.stdout, b" one two");
    let batch = run("START.BAT");
    assert_eq!(batch.status.code(), Some(7), "{batch:?}");
    assert_eq!(batch.stdout, b"7\r\n");
    assert_eq!(fs::read(fixture.0.join("OUT.TXT")).unwrap(), b" batch tail");
    for command in ["LH /S C:\\BIN\\PROBE", "LH /L:1; C:\\BIN\\PROBE", "LH /L:1"] {
        let invalid = run(command);
        assert!(!invalid.status.success(), "{command}: {invalid:?}");
    }
    for command in ["HELP LOADHIGH", "LH /?"] {
        let help = run(command);
        assert!(help.status.success(), "{help:?}");
        assert!(String::from_utf8_lossy(&help.stdout).contains("conventional memory"));
    }
}

#[test]
fn loadhigh_loads_com_and_mz_above_conventional_memory_when_they_fit() {
    let fixture = Fixture::new();
    let mut probe = [
        0x0e, 0x5a, // DX = CS (COM program segment)
        0xb4, 0x62, 0xcd, 0x21, 0x39, 0xd3, 0x75, 0x1f, // DOS returns PSP in BX
        0xb4, 0x2f, 0xcd, 0x21, 0x8c, 0xc0, 0x39, 0xd0, 0x75, 0x15, // DTA segment
        0x81, 0xfb, 0x80, 0x00, 0x75, 0x0f, // DTA offset
        0x3d, 0x00, 0xd0, 0x75, 0x05, // Compare returned segment with D000h
        0xb8, 7, 0x4c, 0xcd, 0x21, // exit 7: upper memory
        0xb8, 6, 0x4c, 0xcd, 0x21, // exit 6: conventional memory
        0xb8, 1, 0x4c, 0xcd, 0x21, // exit 1: inconsistent program data
    ];
    fs::write(fixture.0.join("PROBE.COM"), probe).unwrap();

    let mut mz = vec![0; 32];
    let relocation = (14 + probe.len()) as u16;
    let [relocation_lo, relocation_hi] = relocation.to_le_bytes();
    let relocation_check = [
        0x8c,
        0xc8, // MOV AX,CS
        0x2e,
        0x3b,
        0x06,
        relocation_lo,
        relocation_hi, // CMP AX, CS:[relocated word]
        0x74,
        5,
        0xb8,
        2,
        0x4c,
        0xcd,
        0x21, // exit 2: incorrect relocation
    ];
    mz[..2].copy_from_slice(b"MZ");
    let image_size = mz.len() as u16 + relocation + 2;
    mz[2..4].copy_from_slice(&image_size.to_le_bytes());
    mz[4] = 1; // Pages
    mz[6] = 1; // Relocation count
    mz[8] = 2; // Header paragraphs
    mz[0x10] = 0x80; // Initial SP
    mz[0x18] = 0x1c; // Relocation table offset
    mz[0x1c..0x1e].copy_from_slice(&relocation.to_le_bytes());
    mz.extend_from_slice(&relocation_check);
    probe[0] = 0x1e; // PUSH DS (EXE program segment)
    mz.extend_from_slice(&probe);
    mz.extend([0, 0]);
    fs::write(fixture.0.join("PROBE.EXE"), mz).unwrap();

    let mount = format!("C={}", fixture.0.display());
    for (cpu, command, expected) in [
        ("386", "LH PROBE.COM", 7),
        ("286", "LOADHIGH PROBE.EXE", 7),
        ("386", "LH /L:1,4096 /S PROBE.COM", 7),
        ("386", "LH /L:1,256 /S PROBE.COM", 6),
        ("386", "LH /L:1,131073 PROBE.EXE", 6),
        ("386", "LH /L:0;1 PROBE.EXE", 6),
        ("386", "PROBE.COM", 6),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([
                "--mount",
                &mount,
                "--cpu",
                cpu,
                "--headless",
                "--exec",
                command,
            ])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(expected),
            "{cpu} {command}: {output:?}"
        );
    }
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
fn batch_redirection_keeps_stream_positions_across_nested_commands() {
    let fixture = Fixture::new();
    let readonly = fixture.0.join("readonly");
    fs::create_dir(&readonly).unwrap();
    fs::write(readonly.join("INPUT.DAT"), b"ABCDE").unwrap();
    let pick = [
        0x31, 0xdb, 0xb9, 1, 0, 0xba, 0, 2, 0xb4, 0x3f, 0xcd, 0x21, // read one byte
        0x73, 5, 0xb8, 1, 0x4c, 0xcd, 0x21, 0x85, 0xc0, 0x74, 8, // no output at EOF
        0x8a, 0x16, 0, 2, 0xb4, 2, 0xcd, 0x21, // print the byte with AH=02h
        0xb8, 0, 0x4c, 0xcd, 0x21,
    ];
    fs::write(fixture.0.join("PICK.COM"), pick).unwrap();
    let mut switch = vec![
        0xba, 0, 2, 0xb8, 1, 0x3d, 0xcd, 0x21, // open NUL
        0x73, 5, 0xb8, 1, 0x4c, 0xcd, 0x21, 0x89, 0xc3, 0xb9, 1, 0, 0xb4, 0x46, 0xcd,
        0x21, // replace child stdout
        0x73, 5, 0xb8, 2, 0x4c, 0xcd, 0x21, 0x31, 0xdb, 0xb4, 0x3e, 0xcd,
        0x21, // close child stdin
        0x73, 5, 0xb8, 3, 0x4c, 0xcd, 0x21, 0xb2, b'X', 0xb4, 2, 0xcd, 0x21, 0xb8, 0, 0x4c, 0xcd,
        0x21,
    ];
    switch.resize(0x100, 0);
    switch.extend_from_slice(b"NUL\0");
    fs::write(fixture.0.join("SWITCH.COM"), switch).unwrap();
    fs::write(
        fixture.0.join("RUN.BAT"),
        b"@echo off\r\necho first\r\npick\r\nswitch\r\ncall :sub\r\npick >NUL\r\ncall CHILD.BAT >ALT.LOG\r\necho display >CON\r\npick\r\necho last\r\nexit /b 0\r\n:sub\r\npick\r\npause\r\ngoto :EOF\r\n",
    )
    .unwrap();
    fs::write(
        fixture.0.join("CHILD.BAT"),
        b"@echo on\r\necho inside\r\n@echo off\r\npick\r\n",
    )
    .unwrap();
    fs::write(fixture.0.join("OUT.LOG"), b"prior\r\n").unwrap();
    let run = |command: &str| {
        Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([
                "--headless",
                "--mount-rw",
                &format!("C={}", fixture.0.display()),
                "--mount",
                &format!("D={}", readonly.display()),
                "--exec",
                command,
            ])
            .output()
            .unwrap()
    };

    let result = run("RUN <D:INPUT.DAT >>OUT.LOG");
    assert!(result.status.success(), "{result:?}");
    assert_eq!(result.stdout, b"display\r\n");
    assert_eq!(
        fs::read(fixture.0.join("OUT.LOG")).unwrap(),
        b"prior\r\nfirst\r\nABPress any key to continue . . .\r\nlast\r\n"
    );
    assert_eq!(
        fs::read(fixture.0.join("ALT.LOG")).unwrap(),
        b"C:\\> echo inside\r\ninside\r\nE"
    );

    fs::write(
        fixture.0.join("OUTER.BAT"),
        b"@echo off\r\ncall RUN.BAT <D:INPUT.DAT >OTHER.LOG\r\necho restored\r\n",
    )
    .unwrap();
    let result = run("OUTER");
    assert!(result.status.success(), "{result:?}");
    assert_eq!(result.stdout, b"display\r\nrestored\r\n");
    assert_eq!(
        fs::read(fixture.0.join("OTHER.LOG")).unwrap(),
        b"first\r\nABPress any key to continue . . .\r\nlast\r\n"
    );
    fs::write(
        fixture.0.join("CHAIN.BAT"),
        b"@echo off\r\nNEXT.BAT\r\necho unreachable\r\n",
    )
    .unwrap();
    fs::write(fixture.0.join("NEXT.BAT"), b"echo chained\r\n").unwrap();
    let result = run("CHAIN >CHAIN.LOG");
    assert!(
        result.status.success() && result.stdout.is_empty(),
        "{result:?}"
    );
    assert_eq!(
        fs::read(fixture.0.join("CHAIN.LOG")).unwrap(),
        b"chained\r\n"
    );
}

#[test]
fn shell_redirects_files_devices_and_batch_commands() {
    use std::process::Stdio;

    let fixture = Fixture::new();
    let readonly = fixture.0.join("readonly");
    fs::create_dir(&readonly).unwrap();
    fs::write(readonly.join("INPUT.DAT"), b"A\x00\x1aZ").unwrap();
    fs::write(readonly.join("KEYS.TXT"), b"one\r\ntwo\r\n\x1a\r\n").unwrap();
    let filter = [
        0x31, 0xdb, 0xb9, 8, 0, 0xba, 0, 2, 0xb4, 0x3f, 0xcd, 0x21, // read stdin
        0x73, 5, 0xb8, 1, 0x4c, 0xcd, 0x21, 0x89, 0xc1, 0xbb, 1, 0, 0xb4, 0x40, 0xcd,
        0x21, // copy bytes to stdout
        0x73, 5, 0xb8, 2, 0x4c, 0xcd, 0x21, 0xb8, 0, 0x4c, 0xcd, 0x21,
    ];
    fs::write(fixture.0.join("FILTER.COM"), filter).unwrap();
    fs::write(
        fixture.0.join("RUN.BAT"),
        b"@echo off\r\necho first>LOG.TXT\r\nif exist LOG.TXT echo second>>LOG.TXT\r\nfilter <D:INPUT.DAT >COPY.DAT\r\ncopy con NOTE.TXT <D:KEYS.TXT >NUL\r\necho done\r\n",
    )
    .unwrap();
    let c = format!("C={}", fixture.0.display());
    let d = format!("D={}", readonly.display());
    let run = |command: &str, stdin: &[u8]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([
                "--headless",
                "--mount-rw",
                &c,
                "--mount",
                &d,
                "--exec",
                command,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin).unwrap();
        child.wait_with_output().unwrap()
    };

    let batch = run("RUN", b"");
    assert!(batch.status.success(), "{batch:?}");
    assert_eq!(batch.stdout, b"done\r\n");
    assert_eq!(
        fs::read(fixture.0.join("LOG.TXT")).unwrap(),
        b"first\r\nsecond\r\n"
    );
    assert_eq!(fs::read(fixture.0.join("COPY.DAT")).unwrap(), b"A\x00\x1aZ");
    assert_eq!(
        fs::read(fixture.0.join("NOTE.TXT")).unwrap(),
        b"one\r\ntwo\r\n"
    );

    let discard = run("ECHO quiet>NUL", b"");
    assert!(
        discard.status.success() && discard.stdout.is_empty(),
        "{discard:?}"
    );
    let literal = run("ECHO \"a>b\"", b"");
    assert_eq!(literal.stdout, b"\"a>b\"\r\n");
    let quoted_pipe = run("ECHO \"a|b\" | FILTER | FILTER", b"");
    assert!(quoted_pipe.status.success(), "{quoted_pipe:?}");
    assert_eq!(quoted_pipe.stdout, b"\"a|b\"\r\n");

    let binary_pipe = run("TYPE D:INPUT.DAT | FILTER | FILTER >PIPE.DAT", b"");
    assert!(binary_pipe.status.success(), "{binary_pipe:?}");
    assert!(binary_pipe.stdout.is_empty());
    assert_eq!(fs::read(fixture.0.join("PIPE.DAT")).unwrap(), b"A\x00\x1aZ");

    let override_input = run("ECHO ignored | FILTER <D:INPUT.DAT", b"");
    assert!(override_input.status.success(), "{override_input:?}");
    assert_eq!(override_input.stdout, b"A\x00\x1aZ");

    let override_output = run("ECHO saved >SAVE.LOG | FILTER", b"");
    assert!(override_output.status.success(), "{override_output:?}");
    assert!(override_output.stdout.is_empty());
    assert_eq!(fs::read(fixture.0.join("SAVE.LOG")).unwrap(), b"saved\r\n");

    fs::write(
        fixture.0.join("NESTED.BAT"),
        b"@echo inner | FILTER\r\n@exit /b\r\n@echo skipped\r\n",
    )
    .unwrap();
    fs::write(
        fixture.0.join("PIPES.BAT"),
        b"@echo off\r\necho first | FILTER\r\nNESTED.BAT | FILTER\r\necho last\r\n",
    )
    .unwrap();
    let batch_pipe = run("PIPES.BAT", b"");
    assert!(batch_pipe.status.success(), "{batch_pipe:?}");
    assert_eq!(batch_pipe.stdout, b"first\r\ninner\r\nlast\r\n");
    assert_eq!(run("NESTED.BAT | FILTER", b"").stdout, b"inner\r\n");

    let append = run("<D:INPUT.DAT FILTER>>COPY.DAT", b"");
    assert!(
        append.status.success() && append.stdout.is_empty(),
        "{append:?}"
    );
    assert_eq!(
        fs::read(fixture.0.join("COPY.DAT")).unwrap(),
        b"A\x00\x1aZA\x00\x1aZ"
    );
    let confirmation = run("COPY /-Y D:INPUT.DAT COPY.DAT >PROMPT.TXT", b"n");
    assert!(
        confirmation.status.success() && confirmation.stdout.is_empty(),
        "{confirmation:?}"
    );
    assert_eq!(
        fs::read(fixture.0.join("PROMPT.TXT")).unwrap(),
        b"Overwrite C:\\COPY.DAT (Y/N)? N\r\n0 file(s) copied.\r\n"
    );

    for command in [
        "ECHO hello |",
        "| ECHO hello",
        "ECHO hello >",
        "ECHO hello >X.TXT >Y.TXT",
        "ECHO no >D:BAD.TXT",
        "FILTER <D:MISSING >SAFE.TXT",
    ] {
        let result = run(command, b"");
        assert!(!result.status.success(), "{command}: {result:?}");
    }
    for name in ["X.TXT", "Y.TXT", "SAFE.TXT"] {
        assert!(!fixture.0.join(name).exists(), "{name}");
    }
    assert!(!readonly.join("BAD.TXT").exists());
}

#[test]
fn legacy_character_input_follows_redirected_stdin_until_eof() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("INPUT.DAT"), b"A\x00\x03BFirst\r\nPartial").unwrap();
    let mut code = Vec::new();
    for (index, (setup, check, branch)) in [
        (&[0xba, 0, 3, 0xb8, 0, 0x3d][..], &[][..], 0x73), // open input
        (&[0x89, 0xc3, 0x31, 0xc9, 0xb4, 0x46][..], &[][..], 0x73), // replace stdin
        (&[0xb4, 0x0b][..], &[0x3c, 0xff][..], 0x74),      // input exists
        (&[0xb4, 1][..], &[0x3c, b'A'][..], 0x74),         // read and echo A
        (&[0xb2, 0xff, 0xb4, 6][..], &[][..], 0x75),       // zero byte is data
        (&[0xb4, 8][..], &[0x3c, 3][..], 0x74),            // Ctrl+C is data with BREAK OFF
        (&[0xb8, 7, 0x0c][..], &[0x3c, b'B'][..], 0x74),   // flush then read B
        (
            &[0xc6, 6, 0x20, 3, 16, 0xba, 0x20, 3, 0xb4, 0x0a][..],
            &[0x81, 0x3e, 0x21, 3, 5, b'F'][..],
            0x74,
        ), // read the first line
        (
            &[0xba, 0x20, 3, 0xb4, 0x0a][..],
            &[0x81, 0x3e, 0x21, 3, 7, b'P'][..],
            0x74,
        ), // finish the partial line at EOF
        (
            &[0xba, 0x20, 3, 0xb4, 0x0a][..],
            &[0x81, 0x3e, 0x21, 3, 0, 13][..],
            0x74,
        ), // empty line on repeat
        (&[0xb4, 0x0b][..], &[0x3c, 0][..], 0x74),         // no data remains
        (&[0xb2, 0xff, 0xb4, 6][..], &[][..], 0x74),       // polling at EOF sets ZF
    ]
    .into_iter()
    .enumerate()
    {
        code.extend_from_slice(setup);
        code.extend([0xcd, 0x21]);
        code.extend_from_slice(check);
        code.extend([branch, 5, 0xb8, index as u8 + 1, 0x4c, 0xcd, 0x21]);
    }
    code.extend([0xb8, 0, 0x4c, 0xcd, 0x21]);
    code.resize(0x200, 0);
    code.extend_from_slice(b"INPUT.DAT\0");
    fs::write(fixture.0.join("READIN.COM"), code).unwrap();

    let mount = format!("C={}", fixture.0.display());
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args(["--headless", "--mount", &mount, "--exec", "READIN"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"AFirst\r\nPartial");
}

#[test]
fn ctty_routes_default_dos_streams_and_preserves_explicit_redirection() {
    let fixture = Fixture::new();
    let mut program = vec![
        0xba, 0, 2, 0x31, 0xdb, 0xb9, 1, 0, 0xb4, 0x3f, 0xcd, 0x21, // read stdin
        0x85, 0xc0, 0x74, 7, 0x43, 0xb4, 0x40, 0xcd, 0x21, 0xeb, 7, // copy input to stdout
        0xb2, b'S', 0xb4, 2, 0xcd, 0x21, 0x90, // write to stdout on EOF
        0xba, 1, 2, 0xbb, 2, 0, 0xb9, 1, 0, 0xb4, 0x40, 0xcd, 0x21, // write to stderr
        0xb0, b'B', 0xcd, 0x29, // write directly to the screen
        0xb8, 0, 0x4c, 0xcd, 0x21,
    ];
    program.resize(0x100, 0);
    program.extend_from_slice(b"\0E");
    fs::write(fixture.0.join("STREAMS.COM"), program).unwrap();
    fs::write(fixture.0.join("INPUT.TXT"), b"R").unwrap();
    fs::write(
        fixture.0.join("RUN.BAT"),
        b"@echo off\r\nctty nul:\r\necho hidden\r\nstreams\r\nstreams <INPUT.TXT >COPY.TXT\r\necho explicit >CON\r\ncall CHILD\r\necho restored\r\n",
    )
    .unwrap();
    fs::write(fixture.0.join("CHILD.BAT"), b"ctty con\r\necho child\r\n").unwrap();
    let mount = format!("C={}", fixture.0.display());
    let run = |command| {
        Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args(["--headless", "--mount-rw", &mount, "--exec", command])
            .output()
            .unwrap()
    };
    let batch = run("RUN");
    assert!(batch.status.success(), "{batch:?}");
    assert_eq!(batch.stdout, b"BBexplicit\r\nchild\r\nrestored\r\n");
    assert_eq!(fs::read(fixture.0.join("COPY.TXT")).unwrap(), b"R");

    let redirected = run("RUN >BATCH.TXT");
    assert!(redirected.status.success(), "{redirected:?}");
    assert_eq!(redirected.stdout, b"BBexplicit\r\n");
    assert_eq!(
        fs::read(fixture.0.join("BATCH.TXT")).unwrap(),
        b"hidden\r\nSchild\r\nrestored\r\n"
    );

    let invalid = run("CTTY COM1");
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("Invalid device"));
}

#[test]
fn legacy_character_output_follows_redirected_stdout() {
    let fixture = Fixture::new();
    let mut program = vec![
        0xba, 0, 2, 0x31, 0xc9, 0xb4, 0x3c, 0xcd, 0x21, // create LEGACY.DAT
        0x73, 5, 0xb8, 1, 0x4c, 0xcd, 0x21, 0x89, 0xc3, 0xb9, 1, 0, 0xb4, 0x46, 0xcd,
        0x21, // redirect stdout to file
        0x73, 5, 0xb8, 2, 0x4c, 0xcd, 0x21, 0xb2, b'A', 0xb4, 0x02, 0xcd, 0x21, // AH=02h
        0xb2, b'B', 0xb4, 0x06, 0xcd, 0x21, // AH=06h, output
        0xba, 15, 2, 0xb4, 0x09, 0xcd, 0x21, // AH=09h, dollar-terminated
        0xbb, 1, 0, 0xba, 1, 0, 0x31, 0xc9, 0xb8, 0, 0x42, 0xcd, 0x21, // seek into stdout
        0xba, 17, 2, 0xb4, 0x09, 0xcd, 0x21, // empty string must not truncate the file
        0xb0, b'X', 0xcd, 0x29, // unredirected fast console write
        0xba, 11, 2, 0xb8, 1, 0x3d, 0xcd, 0x21, // open NUL for writing
        0x73, 5, 0xb8, 3, 0x4c, 0xcd, 0x21, 0x89, 0xc3, 0xb9, 1, 0, 0xb4, 0x46, 0xcd,
        0x21, // replace stdout with NUL
        0x73, 5, 0xb8, 4, 0x4c, 0xcd, 0x21, 0xb2, b'Z', 0xb4, 0x02, 0xcd, 0x21, 0xba, 15, 2, 0xb4,
        0x09, 0xcd, 0x21, 0xb0, b'Y', 0xcd, 0x29, 0xb8, 0, 0x4c, 0xcd, 0x21,
    ];
    program.resize(0x100, 0);
    program.extend_from_slice(b"LEGACY.DAT\0NUL\0C\x9b$");
    fs::write(fixture.0.join("CHAROUT.COM"), program).unwrap();

    let mount = format!("C={}", fixture.0.display());
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args(["--headless", "--mount-rw", &mount, "--exec", "CHAROUT"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"XY");
    assert_eq!(fs::read(fixture.0.join("LEGACY.DAT")).unwrap(), b"ABC\x9b");
}

#[test]
fn guest_duplicates_share_offsets_and_can_redirect_standard_output() {
    let fixture = Fixture::new();
    let mut code = Vec::new();
    for (index, (setup, check, pass)) in [
        (&[0xbb, 1, 0, 0xb4, 0x45][..], &[][..], 0x73), // save stdout
        (
            &[0x89, 0xc7, 0xba, 0, 3, 0x31, 0xc9, 0xb4, 0x3c][..],
            &[][..],
            0x73,
        ), // create OUT.DAT
        (&[0x89, 0xc6, 0x89, 0xf3, 0xb4, 0x45][..], &[][..], 0x73), // duplicate file
        (
            &[0x89, 0xc5, 0x89, 0xf3, 0xb9, 1, 0, 0xb4, 0x46][..],
            &[][..],
            0x73,
        ), // redirect stdout
        (&[0x89, 0xf3, 0xb4, 0x3e][..], &[][..], 0x73), // close original
        (&[0xbb, 1, 0, 0xb8, 0, 0x44][..], &[0x83, 0xfa, 2][..], 0x74), // stdout is drive C
        (
            &[0xbb, 1, 0, 0xb9, 2, 0, 0xba, 8, 3, 0xb4, 0x40][..],
            &[0x83, 0xf8, 2][..],
            0x74,
        ), // write AB via stdout
        (
            &[0x89, 0xeb, 0xb9, 2, 0, 0xba, 10, 3, 0xb4, 0x40][..],
            &[0x83, 0xf8, 2][..],
            0x74,
        ), // append CD via duplicate
        (
            &[0xbb, 1, 0, 0x31, 0xc9, 0x31, 0xd2, 0xb8, 0, 0x42][..],
            &[][..],
            0x73,
        ), // rewind stdout
        (
            &[0x89, 0xeb, 0xb9, 2, 0, 0xba, 0x20, 3, 0xb4, 0x3f][..],
            &[0x81, 0x3e, 0x20, 3, b'A', b'B'][..],
            0x74,
        ), // read AB via duplicate
        (
            &[0xbb, 1, 0, 0xb9, 2, 0, 0xba, 0x20, 3, 0xb4, 0x3f][..],
            &[0x81, 0x3e, 0x20, 3, b'C', b'D'][..],
            0x74,
        ), // read CD via stdout
        (&[0x89, 0xfb, 0xb9, 1, 0, 0xb4, 0x46][..], &[][..], 0x73), // restore stdout
        (&[0x89, 0xfb, 0xb4, 0x3e][..], &[][..], 0x73), // close saved handle
        (
            &[0xbb, 1, 0, 0xb9, 2, 0, 0xba, 12, 3, 0xb4, 0x40][..],
            &[0x83, 0xf8, 2][..],
            0x74,
        ), // print OK
    ]
    .into_iter()
    .enumerate()
    {
        code.extend_from_slice(setup);
        code.extend([0xcd, 0x21]);
        code.extend_from_slice(check);
        code.extend([pass, 5, 0xb8, index as u8 + 1, 0x4c, 0xcd, 0x21]);
    }
    code.extend([0xb8, 0, 0x4c, 0xcd, 0x21]);
    assert!(code.len() <= 0x200);
    code.resize(0x200, 0);
    code.extend_from_slice(b"OUT.DAT\0ABCDOK");
    fs::write(fixture.0.join("DUP.COM"), code).unwrap();

    let mount = format!("C={}", fixture.0.display());
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args(["--headless", "--mount-rw", &mount, "--exec", "DUP"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"OK");
    assert_eq!(fs::read(fixture.0.join("OUT.DAT")).unwrap(), b"ABCD");
}

#[test]
fn copy_con_reads_edited_lines_and_prints_files() {
    use std::process::Stdio;

    let fixture = Fixture::new();
    let mount = format!("C={}", fixture.0.display());
    fs::write(fixture.0.join("STREAM.DAT"), b"visible\r\n\x1ahidden").unwrap();
    fs::write(
        fixture.0.join("RUN.BAT"),
        b"@echo off\r\ncopy con BATCH.TXT\r\necho done\r\n",
    )
    .unwrap();
    let run = |command: &str, input: &[u8]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args(["--headless", "--mount-rw", &mount, "--exec", command])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        child.wait_with_output().unwrap()
    };

    let typed = run("COPY /V CON OUT.TXT", b"one\r\nx\x08two\n\x1a\n");
    assert!(typed.status.success(), "{typed:?}");
    assert!(typed.stdout.ends_with(b"^Z\r\n1 file(s) copied.\r\n"));
    assert_eq!(
        fs::read(fixture.0.join("OUT.TXT")).unwrap(),
        b"one\r\ntwo\r\n"
    );

    let decline = run("COPY /-Y CON OUT.TXT", b"n");
    assert!(decline.status.success(), "{decline:?}");
    assert!(decline.stdout.ends_with(b"N\r\n0 file(s) copied.\r\n"));
    let cancel = run("COPY /-Y CON OUT.TXT", b"ypartial\x03");
    assert!(!cancel.status.success(), "{cancel:?}");
    assert!(cancel.stdout.ends_with(b"^C\r\n"));
    assert_eq!(
        fs::read(fixture.0.join("OUT.TXT")).unwrap(),
        b"one\r\ntwo\r\n"
    );

    let batch = run("RUN", b"batch\x1a\r\n");
    assert!(batch.status.success(), "{batch:?}");
    assert!(batch.stdout.ends_with(b"done\r\n"));
    assert_eq!(fs::read(fixture.0.join("BATCH.TXT")).unwrap(), b"batch");

    let combined = run("COPY CON+CON PAIR.TXT", b"first\x1a\r\nsecond\x1a\r\n");
    assert!(combined.status.success(), "{combined:?}");
    assert_eq!(
        fs::read(fixture.0.join("PAIR.TXT")).unwrap(),
        b"firstsecond"
    );

    let text = run("COPY STREAM.DAT CON", b"");
    assert!(text.status.success(), "{text:?}");
    assert_eq!(text.stdout, b"visible\r\n1 file(s) copied.\r\n");
    let binary = run("COPY STREAM.DAT CON /B", b"");
    assert!(binary.status.success(), "{binary:?}");
    assert_eq!(binary.stdout, b"visible\r\n\x1ahidden1 file(s) copied.\r\n");
}

#[test]
fn copy_accepts_null_sources_and_destinations() {
    let fixture = Fixture::new();
    let writable = fixture.0.join("writable");
    let readonly = fixture.0.join("readonly");
    fs::create_dir(&writable).unwrap();
    fs::create_dir(&readonly).unwrap();
    fs::create_dir(readonly.join("SUB")).unwrap();
    fs::write(readonly.join("SOURCE.DAT"), b"data\x1amore").unwrap();
    fs::write(writable.join("EMPTY.DAT"), b"replace me").unwrap();
    let c = format!("C={}", writable.display());
    let d = format!("D={}", readonly.display());
    let run = |command: &str| {
        Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([
                "--headless",
                "--mount-rw",
                &c,
                "--mount",
                &d,
                "--exec",
                command,
            ])
            .output()
            .unwrap()
    };

    for command in [
        "COPY /-Y D:SOURCE.DAT NUL",
        "COPY D:SOURCE.DAT D:\\SUB\\NUL.TXT /V",
        "COPY /Y D:\\SUB\\NUL EMPTY.DAT",
        "COPY /B NUL+D:SOURCE.DAT+NUL SAME.DAT",
        "COPY D:SOURCE.DAT+NUL TEXT.DAT",
    ] {
        let result = run(command);
        assert!(result.status.success(), "{command}: {result:?}");
        assert!(
            result.stdout.ends_with(b"1 file(s) copied.\r\n"),
            "{command}: {result:?}"
        );
    }
    assert_eq!(fs::read(writable.join("EMPTY.DAT")).unwrap(), b"");
    assert_eq!(
        fs::read(writable.join("SAME.DAT")).unwrap(),
        b"data\x1amore"
    );
    assert_eq!(fs::read(writable.join("TEXT.DAT")).unwrap(), b"data");
    assert_eq!(fs::read_dir(&writable).unwrap().count(), 3);
    assert_eq!(
        fs::read(readonly.join("SOURCE.DAT")).unwrap(),
        b"data\x1amore"
    );
    assert!(fs::read_dir(readonly.join("SUB")).unwrap().next().is_none());

    for command in [
        "COPY D:MISSING.DAT NUL",
        "COPY D:SOURCE.DAT D:\\MISSING\\NUL",
        "COPY NUL D:WRITE.DAT",
        "COPY NUL",
    ] {
        let result = run(command);
        assert!(!result.status.success(), "{command}: {result:?}");
    }
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
                "VERIFY.BAT",
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
fn dos_file_dates_match_directory_sorting_and_guest_services() {
    use std::fs::FileTimes;
    use std::time::Duration;
    use zip::write::SimpleFileOptions;

    let fixture = Fixture::new();
    let path = fixture.0.join("TIMES.ZIP");
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    for (name, timestamp) in [
        (
            "NEW.DAT",
            zip::DateTime::from_date_and_time(2001, 1, 1, 0, 0, 0).unwrap(),
        ),
        (
            "OLD.DAT",
            zip::DateTime::from_date_and_time(2000, 2, 29, 15, 4, 6).unwrap(),
        ),
    ] {
        zip.start_file(
            name,
            SimpleFileOptions::default().last_modified_time(timestamp),
        )
        .unwrap();
        zip.write_all(b"x").unwrap();
    }
    zip.finish().unwrap();
    let host_file = fs::File::create(fixture.0.join("OLD.DAT")).unwrap();
    host_file
        .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(951_836_646)))
        .unwrap();

    // FindFirst and Get File Date and Time must return 2000-02-29 15:04:06.
    let mut probe = vec![
        0xba, 0, 2, 0x31, 0xc9, 0xb4, 0x4e, 0xcd, 0x21, 0x73, 5, 0xb8, 1, 0x4c, 0xcd, 0x21, 0xa1,
        0x96, 0, 0x3d, 0x83, 0x78, 0x74, 5, 0xb8, 2, 0x4c, 0xcd, 0x21, 0xa1, 0x98, 0, 0x3d, 0x5d,
        0x28, 0x74, 5, 0xb8, 3, 0x4c, 0xcd, 0x21, 0xba, 0, 2, 0xb8, 0, 0x3d, 0xcd, 0x21, 0x73, 5,
        0xb8, 4, 0x4c, 0xcd, 0x21, 0x89, 0xc3, 0xb8, 0, 0x57, 0xcd, 0x21, 0x73, 5, 0xb8, 5, 0x4c,
        0xcd, 0x21, 0x81, 0xf9, 0x83, 0x78, 0x74, 5, 0xb8, 6, 0x4c, 0xcd, 0x21, 0x81, 0xfa, 0x5d,
        0x28, 0x74, 5, 0xb8, 7, 0x4c, 0xcd, 0x21, 0xb8, 0, 0x4c, 0xcd, 0x21,
    ];
    probe.resize(0x100, 0);
    probe.extend_from_slice(b"D:\\OLD.DAT\0");
    fs::write(fixture.0.join("ZIPTIME.COM"), &probe).unwrap();
    probe[0x100] = b'C';
    fs::write(fixture.0.join("HOSTTIME.COM"), &probe).unwrap();

    let c = format!("C={}", fixture.0.display());
    let d = format!("D={}", path.display());
    let run = |command: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([
                "--mount-rw",
                &c,
                "--mount",
                &d,
                "--headless",
                "--exec",
                command,
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{command}: {output:?}");
        output.stdout
    };
    assert_eq!(run("DIR D:\\ /B/OD"), b"OLD.DAT\r\nNEW.DAT\r\n");
    assert_eq!(run("DIR D:\\ /B/O-D"), b"NEW.DAT\r\nOLD.DAT\r\n");
    for file in ["C:\\OLD.DAT", "D:\\OLD.DAT"] {
        let listing = String::from_utf8(run(&format!("DIR {file}"))).unwrap();
        assert!(listing.contains("02-29-00   3:04p"), "{listing}");
    }
    assert!(run("ZIPTIME").is_empty());
    assert!(run("HOSTTIME").is_empty());

    fs::write(
        fixture.0.join("COPYTIME.BAT"),
        b"@echo off\r\ndate 06-15-2004\r\ntime 09:30\r\ncopy D:\\OLD.DAT SAME.TMP\r\ncopy /y D:\\NEW.DAT SAME.TMP\r\ncopy D:\\OLD.DAT SINGLE.TMP\r\ncopy D:\\OLD.DAT+D:\\NEW.DAT BOTH.TMP\r\ndir *.tmp\r\n",
    )
    .unwrap();
    let listing = String::from_utf8(run("COPYTIME")).unwrap();
    for (name, timestamp) in [
        ("SAME.TMP", "01-01-01  12:00a"),
        ("SINGLE.TMP", "02-29-00   3:04p"),
        ("BOTH.TMP", "06-15-04   9:30a"),
    ] {
        let row = listing
            .lines()
            .find(|line| line.starts_with(name) && line.contains(timestamp));
        assert!(row.is_some(), "{name} at {timestamp}: {listing}");
    }
    assert_eq!(fs::read(fixture.0.join("BOTH.TMP")).unwrap(), b"xx");
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
    let rows: Vec<_> = wide
        .lines()
        .skip_while(|line| !line.contains("Directory of"))
        .skip(2)
        .collect();
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
fn dir_accepts_dos_compression_switches_on_regular_mounts() {
    let fixture = Fixture::new();
    let path = fixture.0.join("compressed-host.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in [
        ("Z.TXT", &b"z"[..]),
        ("A.TXT", &[b'a'; 1024]),
        (
            "TEST.BAT",
            &b"@ECHO OFF\r\nSET DIRCMD=/CH /O:CN\r\nDIR *.TXT /-C /B\r\n"[..],
        ),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    let drive = format!("C={}", path.display());
    let run = |command| {
        let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args(["--headless", "--mount", &drive, "--exec", command])
            .output()
            .unwrap();
        assert!(output.status.success(), "{command}: {output:?}");
        output.stdout
    };
    let normal = run("DIR *.TXT");
    assert_eq!(run("DIR *.TXT /C"), normal);
    assert_eq!(run("DIR *.TXT /CH"), normal);
    let wide = run("DIR *.TXT /W");
    assert_eq!(run("DIR *.TXT /W/CH"), wide);
    assert_eq!(run("DIR *.TXT /B /O:-C-N"), b"Z.TXT\r\nA.TXT\r\n");
    assert_eq!(run("TEST.BAT"), b"A.TXT\r\nZ.TXT\r\n");
}

#[test]
fn dir_reports_allocatable_space_for_writable_host_mounts() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("DATA.TXT"), vec![0; 1024]).unwrap();
    let c = format!("C={}", fixture.0.display());
    let run = |access, command| {
        let result = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args(["--headless", access, &c, "--exec", command])
            .output()
            .unwrap();
        assert!(result.status.success(), "{command}: {result:?}");
        String::from_utf8(result.stdout).unwrap()
    };
    assert_eq!(
        run("--mount-rw", "VOL C:"),
        " Volume in drive C has no label\r\n"
    );
    assert_eq!(run("--mount-rw", "DIR /B"), "DATA.TXT\r\n");
    let directory = run("--mount-rw", "DIR");
    assert!(directory.lines().any(|line| {
        line.split_whitespace()
            .eq(["1", "file(s)", "1,024", "bytes"])
    }));
    let free = directory
        .lines()
        .last()
        .unwrap()
        .trim()
        .strip_suffix(" bytes free")
        .unwrap();
    assert!(
        free.replace(',', "").parse::<u64>().unwrap() > 0,
        "{directory}"
    );
    assert!(run("--mount", "DIR").ends_with(" 0 bytes free\r\n"));
}

#[test]
fn date_and_time_share_the_guest_clock_and_reprompt_for_invalid_lines() {
    use std::process::Stdio;

    let fixture = Fixture::new();
    let mut probe = vec![
        0xb4, 0x2a, 0xcd, 0x21, // get date
        0x81, 0xf9, 0xd0, 7, 0x74, 5, // year 2000
        0xb8, 1, 0x4c, 0xcd, 0x21, 0x81, 0xfa, 0x1d, 2, 0x74, 5, // February 29
        0xb8, 2, 0x4c, 0xcd, 0x21, 0x3c, 2, 0x74, 5, 0xb8, 3, 0x4c, 0xcd, 0x21, // Tuesday
        0xb4, 0x2c, 0xcd, 0x21, 0x81, 0xf9, 36, 13, 0x74, 5, 0xb8, 4, 0x4c, 0xcd,
        0x21, // 13:36
    ];
    for (service, cx, dx, status) in [
        (0x2b, [0xd4, 7], [29, 2], 0),   // 2004-02-29
        (0x2b, [0xd4, 7], [30, 2], 255), // invalid day
        (0x2d, [15, 10], [0, 0], 0),     // 10:15
        (0x2d, [0, 24], [0, 0], 255),    // invalid hour
    ] {
        probe.extend([
            0xb9, cx[0], cx[1], 0xba, dx[0], dx[1], 0xb4, service, 0xcd, 0x21, 0x3c, status, 0x74,
            5, 0xb8, 5, 0x4c, 0xcd, 0x21,
        ]);
    }
    probe.extend([0xb8, 0, 0x4c, 0xcd, 0x21]);
    fs::write(fixture.0.join("CLOCK.COM"), probe).unwrap();
    fs::write(
        fixture.0.join("SETUP.BAT"),
        b"@echo off\r\ndate\r\npause\r\ntime\r\nclock\r\nprompt $D $T $G\r\necho on\r\ndate /t\r\ntime /t\r\n",
    )
    .unwrap();
    let mount = format!("C={}", fixture.0.display());
    let mut child = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args(["--headless", "--mount", &mount, "--exec", "SETUP"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"02/30/2000\r\n02/29/2000\r\nx1:36p\r\n")
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success(), "{result:?}");
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(
        output.contains("Invalid date\r\nEnter new date"),
        "{output}"
    );
    assert!(
        output.contains("Current date is Sun 02-29-2004"),
        "{output}"
    );
    assert!(output.contains("Current time is 10:15:"), "{output}");
    assert!(output.contains("Sun 02-29-2004 10:15:"), "{output}");
}

#[test]
fn chcp_shares_the_active_page_with_guest_programs_and_oem_batch_text() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("PAGE.COM"),
        [
            0xb8, 1, 0x66, 0xcd, 0x21, // Query active and system pages
            0x73, 5, 0xb8, 1, 0x4c, 0xcd, 0x21, 0x81, 0xfb, 0x52, 3, 0x74, 5, 0xb8, 2, 0x4c, 0xcd,
            0x21, // BX = 850
            0x81, 0xfa, 0xb5, 1, 0x74, 5, 0xb8, 3, 0x4c, 0xcd, 0x21, // DX = 437
            0xbb, 0, 0, 0xb8, 2, 0x66, 0xcd, 0x21, // Reject unprepared page
            0x72, 5, 0xb8, 4, 0x4c, 0xcd, 0x21, 0xbb, 0xb5, 1, 0xb8, 2, 0x66, 0xcd,
            0x21, // Switch back to 437
            0x73, 5, 0xb8, 5, 0x4c, 0xcd, 0x21, 0xb8, 0, 0x4c, 0xcd, 0x21,
        ],
    )
    .unwrap();
    fs::write(fixture.0.join("OEM.BAT"), b"@echo \x9b\r\n").unwrap();
    fs::write(
        fixture.0.join("SETUP.BAT"),
        b"@echo off\r\nchcp\r\nchcp 850\r\nchcp\r\ncall OEM.BAT\r\nPAGE\r\nif errorlevel 1 exit %errorlevel%\r\nchcp\r\n",
    )
    .unwrap();
    let mount = format!("C={}", fixture.0.display());
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args(["--headless", "--mount", &mount, "--exec", "SETUP"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        output.stdout,
        b"Active code page: 437\r\nActive code page: 850\r\n\x9b\r\nActive code page: 437\r\n"
    );
}

#[test]
fn break_command_shares_the_guest_ctrl_c_setting() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("FLAGS.COM"),
        [
            0xb8, 0, 0x33, 0xcd, 0x21, // Get the flag
            0x80, 0xfa, 1, 0x74, 5, 0xb8, 1, 0x4c, 0xcd, 0x21, 0x30, 0xd2, 0xb8, 1, 0x33, 0xcd,
            0x21, // Clear the flag
            0xb8, 0, 0x4c, 0xcd, 0x21,
        ],
    )
    .unwrap();
    fs::write(
        fixture.0.join("FLAGS.BAT"),
        b"@echo off\r\nbreak\r\nbreak on\r\nbreak\r\nFLAGS.COM\r\nif errorlevel 1 exit %errorlevel%\r\nbreak\r\n",
    )
    .unwrap();
    let mount = format!("C={}", fixture.0.display());
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args(["--headless", "--mount", &mount, "--exec", "FLAGS.BAT"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        output.stdout,
        b"BREAK is off\r\nBREAK is on\r\nBREAK is off\r\n"
    );
}

#[test]
fn ver_and_verify_use_the_guest_dos_version_and_verify_flag() {
    let fixture = Fixture::new();
    let mut probe = vec![
        0xb8, 0, 0x30, 0xcd, 0x21, // DOS version
        0x3d, 5, 0, 0x74, 5, 0xb8, 1, 0x4c, 0xcd, 0x21, 0xb4, 0x54, 0xcd,
        0x21, // read VERIFY flag set by the shell
        0x3c, 1, 0x74, 5, 0xb8, 2, 0x4c, 0xcd, 0x21, 0xba, 0, 2, 0x31, 0xc9, 0xb4, 0x3c, 0xcd,
        0x21, // create GUEST.DAT
        0x73, 5, 0xb8, 3, 0x4c, 0xcd, 0x21, 0x89, 0xc3, 0xba, 10, 2, 0xb9, 1, 0, 0xb4, 0x40, 0xcd,
        0x21, // verified write
        0x73, 5, 0xb8, 4, 0x4c, 0xcd, 0x21, 0xb8, 0, 0x2e, 0xcd,
        0x21, // switch off from guest
        0xb8, 0, 0x4c, 0xcd, 0x21,
    ];
    probe.resize(0x100, 0);
    probe.extend_from_slice(b"GUEST.DAT\0x");
    fs::write(fixture.0.join("VERIFY.COM"), probe).unwrap();
    fs::write(
        fixture.0.join("SETUP.BAT"),
        b"@echo off\r\nver\r\nverify\r\nverify on\r\nverify\r\ncopy SOURCE.DAT NEW.DAT\r\ncopy /y SOURCE.DAT OLD.DAT\r\nVERIFY.COM\r\nif errorlevel 1 exit %errorlevel%\r\nverify\r\n",
    )
    .unwrap();
    let contents = vec![0x42; 8193];
    fs::write(fixture.0.join("SOURCE.DAT"), &contents).unwrap();
    fs::write(fixture.0.join("OLD.DAT"), b"old").unwrap();

    let mount = format!("C={}", fixture.0.display());
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args(["--headless", "--mount-rw", &mount, "--exec", "SETUP"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains("Dosa DOS version 5.00"), "{output}");
    assert_eq!(
        output
            .lines()
            .filter(|line| line.starts_with("VERIFY is "))
            .collect::<Vec<_>>(),
        ["VERIFY is off", "VERIFY is on", "VERIFY is off"]
    );
    for name in ["NEW.DAT", "OLD.DAT"] {
        assert_eq!(fs::read(fixture.0.join(name)).unwrap(), contents);
    }
    assert_eq!(fs::read(fixture.0.join("GUEST.DAT")).unwrap(), b"x");
}

#[test]
fn dos_file_creates_and_writes_use_the_adjusted_clock() {
    let fixture = Fixture::new();
    let mut probe = vec![
        0xba, 0, 2, 0x31, 0xc9, 0xb4, 0x3c, 0xcd, 0x21, // create EMPTY.DAT
        0x73, 5, 0xb8, 1, 0x4c, 0xcd, 0x21, 0x89, 0xc3, // keep file handle
        0xb4, 0x3e, 0xcd, 0x21, // close
        0xba, 10, 2, 0x31, 0xc9, 0xb4, 0x3c, 0xcd, 0x21, // truncate WRITE.DAT
        0x73, 5, 0xb8, 2, 0x4c, 0xcd, 0x21, 0x89, 0xc3, // keep file handle
        0xb9, 44, 16, 0x31, 0xd2, 0xb4, 0x2d, 0xcd, 0x21, // set time to 16:44
        0xba, 20, 2, 0xb9, 1, 0, 0xb4, 0x40, 0xcd, 0x21, // write one byte
        0x73, 5, 0xb8, 3, 0x4c, 0xcd, 0x21, 0xb8, 0, 0x57, 0xcd,
        0x21, // DOS handle modification time
        0x73, 5, 0xb8, 4, 0x4c, 0xcd, 0x21, 0x81, 0xfa, 0x5d, 0x28, 0x74, 5, 0xb8, 5, 0x4c, 0xcd,
        0x21, // February 29, 2000
        0x81, 0xe1, 0xe0, 0xff, 0x81, 0xf9, 0x80, 0x85, 0x74, 5, // 16:44
        0xb8, 6, 0x4c, 0xcd, 0x21, 0xb8, 0, 0x4c, 0xcd, 0x21,
    ];
    probe.resize(0x100, 0);
    probe.extend_from_slice(b"EMPTY.DAT\0WRITE.DAT\0x");
    fs::write(fixture.0.join("STAMP.COM"), probe).unwrap();
    fs::write(fixture.0.join("WRITE.DAT"), b"old contents").unwrap();
    fs::write(
        fixture.0.join("SETUP.BAT"),
        b"@echo off\r\ndate 02-29-2000\r\ntime 13:36\r\nstamp\r\nif errorlevel 1 exit %errorlevel%\r\ndir *.dat /od\r\n",
    )
    .unwrap();

    let mount = format!("C={}", fixture.0.display());
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args(["--headless", "--mount-rw", &mount, "--exec", "SETUP"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let listing = String::from_utf8(output.stdout).unwrap();
    for (name, time) in [("EMPTY.DAT", "1:36p"), ("WRITE.DAT", "4:44p")] {
        let row = listing
            .lines()
            .find(|line| line.contains(name))
            .unwrap_or_else(|| panic!("{listing}"));
        assert!(row.contains("02-29-00") && row.contains(time), "{row}");
    }
    assert_eq!(fs::read(fixture.0.join("EMPTY.DAT")).unwrap(), b"");
    assert_eq!(fs::read(fixture.0.join("WRITE.DAT")).unwrap(), b"x");
}

#[test]
fn dir_p_pages_at_the_selected_text_height_and_can_be_cancelled() {
    use std::process::Stdio;

    let fixture = Fixture::new();
    for index in 0..52 {
        fs::write(fixture.0.join(format!("F{index:02}.TXT")), b"x").unwrap();
    }
    fs::write(
        fixture.0.join("PAGES.BAT"),
        b"@echo off\r\nset DIRCMD=/P\r\ndir /b f*.txt\r\necho after\r\n",
    )
    .unwrap();
    let mount = format!("C={}", fixture.0.display());
    let run = |rows: &str, command: &str, answers: &[u8]| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([
                "--headless",
                "--mount",
                &mount,
                "--text-rows",
                rows,
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
    let prompt = "Press any key to continue . . .";
    for (rows, first_last_file, responses) in
        [("25", "F23.TXT", &b"xy"[..]), ("50", "F48.TXT", &b"x"[..])]
    {
        let output = run(rows, "DIR /B/P F*.TXT", responses);
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(stdout.matches(prompt).count(), responses.len());
        assert!(
            stdout
                .split(prompt)
                .next()
                .unwrap()
                .ends_with(&format!("{first_last_file}\r\n"))
        );
        assert!(stdout.ends_with("F51.TXT\r\n"), "{stdout}");
    }
    let cancelled = run("25", "PAGES", b"\x1b");
    assert!(cancelled.status.success(), "{cancelled:?}");
    let text = String::from_utf8(cancelled.stdout).unwrap();
    assert!(text.ends_with(&format!("{prompt}\r\nafter\r\n")));
    assert!(!text.contains("F24.TXT"));
    let eof = run("25", "DIR /B/P F*.TXT", b"");
    assert!(!eof.status.success());
    assert!(String::from_utf8_lossy(&eof.stderr).contains("DIR /P needs a key"));
}

#[test]
fn dir_searches_all_subdirectories_and_applies_sorting_and_dircmd() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.0.join("DATA/NESTED")).unwrap();
    fs::create_dir(fixture.0.join("ZDIR")).unwrap();
    for (path, data) in [
        ("DATA/NESTED/Match.txt", &b"nested"[..]),
        ("DATA/A.BIN", &b"bin"[..]),
        ("B.TXT", &b"b"[..]),
        ("C.TXT", &b"more"[..]),
        ("D.BIN", &b"medium"[..]),
        ("ZDIR/A.TXT", &b"z"[..]),
    ] {
        fs::write(fixture.0.join(path), data).unwrap();
    }
    fs::write(
        fixture.0.join("DEFAULTS.BAT"),
        b"@echo off\r\nset DIRCMD=/B/L/S/O:-N C:\\DATA\r\ndir\r\ndir /-s/-l/-o C:\\ZDIR\r\n",
    )
    .unwrap();
    let mount = format!("C={}", fixture.0.display());
    let run = |command| {
        Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args(["--headless", "--mount", &mount, "--exec", command])
            .output()
            .unwrap()
    };
    for (command, expected) in [
        (
            "DIR *.TXT /S/B/O:-S",
            &b"C:\\C.TXT\r\nC:\\B.TXT\r\nC:\\DATA\\NESTED\\MATCH.TXT\r\nC:\\ZDIR\\A.TXT\r\n"[..],
        ),
        (
            "DIR MATCH.TXT /S/B/L",
            &b"C:\\data\\nested\\match.txt\r\n"[..],
        ),
        (
            "DIR /B/O:GE-N",
            &b"ZDIR\r\nDATA\r\nDEFAULTS.BAT\r\nD.BIN\r\nC.TXT\r\nB.TXT\r\n"[..],
        ),
        (
            "DEFAULTS",
            &b"C:\\data\\nested\r\nC:\\data\\a.bin\r\nC:\\data\\nested\\match.txt\r\nA.TXT\r\n"[..],
        ),
    ] {
        let result = run(command);
        assert!(result.status.success(), "{command}: {result:?}");
        assert_eq!(result.stdout, expected, "{command}");
    }
    let missing = run("DIR MISSING.TXT /S/B");
    assert!(!missing.status.success(), "{missing:?}");
    assert!(String::from_utf8_lossy(&missing.stderr).contains("File not found"));
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
fn writable_zip_snapshot_restores_an_open_file_without_changing_the_archive() {
    use dosa_dos::{Machine, RunState};
    use dosa_mount::{DosTimestamp, Mount};

    let fixture = Fixture::new();
    let zip = fixture.0.join("checkpoint.zip");
    let mut program = vec![
        0xba, 0x40, 1, 0xb8, 2, 0x3d, 0xcd, 0x21, // Open the file read/write.
        0x89, 0xc3, 0xba, 0x50, 1, 0xb9, 2, 0, 0xb4, 0x3f, 0xcd, 0x21, // Read two bytes.
        0x30, 0xe4, 0xcd, 0x16, // Wait for a key.
        0xba, 0x52, 1, 0xb9, 1, 0, 0xb4, 0x40, 0xcd, 0x21, // Write at the saved offset.
        0x72, 2, 0x30, 0xc0, 0xb4, 0x4c, 0xcd, 0x21, // Return the write status.
    ];
    program.resize(0x40, 0);
    program.extend(b"SUB\\STATE.DAT\0");
    program.resize(0x52, 0);
    program.push(b'Z');

    let mut archive = zip::ZipWriter::new(fs::File::create(&zip).unwrap());
    let archive_time = zip::DateTime::from_date_and_time(1995, 3, 26, 14, 30, 0).unwrap();
    let options = zip::write::SimpleFileOptions::default().last_modified_time(archive_time);
    for (name, data) in [("EDIT.COM", program.as_slice()), ("SUB/STATE.DAT", b"ABCD")] {
        archive.start_file(name, options).unwrap();
        archive.write_all(data).unwrap();
    }
    archive.finish().unwrap();
    let original_zip = fs::read(&zip).unwrap();
    let original_mount = Mount::open(&zip).unwrap();
    let timestamp = original_mount.metadata("SUB/STATE.DAT").unwrap().modified;
    assert_ne!(timestamp, Some(DosTimestamp::EPOCH));
    assert!(timestamp.is_some());
    let new_machine = || {
        let mut machine = Machine::new();
        machine
            .mount('C', Mount::open_session_zip(&zip).unwrap())
            .unwrap();
        assert_eq!(
            machine
                .drive('C')
                .unwrap()
                .metadata("SUB/STATE.DAT")
                .unwrap()
                .modified,
            timestamp
        );
        machine
    };
    let mut original = new_machine();
    original.launch('C', "EDIT.COM", "").unwrap();
    assert_eq!(original.run(200).unwrap(), RunState::WaitingForKey);
    let state = original.save_guest().unwrap();

    let mut restored = new_machine();
    restored.load_guest(&state).unwrap();
    let read_live = |machine: &Machine| machine.drive('C').unwrap().read("SUB/STATE.DAT").unwrap();
    assert_eq!(read_live(&restored), b"ABCD");
    restored.key(b' ', 0x39);
    assert_eq!(restored.run(200).unwrap(), RunState::Exited(0));
    assert_eq!(read_live(&restored), b"ABZD");
    assert!(matches!(
        restored.load_guest(&state),
        Err(dosa_dos::GuestSnapshotError::MediaChanged)
    ));
    assert_eq!(read_live(&restored), b"ABZD");
    assert_eq!(read_live(&original), b"ABCD");
    assert_eq!(fs::read(&zip).unwrap(), original_zip);
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
            "@echo off\necho hello & second\n",
            "case",
            "batch command chaining",
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
