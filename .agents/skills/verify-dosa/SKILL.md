---
name: verify-dosa
description: Verify Dosa through its desktop window and headless CLI. Use before opening a Dosa pull request or when checking guest launch, DOS shell commands, mounted media, disk boot, or desktop behavior.
---

# Verify Dosa

Use the [feature map](features/README.md) to choose the affected user paths. Dosa's primary surface is its GPU-rendered desktop with a DOS prompt. The headless CLI runs the same shell and guest core but does not exercise the window, host input, or native menus.

## Pull request baseline

For every PR, run Launch, Doctor, the headless batch recipe in [DOS shell and batches](features/shell.md), and Cleanup. The batch must write `RESULT.TXT`, show `DOSA-PROBE`, and let a fresh Dosa process read the saved file. Drive the additional mapped paths affected by the diff before cleanup. Use a fresh scratch mount for each independent recipe.

## Launch

From the repository root, use the default Cargo target directory and run the following in PowerShell. Keep the same shell for the recipe. On Linux or macOS with PowerShell, the binary has no `.exe` extension.

```powershell
cargo build --locked --bin dosa
if ($LASTEXITCODE -ne 0) { throw 'Dosa build failed' }
$binaryName = if ($env:OS -eq 'Windows_NT') { 'dosa.exe' } else { 'dosa' }
$dosa = (Resolve-Path "target/debug/$binaryName").Path
$runId = [guid]::NewGuid().ToString('N')
$proof = (New-Item -ItemType Directory -Path "target/verify-dosa/$runId/evidence" -Force).FullName
$scratch = (New-Item -ItemType Directory -Path "target/verify-dosa/$runId/scratch").FullName
$media = (New-Item -ItemType Directory -Path (Join-Path $scratch 'media')).FullName
```

Each headless command starts a fresh process. There is no service, auth, initialization, or persistent guest state. For a manual desktop run, use `& $dosa --mount "C=$media"`, wait for the `C:\>` prompt, and type `DIR` to confirm the drive is usable. Leave the foreground process running only while driving it. Keep generated or copied fixtures under this run's scratch directory. Parallel headless runs can use distinct fixtures. Give each native-input run exclusive use of its desktop session so keystrokes go to the intended window.

## Doctor

Use this read-only command against the binary just built. It proves the chosen executable reaches the DOS shell without any mounted media.

```powershell
& $dosa --headless --exec 'HELP MOUNT' > "$proof/doctor.stdout.txt" 2> "$proof/doctor.stderr.txt"
$doctorExit = $LASTEXITCODE
"$dosa --headless --exec 'HELP MOUNT'`nexit=$doctorExit" | Set-Content "$proof/doctor.command.txt"
if ($doctorExit -ne 0 -or -not (Select-String -Path "$proof/doctor.stdout.txt" -SimpleMatch '/SESSION' -Quiet)) {
    throw "Dosa shell readiness check failed. See $proof"
}
```

## Drive

Use `--headless --exec 'DOS command'` for a shell or guest action. Pass the entire DOS command as one argument, including any DOS redirection. Keep a separate host redirect for captured stdout and stderr. Use the existing `desktop_smoke` harness for the live GPU event loop. Use the built executable and target window for native picker, keyboard, or window behavior.

## Evidence

Keep evidence under `$proof`. For each mapped entry point driven, record the command or UI action, its result, and relevant preconditions. Capture stdout, stderr and the exit code for CLI runs. For native UI work, capture the window before and after the action and record the target window and observed behavior. Report the coverage of the desktop smoke by its actual assertions. Use host file bytes, directory listings, or before and after hashes to verify storage effects. For persistence across launches, reopen the data through a fresh guest or Dosa process as well. Exercise the production entry point for the behavior under review and use generated fixtures or an authorized copy of user media.

## Cleanup

Close any window opened for this run using its own close button. If it hangs, stop only the run's known process ID. Headless invocations and successful smoke runs exit on their own. Remove this run's scratch directory and confirm the evidence files still exist.

```powershell
$expectedScratch = Join-Path (Split-Path -Parent $proof) 'scratch'
$scratchPath = (Resolve-Path -LiteralPath $scratch -ErrorAction Stop).ProviderPath
if ($scratchPath -ne $expectedScratch -or $runId -notmatch '^[0-9a-f]{32}$') {
    throw 'Scratch path does not match this verification run'
}
Remove-Item -LiteralPath $scratchPath -Recurse -Force -ErrorAction Stop
Get-ChildItem -LiteralPath $proof
```

## Helpers

Use the repository's Cargo integration harnesses where the feature map names them. Each invocation and its expected observable result is in the corresponding recipe.
