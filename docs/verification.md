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
