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
new block. Android stages small blocks into page batches before publishing any
entry point; batches flush at one page, eight queued blocks or eight subsequent
compiler visits. Failed publication leaves previously published pages intact.
Code, chain slots and helper descriptors are owned and released by the
CPU. RAM code guards invalidate changed instruction bytes. Rust helper panics
return through generated frames before unwinding to the existing error boundary.

The reference, ARM64 and optional static C execution paths use exact instruction
budgets. Special-register moves use a shared canonical ISA decoder before the
solver-derived field table, correcting register-bit permutations such as SP = R5
being decoded as SP = R3. Scalar extraction from a 64-bit register pair and
signed saturating add/subtract likewise use canonical fields rather than
register-specific solver samples. Unproven control bits fail explicitly. Public
decoding, cached instructions and native execution use the same fields. Exact
budgets fix a prior block-boundary overshoot that could make input timing
and the two-core schedule differ between reference and compiled execution.

The ARM64 backend has separate full-block and bounded-prefix paths, a tagged
indirect-branch cache, block-local register allocation and constant-address
lowering. Integer arithmetic preserves guest flags; floating multiply/accumulate
keeps the reference's separate rounding and nonfinite-value behavior. Floating-point
register allocation retains raw guest bits, including NaN payloads, and publishes
them before helpers and exits. Direct lowering covers common paired memory and
64-bit operations, with helpers for
unsupported cases. Carry arithmetic, condition-code branches, indirect calls,
test-and-set and ordinary/special-register stack lists have typed lowering.
Special-register transfers retain ordered fallback for SP-sensitive lists and
interrupt-readiness checks at the original instruction boundary. CPU1 batches compilation work while observing the same idle
probe PC and interrupt boundaries as the reference scheduler.

Interrupt-window checks consider enabled pending interrupts for the current CPU
and its stable priority/masking gates. They preserve an already due event and do
not use transient instruction-pair holds to suppress an interrupt window. CPU0
marks ISR entry immediately after interrupt admission. CPU1 reads TIMER4 against
its frozen clock without invoking CPU0's idle-time heuristic.

After the initial CPU1 probe, a large slice may attempt one bounded proof of a
repeating loop. At most 256 reference instructions and 128 written RAM words are
examined against the frozen CPU1 clock. CPU architecture, written memory and
peripheral state must return exactly to their starting values. Unsupported
effects, interrupts and the original idle signature reject the proof. Every trial
restores memory, CPU and decode-cache state. Proven repetitions count toward
guest retirement, separately from actual generated-code execution. This is a
generic runtime check; it does not recognize firmware names or hard-coded PCs.

Executable RAM and flash ranges are tracked across both CPUs. Writes to decoded
code invalidate execution; flash data writes and identical watched RAM stores do
not discard compiled code. Decoder lookahead and wrapped flash page programs are
included in the invalidation boundary. Compiled memory accesses retain range,
width, alignment and peripheral-side-effect checks. Restoring saved flash checks
the complete image length before mutation and invalidates only changed mapped
code. Unchanged encrypted package bytes do not overwrite the decoded XIP view.

Bulk stack transfers check their complete RAM span before using direct loads and
stores; watched code and other address ranges retain ordinary bus operations.
Synchronous FM-1 GPIO updates preserve the existing event deadline, avoiding a
hardware-service exit for every front-panel bit. Timer, DMA and interrupt
programming retain scheduling barriers. Low MMIO addresses use page-based region
classification, checked against the original routing order, while high memory
windows retain their existing priority.

CPU1 startup accepts both the clock-register release and the C1_CON reset/release
sequence with the entry mailbox at `0x01c7fff8`. Later pause/resume operations do
not recreate its state. The shared P33 serial bridge supports command/address/data
transactions, readback and write/or/and/xor in both register domains. These
behaviors follow the imported Melodee HAL (`fm1_sys.h`, `fm1_core1_stop.h`,
`fm1_multicore.h`). P33 transfers complete synchronously; analog supply physics
and watchdog/reset timing remain outside the model.

ALNK models each enabled receive/transmit channel on the shared buffer clock.
Receive channels supply zero input and raise their channel interrupt even when
the board has no analog input; FM1 Open uses that interrupt to run its DSP.
Transmit channel 3 supplies the FM-1 output. One event advances the shared half
buffer phase once, and ordinary bus writes preserve executable-RAM invalidation.
This follows the imported FM1 Open audio HAL; it does not simulate external I2S
devices.

Android's output thread consumes preallocated PCM. The engine worker wakes when
PCM is consumed; display snapshots and periodic flash snapshots yield to low PCM
reserve. Immutable save snapshots go to an ordered storage worker, with barriers
for explicit saves and instrument changes. Partial AudioTrack writes retain their
unwritten suffix. When PCM is briefly unavailable, the output thread waits only
within the time covered by already queued AudioTrack frames, retaining a 20 ms
cushion. The deadline is fixed per chunk; wrap/reset anomalies disable waiting
until a flush. This avoids committing silence before the actual output deadline.
These changes do not increase the 1024-frame PCM target or
change the firmware's sample rate or instruction clock.

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
Agreement with the reference runtime does not establish timing or electrical
parity with a physical FM-1. Imported binaries do not have the performance
guarantees of the bundled source-native adapters.
Predicated-block extent scanning is iterative and bounded to 256 nesting levels
and 65,536 decoded instructions per scan. Excessive or undecodable extents fail
explicitly instead of exhausting the Android thread's native stack.

