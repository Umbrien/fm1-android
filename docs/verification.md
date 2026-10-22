# Verification boundaries

The original Android port was exercised on the dedicated Pixel 5 / API 33 arm64
emulator. Eight exact compiled-native kinds booted, drew nonblank LCDs, produced
finite nonzero note PCM, and round-tripped complete edited storage. Preview
storage retained its distinct 536576-byte size.

SLOOP checks recorded a real synthetic pattern, captured typed objects, rejected
a bad workspace checksum, preserved global settings on project overlay, restored
samples, rendered audible loop/song WAVs, and checked a 3:2 duration relationship
for A1+B2 versus A1+B1 arrangements. Disposable export left the source unchanged.
Actual Activity tests injected repeated lower-edge taps, two fingers on one key,
multitouch glissando/release, background/foreground callbacks, explicit pause,
and backup restoration with an edited master setting after Activity recreation.

A warmed five-second emulator sample advanced the OS playback head by about
221206 frames at 44100 Hz, with zero additional application-ring shortages and
zero additional OS AudioTrack underruns. The configured system buffer was 1948
frames and the ring target 1024. Startup and transitions had separately recorded
shortages. Forcing the system buffer to 512 frames slowed OS consumption; the
implementation honors the device's minimum instead. These queue/counter values
do not measure end-to-end touch-to-sound latency.

Actual Android document-picker workflows saved/exported/imported a project,
canceled import review, selected/auditioned/imported a synthetic WAV sample and
saved a one-second WAV. That picker export used an empty loop and proved file
container/delivery only; the separate recorded-pattern tests proved audible
loop/song output. All six finishes were captured expanded/collapsed. Actual UI
instrument selection hid Files for Felucca and restored it for SLOOP.

An optional private synthetic exchange test imported three actual Swift/iOS
exports and applied projects through the settings-preserving overlay. Original
unchanged Swift structs decoded and restored an Android backup into a fresh
native engine, producing finite audible notes. These external test fixtures and
raw receipts are intentionally not published. Default clean-clone tests report
the exchange case as not requested unless explicit fixtures are supplied.

The standalone extraction is built independently from its own Cargo workspace
and Android sources. It contains no Swift frontend, Xcode project, original Git
history, personal data, signing credentials, firmware binary or compiled output.
Required vendored MIT Rust source and GPL adapters/notices remain attributed.

Physical Android latency, listening quality, long-session thermals, audio-focus
interruptions, unplugged outputs, exact hardware sound parity and human usability
remain unverified. A local build/test is not a store release or human acceptance.

## Kotlin host migration

The app and its instrumentation are now Kotlin under `org.fm1.android`; the
C/Rust engines are unchanged. Kotlin 2.0.21 with SDK 35 D8 passes packaging
without unsupported metadata warnings. The migrated app passed all eleven
emulator checks, including optional real Swift exchange, native engine routes,
project/sample/song behavior, real multitouch, audio output, lifecycle and
restored master settings after Activity recreation.

The first UI run found the emulator asleep, so injected events did not reach the
Activity. Waking the dedicated emulator and dismissing keyguard fixed the test
precondition; touch assertions and application behavior were not weakened. The
test script now establishes that precondition before injection. A warmed five-
second sample advanced the OS playback head by 221206 frames, with zero
additional OS underruns and application-ring shortages. Startup/transition
shortages remain separately recorded.

The new application ID installs alongside the previous app and leaves its
installation/data in place. Existing SLOOP project/backup files use the unchanged
format and can be exported from that app and imported here. No private data is
automatically read from another application.

The final Kotlin APK's twelve palette views were checked, including fixed knob
sizes and larger LCD after header collapse. Actual instrument selection hides
Files for Felucca and restores it for SLOOP. A project previously exported by the
Java app was selected through Android DocumentsUI, canceled at review, imported
and opened with a recovery backup. The WAV editor/audition and USR2 import also
completed. A one-second stereo WAV was saved through the system destination
picker; that imported loop was empty, so this specific provider file was silent.
The separate recorded-pattern instrumentation established audible loop/song
rendering. No physical listening acceptance is implied.

