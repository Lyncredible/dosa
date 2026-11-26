# Dosa verification map

Run [Launch and Doctor](../SKILL.md) from the repository root. Each recipe uses the run's `$dosa`, `$scratch`, `$media`, and `$proof` variables. Use a fresh scratch mount for each independent scenario, and keep evidence outside the mounted media.

Record which entry points you exercise. For changes that span CLI, profile, DOS prompt, and desktop controls, drive each affected path. Record an unavailable path with its attempted action and missing prerequisite. Cargo integration tests can cover several entry points at once. Check their assertions to see which ones.

## Features

- [DOS shell and batches](shell.md) covers commands, batch arguments, and redirection.
- [Program launch and profiles](launch.md) covers COM execution, lookup, and exit status.
- [Mounted drives](mounts.md) covers access modes, ZIP sessions, FAT, and ISO media.
- [Boot a disk image](boot.md) covers second-stage loading and invalid boot images.
- [Desktop display and controls](desktop.md) covers live rendering, input, and the native window.
