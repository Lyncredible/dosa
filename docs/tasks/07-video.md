# 07 · CGA and planar VGA

Support DOS programs that use CGA mode 06h or switch from VGA mode 13h to unchained VGA memory. Keep color-index conversion and aspect correction usable through the existing application display.

- CGA mode 06h uses interleaved video memory at B800h. A synthetic MZ guest verifies drawing in both banks and restoring the text console. [PR #7](https://github.com/Lyncredible/dosa/pull/7)
- The CPU can route a physical memory window to an emulated device. Its tests cover A20 wrapping and string transfers across the device boundary. [PR #8](https://github.com/Lyncredible/dosa/pull/8)
- Planar VGA supports map masks, read latches, write modes, CRTC page start and pitch, and 320×200 and 320×240 scanout. Synthetic guests verify plane writes, latch copies, and scanout geometry. [PR #9](https://github.com/Lyncredible/dosa/pull/9)
