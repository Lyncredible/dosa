# 13 · More DOS text rows and columns

Host console presets now include 80 and 132 columns with 25, 43, or 50 rows. The View menu, launch profiles and CLI select the geometry. Tests verify BIOS stride, wrapping, scrolling, cursor bounds, framebuffer dimensions and independent row/column overrides.

Support 80- and 132-column text with 25, 43, and 50 rows through the shared DOS video adapter. Size text cells, cursor positioning, scroll bounds, BIOS Data Area fields, video memory reads and writes, and frontend rendering from the selected mode.

- Exercise console output, cursor movement, wrapping, and scrolling at rows 43 and 50.
- Verify guest-visible geometry and the last row of B800 text memory, for both the shell and a real-mode program.
- Verify intended text geometry after leaving CGA or VGA graphics and returning to the shell.
- Check the higher-density text modes in a live GPU window at several window sizes.

DOSBox-X offers both 80- and 132-column text modes with several row counts through its menu and shell. [DOSBox-X TrueType and text geometry guide](https://dosbox-x.com/wiki/Guide%3AUsing-TrueType-font-output-in-DOSBox%E2%80%90X)
