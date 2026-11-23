use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn runs_a_dos_program_from_file_directory_and_zip_mounts() {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = Fixture(std::env::temp_dir().join(format!("dosa-{}-{suffix}", std::process::id())));
    fs::create_dir(&dir.0).unwrap();
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
