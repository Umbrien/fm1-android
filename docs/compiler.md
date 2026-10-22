# On-device firmware compiler

The Instruments dialog's **Open .fwsc…** action imports an FM-1 update package
through Android's document picker. The app copies the package into private
storage, validates it, boots a candidate and requires 8192 audio frames plus an
actual ARM64 execution counter before replacing the current instrument. Startup
has a 15-second deadline checked between bounded render calls. A failure leaves
the previous instance available and reports an error. Audio is suspended during
candidate startup; compilation and synthesis belong to the serial engine worker,
not the AudioTrack output thread.

Nine bundled instruments retain their source-native adapters. Stock V15,
FM-1+VA 102 and imported binaries use the existing PI32v2 machine runtime with
its ARM64 compiler enabled on Android. Imports always execute their supplied
binary, including when its hash matches a bundled source-native instrument.
This avoids silently substituting a different implementation for the user's file.

## Execution and storage

The compiler translates hot decoded instruction blocks (after 32 visits) into
ARM64 instructions in the app process. It retains the FM-1 memory map, peripheral
models, interrupts and dual-core scheduling. Cold, unsupported compiler operations
and timing-sensitive paths use the reference interpreter. This is JIT binary
recompilation, not a C/C++ source compiler or a completely emulation-free port.
No network, external toolchain or Android package installation is involved.

Each CPU reserves a bounded 64 MiB code cache. Android's actual page size controls
allocation. New pages are writable during construction and read/execute after
instruction-cache synchronization; they are never writable and executable at the
same time. Existing executable pages are not made writable while installing a
new block. Code, chain slots and helper descriptors are owned and released by the
CPU. RAM code guards invalidate changed instruction bytes. Rust helper panics
return through generated frames before unwinding to the existing error boundary.

The reference, ARM64 and optional static C execution paths use exact instruction
budgets. This fixes a prior block-boundary overshoot that could make input timing
and the two-core schedule differ between reference and compiled execution.

Packages are stored as `Firmware/<sha256>.fwsc`, with a separate display label.
Imported flash is `Instruments/import-<sha256>.flash`. A rename reuses the same
state; different bytes get independent state even with the same filename. Imports
never inherit a bundled instrument's save. Reopening rechecks the stored package
hash, restores its flash and recompiles. Code caches are not persisted. Reset
applies only to the selected content identity. Switching checkpoints the current
instance before loading a candidate, including reselecting the same package.

## Supported boundary

Files must be 960 bytes to 4 MiB, unpack into a valid app area, identify FM-1 and
use the runtime's 1 MiB flash. SMK-37 packages and damaged headers/app ranges are
rejected. Structural validity and a successful boot do not establish compatibility
with every feature of a firmware: new CPU instructions or unmodeled peripherals
can still stop execution later. The compiler retains the pinned vendored ISA/SoC;
it does not automatically adopt upstream decoder or hardware-model updates.

If no generated ARM64 code executes during imported startup, import fails with an
explicit compilation-unavailable error. Known bundled binary engines retain the
reference fallback when executable memory cannot be allocated. The diagnostic
backend is 1 before compiled execution, 2 for source-native engines and 3 after
actual ARM64 execution; `instrument_jit_instructions` counts guest instructions
retired through generated blocks, including their helper calls.

## Verification — 2026-10-22

- Host: 46 CPU/compiler tests and 11 SoC/parser tests passed, including malformed
  app offsets, truncation, cache reset/ownership, self-modifying RAM and helper
  panic containment.
- Apple Silicon differential run: Stock V15, FM-1+VA 102, Felucca 1.6.1 and X0X
  1.1.0 matched the interpreter exactly for 300 render calls per firmware, with
  note input after call 200. PCM, frame counts, final LCD, LEDs and flash matched.
- Android API 37 arm64 emulator, 4096-byte pages: all 40 core tests passed. This
  includes `/proc/self/maps` verification that published compiler pages are RX,
  not RWX, and execution after cache reuse.
- Signed APK built successfully. All 16 Android instrumentation cases passed:
  nine source-native engines, both compiled binary catalogue entries, four
  compiled/reference comparisons, import/storage/restart/reset/failure recovery,
  SLOOP file/audio operations, UI, touch and lifecycle checks.
- Manual UI check: Instruments → Open .fwsc → Android Downloads → User-X0X.fwsc
  returned to the app with the imported filename and a rendered X0X display. The
  screenshot and UI hierarchy are retained alongside the local receipts.

The Android comparison produced the following times, including cold startup and
compilation, for 220 render calls with the same note input at call 180:

| Firmware | Frames | Compiled ms | Interpreter ms | Speedup |
| --- | ---: | ---: | ---: | ---: |
| Stock V15 | 108032 | 711.71 | 815.07 | 1.14× |
| FM-1+VA 102 | 108032 | 781.01 | 927.22 | 1.19× |
| Felucca 1.6.1 | 109568 | 1368.75 | 2048.88 | 1.50× |
| X0X 1.1.0 | 108032 | 1502.06 | 2352.13 | 1.57× |

These are emulator measurements on the build host, not physical-phone latency,
thermal, listening-quality or hardware parity results. They do not imply speed
matching source-native adapters. Runtime operation on a 16 KiB-page device has
not been measured. No store submission or release publication was performed.

Local detailed receipts are in ignored `Android/build/compiler-verification/`.
Build and focused test commands are in [build.md](build.md). Compiler provenance
and the inspected upstream Android port are recorded in [Vendor/UPSTREAM.md](../Vendor/UPSTREAM.md).