## Firmware update — 2026-10-21

SLOOP is now pinned to v2.8 (`b77bf55e3bf804665b8f5dffb5955bd8443311f0`).
Clipper v1.1-experimental (`e69067211c4690f34e494b5b896e4585163dcc2b`)
is a separate source-native engine (kind 10). Original M-VAVE Stock V15
(`FM-1_015`, kind 11) uses the exact vendor recovery package through the
PI32v2 interpreter, without JIT. FM1 Open V13 remains its separate preview.
All ten package size/SHA256 checks and source/resource preparation passed.

Validation on Apple Silicon macOS and the dedicated arm64 Android emulator:

- All 27 source-enabled host Rust tests passed, including Clipper in the
  shared sound/display/full-flash checks and the public ABI reopen checks.
- All 13 Android instrumentation cases passed: nine native engines, stock
  interpreter boot/note/LCD, SLOOP workspace/sample/loop/song operations,
  Activity multitouch/lifecycle/persistence, and reset/reload of all ten entries.
- SLOOP accepts the prior 2.7.1 workspace envelope identity. A disposable
  2.8 candidate validates its typed components before replacing active work.
  Instrumentation covers this envelope compatibility, preservation of typed
  components, and rejection of unknown firmware identities; external iOS
  exchange fixtures were not supplied for this run.
- Stock produced 148992 frames (about 3.38 seconds), with nonzero finite note
  PCM, in 1415 ms in the emulator check. This is a bounded synthetic render
  measurement, not physical listening or sustained output-latency evidence.
- Factory reset verification waits for the preceding Activity's worker to
  terminate, preventing its pending save from contaminating the isolated test.
- The signed APK contains ten exact packages; signature verification and
  16 KiB ZIP alignment checks passed. Output:
  `Android/build/FM1-android-arm64.apk` (13072899 bytes), SHA256
  `a0ac81c7a307fb22e86ab32daeb45fcca8f6565bfe6db7e365c3aa8d0816489f`.

Raw build receipts remain in ignored build/cache paths. Existing vendor Rust
and Android deprecated-API warnings remain. No physical Android device,
long-session thermal behavior, human listening acceptance or hardware parity
was qualified. Clipper retains its upstream experimental designation, and
stock is explicitly labelled emulated in the instrument picker.

## FM-1+VA addition — 2026-10-21

Added baud girl's exact FM-1+VA Version 102 package as kind 12, labelled
emulated. Its published installer hash matches the downloaded package; it runs
without JIT and owns a separate 1 MiB save. Optional preset packs are not bundled.

All 28 source-enabled host tests and all 14 Android instrumentation cases
passed with the new package present. Android tested its boot, LCD, finite
nonzero note PCM, and independent reset/reload alongside the ten earlier entries.
A panel-driven regression initializes VA and 8-Bit sounds using EDIT → Erase
Preset, checks distinct displays, and plays both modes. Their measured peaks
were 0.084320068359375 (VA) and 0.061737060546875 (8-Bit). The initial FM render
produced 148992 frames in 1542 ms in the emulator; this bounded synthetic
measurement does not establish physical listening quality or sustained latency.

The new signed APK contains eleven pinned packages, passes signature and
16 KiB ZIP alignment checks, and remains at
`Android/build/FM1-android-arm64.apk`: 13937296 bytes, SHA256
`ce757969b28062d1ff5a72381a74a8a67a1e6f67935208face23647057ae93a4`.
The preceding ten-package APK hash above is historical. Physical-device
validation and external iOS exchange fixtures remain outside this run.

## Material 3 system shell — 2026-10-21

The system shell now uses Material Components 1.13.0: buttons/icons, dialogs,
outlined text fields, exposed dropdowns, trim sliders, checkbox and progress
indicators. Material DayNight follows the system appearance; DynamicColors
applies supported system palettes. Synth `PanelView` rendering and native engine
sources are unchanged. Panel finish no longer controls shell colors.