If no generated ARM64 code executes during imported startup, import fails with an
explicit compilation-unavailable error. Known bundled binary engines retain the
reference fallback when executable memory cannot be allocated. The diagnostic
backend is 1 before compiled execution, 2 for source-native engines and 3 after
actual ARM64 execution; `instrument_jit_instructions` counts guest instructions
retired through generated blocks, including their helper calls.

## Physical-phone measurement procedure

Use the configured Java 21, Android SDK 35 and NDK toolchain from
[build.md](build.md). The ignored `.tools/env.sh` is a machine-local convenience
when present. Build the current native code and pinned package assets first:

```sh
source .tools/env.sh # only on a checkout with this machine-local file
./Android/build.sh --core
adb devices -l
./Android/benchmark-phone.sh 'EXACT_ADB_SERIAL'
# Longer sustained run after the short continuity check passes:
./Android/benchmark-phone.sh 'EXACT_ADB_SERIAL' --duration-seconds 300
```

Replace `EXACT_ADB_SERIAL` with the identity in the first column of `adb devices`,
not the model name. The helper requires that explicit authorized serial and
refuses an emulator or a device without ARM64 support. It builds with
`-Pfm1Performance assembleBenchmark assembleBenchmarkAndroidTest`, signs both
APKs with the local development key, and verifies their application IDs and the
instrumentation target before installing. It installs and runs only
`org.fm1.android.benchmark` and `org.fm1.android.benchmark.tests`; it never clears,
uninstalls, resets or installs over `org.fm1.android` or accesses its private saves.
Both installations use `--no-incremental`. Before instrumentation, the helper
wakes the screen and requests keyguard dismissal. If the phone remains securely
locked, unlock it manually and rerun; the helper does not enter a credential.

The helper reuses prepared inputs under `Android/build/`. To use another existing
prepared build, pass `--prepared-build /absolute/path/to/build`; it must contain
`lib/arm64-v8a/libfm1android.so`, all locked `assets/engines/*.fwsc` packages and
`assets/notices/`. The helper stages those generated inputs into `Android/build/`
and checks that APK payload hashes match. It fails clearly when inputs are
missing or package identities differ. It does not compile the Rust/native core.

The isolated launcher label is **FM-1 Benchmark**; the regular app retains **FM-1**.
`performanceOnly` selects FM-1+VA 102, unmutes output, fixes the app master to 716,
and warms for three seconds,
then measures six intervals of four held keys for four seconds followed by one
second released. `--duration-seconds` accepts 30–600 seconds in five-second
intervals; the instrumentation and helper both enforce the requested duration,
interval count and eight applied key edges per interval. The engine must run before and after measurement with a stable
output generation. Both instrumentation and the helper independently require
at least 30 seconds of measurement, generated/accepted-generated/played frame
progress of at least `elapsedMs * sampleRate / 1000 - 4096`, zero missing host
frames and zero additional AudioTrack underruns. Instrumentation also rejects a
runtime error. Played progress must also be no greater than
`elapsedMs * sampleRate / 1000 + 4096`, so a decreased/reset playback head cannot
masquerade as a huge unsigned wrap. At least 48 applied key edges must be counted.
The final key releases pass through a worker barrier with a 20-second timeout;
that wait is included in measurement time, so a starved owner cannot gain a pass.
An inactive engine writing silence cannot pass through zero
underruns alone; helper validation reasons and frame deltas are retained in
`receipt.json`. Existing private
benchmark saves remain between runs; their FM-1+VA flash and preferences hashes
are recorded before and after measurement. Only previous benchmark receipt files
are removed before the test, so stale JSON cannot be mistaken for a new result.

Each invocation creates an ignored
`Android/build/phone-verification/<UTC timestamp>-<unique suffix>/` directory.
It retains signed APKs and their hashes, manifest/signature checks, build/install
logs, raw instrumentation output, `device.json`, `instrumentation.json`,
`performance.json` when produced, and a final `receipt.json`. Device metadata
keeps the selected/reported adb serial and device serial separate from reported
model, board and build fingerprint. Source HEAD and dirty-diff hashes describe
the checkout; the prebuilt JNI SHA-256 identifies the executed native input and
does not prove that it was built from that HEAD.

