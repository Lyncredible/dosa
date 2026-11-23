# 18 · VGA 640×480 sixteen-color graphics

Extend the existing VGA aperture to mode 12h. Decode each pixel from the corresponding bit in all four planes, retaining plane masks, latches and write modes. Verify BIOS pixel read/write and XOR, page addressing, palette programming, and transitions between graphics and shell text.

Initial implementation adds scanout and BIOS pixel access. Attribute-controller palette programming and extended graphics BIOS text remain verification work.