The Android shell now builds with the checksum-pinned Gradle 8.13 wrapper,
AGP 8.11.1 and Kotlin 2.0.21. The existing native preparation, firmware hash
checks and local signing identity are retained. Dependency versions are locked
in `Android/app/gradle.lockfile`; Material/AndroidX notices are available in-app.

Validation on the dedicated arm64 API 37 emulator:

- All 15 default instrumentation cases passed in light mode (no external
  exchange fixtures), including the new Material theme/form/finish-independence
  checks and the existing engine/audio/project/touch/lifecycle/reset cases.
- All three UI-only cases passed in dark mode. The theme check also confirmed
  that Material's primary color matches Android's dynamic system primary.
- Reviewed the shell and dialogs in light/dark mode, including the project-name
  form and scrollable audio-export form. The final typography update was built,
  signed, installed and visually checked in those forms.
- APK signature and zip alignment checks passed. Light mode was restored after
  validation. This does not establish physical-device acceptance or an older-OS
  runtime fallback check.

Local build logs, receipts and screenshots are retained under ignored
`Android/build/material-ui/` and `Android/build/material-*.txt`.

## Skiff 1.4 update — 2026-10-21

Skiff now uses the FM-1 v1.4 release, commit
`c01fa1ed619266d770ea6552b153612e1b77d0a9`. The 610337-byte FM-1 package has
SHA256 `0622a752e8304777c042ccea0732ba023e1413baa6dbc8724576e2ec76de02e0`,
matching the upstream release asset digest. Both the package locks and the
runtime's exact size/hash recognition use this identity; the instrument remains
kind 7. The version label, compile-time version and bundled attribution are
updated. All eight generated header fingerprints are unchanged.

Validation on Apple Silicon and the dedicated arm64 API 37 emulator:

- All 88 Rust workspace tests passed, including all 28 shared-runtime tests with
  every native source enabled and the two interpreter packages supplied. Exact
  package recognition, public ABI reopen, full flash roundtrip and isolation
  checks passed. Existing vendored compiler warnings were retained.
- All 15 Android instrumentation checks passed without external exchange
  fixtures. Skiff was recognized as native kind 7 with the v1.4 package hash,
  produced nonzero audio/display output, and passed reset persistence.
- The standalone Skiff adapter smoke passed: complete 1 MiB flash roundtrip,
  project reopening, alternating-instance DSP/state isolation and all three
  user sample slots. The native state is 1650448 bytes.
- A separately compiled v1.3.2 adapter created a synthetic saved project and
  flash image. The v1.4 adapter reopened the same 8140-byte project identically
  and produced nonzero PCM after a note press. This is a bounded compatibility
  check using synthetic data, not a qualification of every existing project.
- Upstream FM-1 `bank_test`, `smpmix_test` and `theme_test` passed. These cover
  REST entries and the last-track song advance fix, sample levels/sends and
  persistence, and custom palette behavior.
- The Android APK rebuilt and passed signature/alignment checks; every one of
  its eleven firmware package assets matches its lock's size and SHA256.

Local evidence is under ignored `target/skiff-1.4-host/`,
`Research/NativeSkiff/upstream-skiff/build/skiff-1.4-checks/`, and
`Android/build/skiff-1.4-*`. The adapter retains its existing single-core FM-1
host mapping; this update does not add SMK-37 hardware, USB/editor or physical
second-core support, and does not establish hardware parity.

## On-device ARM64 compiler and firmware import — 2026-10-22

The earlier interpreter-only Android restriction is superseded by local `.fwsc`
import and ARM64 JIT execution. All 16 Android instrumentation cases and 40
Android core tests passed in a separate API 37 arm64 emulator. Four original
firmwares were compared against the reference interpreter; imports were checked
through restart, reset, failed replacement and native-engine switching.
See [compiler.md](compiler.md) for the exact validation boundary, measured speed,
current limitations and receipt location. Earlier entries above remain historical.
