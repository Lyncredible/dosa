# 18 · VGA 640×480 sixteen-color graphics

Extend the existing VGA aperture to mode 12h. Decode each pixel from the corresponding bit in all four planes, retaining plane masks, latches and write modes. Verify BIOS pixel read/write and XOR, page addressing, palette programming, and transitions between graphics and shell text.

Scanout and BIOS pixel access are implemented. Attribute-controller palette programming now includes its index/data latch, status-port reset, color selection and BIOS palette writes, with register and pixel regressions. Extended graphics BIOS text remains implementation and verification work.
