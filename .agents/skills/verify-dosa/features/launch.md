# Program launch and profiles

Launch a DOS program from mounted media, resolve the program name without an extension, and observe its output or exit status.

## Sub-features

- `launch-mount` runs a program from a file, directory, or ZIP.
- `launch-profile` launches with saved settings and explicit CLI overrides.
- `launch-status` returns the DOS exit code to the host in headless mode.

## How to get to it (user POV)

- Use `--mount` with `--exec`, or set `exec` and mounts in a TOML profile and use `--profile`.
- At the DOS prompt, type the program name. The shell searches `.COM`, `.EXE`, then `.BAT` when no extension is supplied.
- In the desktop menu, select **Drives → Quick launch** and choose a COM, EXE, or BAT file to mount its containing folder temporarily.

## Driving it with PowerShell and Cargo

Generate a COM that invokes DOS termination with status 7, then run it through the built binary.

```powershell
[IO.File]::WriteAllBytes((Join-Path $media 'EXIT7.COM'), [byte[]](0xB8, 0x07, 0x4C, 0xCD, 0x21))
& $dosa --mount "C=$media" --headless --exec 'EXIT7'
$guestExit = $LASTEXITCODE
if ($guestExit -ne 7) { throw "Expected DOS exit 7, saw $guestExit" }
```

- **Mount sources.** Run `cargo test --locked --test headless runs_a_dos_program_from_file_directory_and_zip_mounts` to verify guest execution and output from all three CLI source types.
- **Profiles.** Run `cargo test --locked --test headless launch_profile_applies_guest_settings_and_explicit_cli_overrides` to verify saved settings and overrides using generated media.
- **Desktop lookup and Quick launch.** Put an ASCII `SHOW.BAT` containing `@TYPE NOTE.TXT` in a fixture directory. Write `QUICK-LAUNCH-PROBE` to its sibling `NOTE.TXT`. Launch Dosa with a separate empty folder mounted as `C:`. Use **Drives → Quick launch** to select `SHOW.BAT`. Check that the guest shows `QUICK-LAUNCH-PROBE` and returns to the original `C:\>` prompt. Then mount the fixture at `D:`, switch to `D:`, and type `SHOW` at the prompt to check extensionless lookup.

## Gotchas

- A nonzero DOS exit code can be the expected result. Capture it immediately and check stderr for emulator errors.
- Profile paths resolve relative to the profile. CLI paths resolve relative to the host working directory.
- Quick launch uses an unused DOS letter and a read-only mount. Use a read-only batch such as `SHOW.BAT` for this path.
