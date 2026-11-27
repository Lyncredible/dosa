# 13 · More DOS text rows and columns

Host console presets now include 80 and 132 columns with 25, 43, or 50 rows. The View menu, launch profiles and CLI select the geometry. Tests verify BIOS stride, wrapping, scrolling, cursor bounds, framebuffer dimensions and independent row/column overrides.

Support 80- and 132-column text with 25, 43, and 50 rows through the shared DOS video adapter. Size text cells, cursor positioning, scroll bounds, BIOS Data Area fields, video memory reads and writes, and frontend rendering from the selected mode.

- Exercise console output, cursor movement, wrapping, and scrolling at rows 43 and 50.
- Verify guest-visible geometry and the last row of B800 text memory, for both the shell and a real-mode program.
- Verify intended text geometry after leaving CGA or VGA graphics and returning to the shell.
- On Windows, run `cargo test --locked -p dosa-desktop presents_wide_text_across_native_window_resizes -- --ignored` with a working GPU backend. It presents a generated 132×50 text grid in a native window at 800×480, 1360×660, and 640×420. It checks fit, whole-pixel scaling, the small-window fallback, and pointer bounds from the published viewport. The desktop application test separately verifies that the DOS text preset produces the same framebuffer geometry.

BIOS INT 10h supports loading built-in CP437 patterns and user-defined glyphs into VGA text font banks. The renderer uses attribute bit 3 to select a glyph when two banks are active. Guest saves and normal program launches preserve the loaded banks. The built-in 8x14 and 8x16 patterns use the bundled 8x8 glyphs scaled to the requested height. Direct CPU font RAM access and ROM pointer queries remain open. [IBM VGA Hardware Reference](https://bitsavers.trailing-edge.com/pdf/ibm/pc/ps2/42G2193_PS2_Hardware_Interface_Technical_Reference_Video_Subsystems_Sep92.pdf)

DOSBox-X offers both 80- and 132-column text modes with several row counts through its menu and shell. [DOSBox-X TrueType and text geometry guide](https://dosbox-x.com/wiki/Guide%3AUsing-TrueType-font-output-in-DOSBox%E2%80%90X)
