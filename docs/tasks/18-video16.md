# 18 · VGA 640×480 sixteen-color graphics

Extend the existing VGA aperture to mode 12h. Decode each pixel from the corresponding bit in all four planes, retaining plane masks, latches and write modes. Verify BIOS pixel read/write and XOR, page addressing, palette programming, and transitions between graphics and shell text.

Scanout and BIOS pixel access are implemented. Attribute-controller palette programming includes its index/data latch, status-port reset, color selection and BIOS palette writes, with register and pixel regressions.

Graphics text supports BIOS teletype, repeated characters, and attributed strings in VGA 12h/13h. CP437 glyphs, wrapping, scrolling, cursor preservation, and tiny guest-programmed CRTC heights have regression coverage. Guest-provided fonts and CGA graphics text remain compatibility work.

VGA BIOS function `INT 10h AX=1003h` switches text attribute bit 7 between blink and bright backgrounds. A desktop regression covers both rendering modes and save-state restoration. [VGA BIOS reference](https://fd.lod.bz/rbil/interrup/video/101003.html)

VGA BIOS function `INT 10h AX=1130h` returns guest-readable CP437 font tables or the guest's current font vector, along with the active character height and last screen row. Regressions read the glyph bytes and verify text and graphics geometry. [Font query reference](https://fd.lod.bz/rbil/interrup/video/101130.html)
