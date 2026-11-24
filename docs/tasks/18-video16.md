# 18 · VGA 640×480 sixteen-color graphics

Extend the existing VGA aperture to mode 12h. Decode each pixel from the corresponding bit in all four planes, retaining plane masks, latches and write modes. Verify BIOS pixel read/write and XOR, page addressing, palette programming, and transitions between graphics and shell text.

Scanout and BIOS pixel access are implemented. Attribute-controller palette programming includes its index/data latch, status-port reset, color selection and BIOS palette writes, with register and pixel regressions.

Graphics text supports BIOS teletype, repeated characters, and attributed strings in VGA 12h/13h. CP437 glyphs, wrapping, scrolling, cursor preservation, and tiny guest-programmed CRTC heights have regression coverage. Guest-provided fonts and CGA graphics text remain compatibility work.
