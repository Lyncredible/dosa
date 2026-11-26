# Desktop display and controls

Run a DOS guest in the live desktop window, follow its visual response to input, and check window and guest controls.

## Sub-features

- `desktop-gpu` mounts a ZIP and displays a guest's VGA frames.
- `desktop-input` sends Enter and Escape and observes distinct guest states.
- `desktop-controls` uses the actual desktop menu and keyboard.

## How to get to it (user POV)

- Launch Dosa with a mounted source, or select a ZIP under **Drives**, type the program name at the DOS prompt, and press Enter.
- On Windows, use **Dosa ▾** or right-click the title bar to open the emulator menu. At the DOS prompt, F10 opens the menu. In fullscreen or on other platforms, use the menu button in the upper left.

## Driving it with Cargo and PowerShell

Use an interactive session with a working GPU backend. Run the block in a disposable shell without other `DOSA_SMOKE_*` overrides. Start Launch and Doctor in that shell so `$proof` is available.

```powershell
$env:DOSA_SMOKE_GENERATED = '1'
try {
    cargo test --test desktop_smoke --locked > "$proof/desktop.stdout.txt" 2> "$proof/desktop.stderr.txt"
    $smokeExit = $LASTEXITCODE
    "DOSA_SMOKE_GENERATED=1 cargo test --test desktop_smoke --locked`nexit=$smokeExit" | Set-Content "$proof/desktop.command.txt"
    if ($smokeExit -ne 0 -or -not (Select-String -Path "$proof/desktop.stdout.txt" -SimpleMatch 'desktop smoke passed: archive mounted, graphics rendered, 2 input transitions' -Quiet)) {
        throw "Desktop smoke did not complete. See $proof"
    }
} finally {
    Remove-Item Env:DOSA_SMOKE_GENERATED
}
```

- **ZIP copy and scaling.** For the affected feature, rerun the block with `DOSA_SMOKE_SESSION=1` or `DOSA_SMOKE_INTEGER_SCALING=1` in that shell. Retain each run's logs under a distinct name and record its flags.
- **Native controls.** Launch `& $dosa --mount "C=$media"`. At `C:\>` press F10, open **View**, and select the 80-column, 43-row preset. Close the menu and check that the viewport displays the taller text grid. Open the menu again to confirm the selected preset. For a reset regression, run a guest, select **Reset guest**, and verify return to the prompt and `DIR` visibility on the original mounted drive. Capture the target window before and after each action.

## Gotchas

- An unset `DOSA_SMOKE_GENERATED` or a conflicting `DOSA_SMOKE_ARCHIVE` can run a different scenario. Require the completion line as well as successful exit.
- The smoke harness feeds the application's menu actions and input events directly. Its live GPU loop checks guest frame changes and the generated run verifies WAV bytes. Exercise the built window for native picker, OS keyboard routing, title bar, or clipboard behavior.
- The test window closes itself after the checks. Its temp archive and WAV are removed by the harness, so retain the captured logs for its built-in assertions.
