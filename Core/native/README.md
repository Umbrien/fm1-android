# Pinned source-native engines

These GPL adapters and state-isolation tools compile eleven exact public firmware
source versions. No proprietary FM1 SDK archive is linked. Source checkouts,
firmware resources, packages and build receipts stay in ignored local paths.

See the root README and docs/build.md for the full preparation/build commands.
Each *.lock.json retains public source commits, upstream resource hashes,
matching package metadata and original notice hashes. packages.lock.json adds
all eleven package cache paths and exact download checks for the Android build.
Pin/resource/compiler mismatches stop the build without a fallback.

The wrappers derive from the Virtual FM-1 commit pinned in upstream.lock.json.
Mutable firmware state is isolated per instance before optimization. The owning
host worker serializes every call; the audio output thread consumes only PCM.
SLOOP's raw store is 1 MiB; FM1 Open portable preview owns a separate 536576-byte
logical store. Preview data never seeds another engine's flash.

Original source/component notices are retained under licenses/ and the root
licenses/ directory. Existing Apple target support belongs to the shared native
build tooling; this repository contains no iOS frontend or Xcode project.

Clipper 1.1-experimental shares the SLOOP hardware host, with separate source,
symbol prefix, state and flash. Its SLOOP-only typed file APIs are excluded.
Stock V15 is an exact catalogue entry executed by the PI32v2 runtime with Android ARM64 JIT;
it is distinct from the native FM1 Open V13 portable preview.

FM-1+VA Version 102 (kind 12) also executes its exact original package in the
binary runtime. Package pins live in packages.lock.json; it has independent 1 MiB
flash and is not admitted through the source-native registry.

ChoralRoot 0.26.1 (kind 14) and GHOULBOX 2.3.2 (kind 15) compile their real
firmware DSP, UI and NOR storage to native ARM64 through the pinned source-host
HALs. They use the shared per-instance state isolation pass, separate symbol
prefixes and independent 1 MiB flash. Kind 13 remains reserved for imported
firmware. Source, generated resource and package pins are in additions.lock.json;
additions_prepare.py and additions_build.py prepare and build them. ChoralRoot's
source-host second-part job executes inline on the owning serial worker.
Original licensing notices and component texts are retained in licenses/.

JIANT 0.9.9 (kind 16) uses the same additions pipeline and its upstream source-host
bridge. The pin matches the public stable installer, commit 6968b20. Its splash
lasts 1730 ms; input tests wait two seconds before playing notes. Generated resources,
source files and the exact selector package are hash checked. No JIT is used.