The helper returns nonzero on a build/transport error, failed instrumentation
assertion, missing complete receipt or failed continuity gate. It retains JSON
and raw output on failure, including the `performance.json` written before a
continuity assertion. A failure before that file is produced is recorded as such.
These measurements cover this workload and output route; they do not establish
acoustic touch latency, thermal stability over a long session, or other firmware
compatibility. While using the app, Instruments → Audio diagnostics also exposes
host timings, accepted/generated frames, host/OS underruns and the current route;
Copy saves a snapshot to the clipboard. Render and producer timings include
cumulative wall and thread CPU time; their difference reveals descheduling.
Producer-wake time can overlap the current render and is not a separate CPU cost.
`pcmWait` records output-thread waits covered by queued audio.

### Compiler profiling and isolated throughput

`Core/examples/firmware_bench.rs` exercises a package in memory, without opening
app saves or playing audio. It warms for 200 render calls, then measures the
requested number of 512-frame calls. Arguments are the package path,
`jit|reference`, block count, `default|va|8bit`, chord period in blocks, held
blocks, and comma-separated key IDs. Defaults are 1000 blocks, default sound,
100/75 chord timing and keys `21,25,28,32`. X0X's default drum kit needs a drum
trigger such as key `15`; a silent keyboard workload does not measure its voice
cost. JSON reports frame counts, short calls, finite PCM peak/hash, latency
percentiles and throughput relative to 44.1 kHz. This measures compute capacity,
not Android output continuity; use the instrumentation gate for that.
The preceding stderr record lists the ten slowest render calls and their measured
code-generation/publication time. Compilation totals include failed attempts and
survive cache resets; ordinary compiled lookups do not read the timing clock.
These totals exclude decoding and profiling-output formatting.

Set `FM1_PROFILE=1` only for untimed investigations. The final stderr JSON includes
MMIO/scheduler counters and each CPU's `jit.hotNativeBlocks`. Generated code
counts entry through both direct and indirect native branches, which Rust entry
counters miss. Full and bounded entries count admission, not necessarily complete
block execution: MMIO or an interrupt may end an admitted prefix early. The list
contains only the hottest 64 blocks. Compare a boot-only capture with a longer
capture before treating a hot startup loop as a playback bottleneck. Normal runs
allocate no native-block counters and emit no counter instructions.

`Core/examples/firmware_bench_ffi.c` can load an APK's extracted native library to
check the exact JNI engine input independently of AudioTrack. Like the Rust
example, it uses volatile state and reports PCM and timing; it is not a substitute
for a foreground physical-phone playback test.

### Redmi Note 9 Pro development measurements

The connected Redmi Note 9 Pro runs Android 13 with 4096-byte pages. The following
30-second FM-1+VA 102 results are failed continuity checks, not release acceptance:

| Change measured | Missing host frames | Missing audio | Local receipt directory |
| --- | ---: | ---: | --- |
| Earlier compiler candidate | 493056 | 11.18 s | `20261022T120020Z-wvhr9ypy` |
| Typed instruction lowering and tighter runtime guards | 278528 | 6.32 s | `20261022T123641Z-jjz75m5g` |
| Output waits within already queued audio | 56832 | 1.29 s | `20261022T125051Z-92_lzjz9` |
| Expanded register cache and SoC dispatch changes | 31232 | 0.71 s | `20261022T131903Z-_kwyqm14` |
| Later development candidate | 20480 | 0.46 s | `20261022T150325Z-kbmwwqfj` |
| Final retained compiler and compatibility changes | 27136 | 0.62 s | `20261022T152557Z-3cvh3zip` |

All receipts are under ignored `Android/build/phone-verification/`. These are
successive development builds, not isolated attribution of each change. The last
run applied all 48 input edges and reported zero AudioTrack underruns, yet still
missed 27136 generated frames: output-thread silence can hide engine starvation
from Android's underrun counter. The continuity gate correctly rejected it.
The final run lasted 30.010 seconds; render work consumed 28.03 seconds of worker
CPU time. The final selection removed optimizations that failed controlled
comparisons, rather than selecting the lowest isolated continuity result.
No 300-second acceptance run was attempted because the short check failed.

## Final retained build verification — 2026-10-22

- Host: 99 CPU/compiler and 47 SoC tests passed.
- Redmi Note 9 Pro, Android 13, ARM64: 9 ISA, 93 CPU/compiler and 47 SoC tests
  passed, including native execution and executable-memory checks.
- All 11 locked firmware packages matched the reference for every call in a
  300-call scenario, including complete audible output after note input, both
  CPUs' architectural state and retirement, clock, LCD, LEDs and flash.
  FM-1+VA 102 also matched through polyphony and all three tested sound modes.
- Kotlin audio/persistence host checks, five Python benchmark-validator tests,
  and WebAssembly compilation checks for the CPU and SoC passed.
- The signed ARM64 APK built successfully. The final physical Android playback
  check failed as recorded above. This build does not establish uninterrupted
  real-time operation for arbitrary firmware.

Region merging, special self-loop dispatch, inline timer-read caching and
unchecked RAM-chain entries were measured and removed. A separate mathematical
poll-loop experiment was not integrated. Their development artifacts are ignored;
the final runtime contains no disabled versions of these experiments.

## Baseline verification — 2026-10-22, commit 8cac9e6

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
