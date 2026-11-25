# Architecture

dosa runs DOS applications in a Rust emulator and presents the guest display in a native desktop window. The built-in DOS services start with the calls needed for small real-mode programs. The first compatibility milestone targets basic COM and MZ execution, text output, and VGA mode 13h. Later milestones track the CPU coverage, DOS services, and PC devices needed for broader application support.

| Crate | Owns |
| --- | --- |
| `dosa-cpu` | x86 execution, registers, segmentation, descriptor tables, and interrupt dispatch |
| `dosa-mount` | Host directory, single-file, and ZIP sources exposed through DOS drive paths |
| `dosa-dos` | Guest program loading, DOS and BIOS services, input queue, and guest display state |
| `dosa-desktop` | Native window and input, GPU presentation, and application lifecycle |
| `dosa` | Command-line arguments, shell, and composition of the DOS library and desktop frontend |

The DOS library composes the CPU and mount libraries. It advances the guest in bounded instruction batches so that keyboard input and window events can run even when a guest loops. CPU and DOS tests run headlessly with deterministic synthetic programs. Unsupported CPU instructions and services should report enough information to reproduce a failure, including the guest address for instruction errors.

The desktop owns platform APIs. It maps window events to guest input and presents guest frames using a GPU-backed surface. Preserve guest aspect ratio and crisp text at useful window sizes. The window can provide modern spacing, typography, and shortcuts independently of the guest display. Guest state and emulation timing belong to the DOS library so that another native frontend can reuse them.

Drive mounts are explicit. DOS paths are resolved within the selected mount, and host paths stay in the mount layer. Folder and ZIP listings use consistent DOS case matching. A ZIP is exposed as a virtual drive and files are decoded on demand. The first milestone treats mounted content as read-only.

Windows is the first runtime validation target. Keep native path handling in the mount layer and window, keyboard, and graphics APIs in the desktop layer. Linux and macOS can use the same CPU, DOS and mount libraries with their native graphics backends.
