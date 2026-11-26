# DOS shell and batches

Run DOS commands, pass arguments to a batch, and redirect the batch's output to a mounted drive.

## Sub-features

- `shell-command` runs built-in DOS commands from the prompt or startup.
- `shell-batch` expands batch arguments and reads back redirected output.

## How to get to it (user POV)

- At the desktop `C:\>` prompt, type `HELP MOUNT` or `RUN PROBE` and press Enter.
- From the host terminal, pass the same command as one argument after `--headless --exec`.

## Driving it with PowerShell

Mount a fresh writable fixture directory. Save the batch as ASCII with DOS line endings.

```powershell
$batch = "@ECHO OFF`r`nECHO DOSA-%1>RESULT.TXT`r`nTYPE RESULT.TXT`r`n"
[IO.File]::WriteAllText((Join-Path $media 'RUN.BAT'), $batch, [Text.Encoding]::ASCII)
Copy-Item (Join-Path $media 'RUN.BAT') "$proof/RUN.BAT"
& $dosa --mount-rw "C=$media" --headless --exec 'RUN PROBE' > "$proof/batch.stdout.txt" 2> "$proof/batch.stderr.txt"
$batchExit = $LASTEXITCODE
"$dosa --mount-rw `"C=$media`" --headless --exec 'RUN PROBE'`nexit=$batchExit" | Set-Content "$proof/batch.command.txt"
if ($batchExit -ne 0) { throw "Dosa batch failed. See $proof" }
Copy-Item (Join-Path $media 'RESULT.TXT') "$proof/RESULT.TXT"
if ((Get-Content "$proof/RESULT.TXT" -Raw).Trim() -cne 'DOSA-PROBE' -or
    -not (Select-String -Path "$proof/batch.stdout.txt" -Pattern '^DOSA-PROBE$' -Quiet)) {
    throw "Batch readback or persisted output differed. See $proof"
}
& $dosa --mount "C=$media" --headless --exec 'TYPE RESULT.TXT' > "$proof/readback.stdout.txt" 2> "$proof/readback.stderr.txt"
$readbackExit = $LASTEXITCODE
"$dosa --mount `"C=$media`" --headless --exec 'TYPE RESULT.TXT'`nexit=$readbackExit" | Set-Content "$proof/readback.command.txt"
if ($readbackExit -ne 0 -or -not (Select-String -Path "$proof/readback.stdout.txt" -Pattern '^DOSA-PROBE$' -Quiet)) {
    throw "Fresh Dosa process could not read the persisted file. See $proof"
}
```

For the desktop path, reset the fixture by removing `RESULT.TXT` from the disposable mount. Launch `& $dosa --mount-rw "C=$media"`, type `RUN PROBE`, and check for `DOSA-PROBE` and return to `C:\>`. Copy the resulting host file into evidence. For changes involving DOS input and cancellation, use `cargo test --locked --test headless batch_pause_reads_headless_input_and_confirms_cancellation` for the existing headless input cases.

## Gotchas

- Keep the whole DOS command quoted on the host so its `>` operator reaches the guest shell.
- Use a writable mount for DOS redirection. A plain `--mount` is useful for independent readback.
