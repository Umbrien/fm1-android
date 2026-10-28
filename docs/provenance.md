# Source and asset provenance

The Android Kotlin/Canvas host and JNI glue are project-owned code. Shared Rust
C-ABI adapters and GPL source-native tooling were extracted from the private
FM-1 iOS development tree without its Git history, Swift frontend, device data,
credentials, logs or firmware-derived generated sources.

The reference emulator crates retain their original MIT attribution and pinned
upstream revision in Vendor/UPSTREAM.md. Native wrapper/state-tool origin is
Virtual FM-1 commit c52668953231655046ee142d2b3ab4fd2c8ca0b4. Their original
copyright/license comments and notices are retained.

| Native source | Public repository | Exact commit |
| --- | --- | --- |
| SLOOP | https://github.com/isod89/sloop-fm1 | `5eb24c1dc18261a85c193e8b28318944690e58fd` |
| Clipper | https://github.com/isod89/clipper-fm1 | `a160376c5ef61d6836ff570df925d12b5a66fc63` |
| felucca | https://github.com/hugelton/Felucca | `d7b1b7c4099bba28854869ddb256b3d9f634cee8` |
| x0x | https://github.com/charlesvestal/fm1-x0x | `27f6fac7e07ce97b5de158b45aec99f59a9e23fd` |
| melodee | https://github.com/keremimo/melodee | `7d2dfa33da8bc8835248be5475e890624d53802e` |
| FM1 Open | https://github.com/m-vave/fm1_open | `4d266850f8a46bd5154a0d495c1f60317156c9f7` |
| Skiff | https://github.com/illuminus5/skiff | `4382e9cf5e5bfc8263ee3775c11c7f45373617a0` |
| fomni | https://github.com/charlesvestal/fm1-fomni | `6ecd227013cc18def0e89d035b3760eb268b342b` |
| jangada | https://github.com/zednaked/jangada | `46e6c2c2ed94b609918aa6f5f7381fac915ffc45` |

All upstream checkouts, resource-generator output and exact release packages
are prepared into ignored local storage. No external firmware source/assets,
factory sound banks, opaque SDK archive, executable or private save is committed.
The generated_isa.rs file in Vendor is the required MIT upstream Rust ISA source;
it is distinct from excluded firmware-derived generated code.

The FM1 Open adapter builds the public portable preview source, excluding board
hardware drivers and the proprietary sdk/lib/libfm1_engine.a. Its independent
logical store is 536576 bytes; the other catalogue entries own 1 MiB stores.

The Android icon at Android/app/src/main/res/drawable/icon.png is a 192×192 resize
of an existing generated original app icon. The original design study used the
manufacturer's product photograph as a visual reference:
https://www.m-vave.com/images/productInfo/fm-1-3.webp . That photograph is not
packaged as the icon. No generation prompt is included.

The README screenshot is the actual Android native panel in synthetic test
state on an emulator. It contains no owner project, personal saved data or
private recording. Engine LCD and UI graphics retain their upstream attribution.

M-VAVE Stock V15 uses the unmodified 699956-byte recovery package from
`m-vave/fm1_open` commit `4c7496daac04a95ff10d1afdcc8f9ead2dbabc41`,
`sdk/firmware/FM-1_015.fwsc`. SHA256 is
`db1642b2b6fa5c2cccb11ffd13878068bb28601678d3644049f99dc40e7edb8a`.
The publisher identifies the recovery resource as Apache-2.0; its complete
component notice is bundled in `licenses/NOTICE-stock-v15`. It executes through
the PI32v2 runtime with ARM64 JIT on Android, separately from all source-native engines.

FM-1+VA Version 102 is the original binary published by baud girl / Madeline
at https://baudgirl.com/work/FM-1+VA/install (2026-10-19). The 859700-byte
package SHA256 `7b169a69b30570581d0ebc7b44851e3e08898d2956ade155363bcf18fac5c348`
matches that installer’s published metadata. It executes as binary kind 12,
with separate 1 MiB flash. No FM-1+VA source or optional preset pack is vendored.
The inspected official pages do not specify firmware redistribution terms;
`licenses/NOTICE-fm1va` retains attribution without treating the host’s license
as a license grant for that firmware.

ChoralRoot 0.27 (Quixotic7 / ChoralRoot contributors, derived from Felucca)
and GHOULBOX 2.4 (Jason Persinger, derived from Felucca by Leo Kuroshita /
Hügelton Instruments) compile their actual DSP, UI and storage source for Android
ARM64 through their source-host HALs, as native kinds 14 and 15. They use the
shared per-instance state transformation and independent 1 MiB saves. Kind 13
remains reserved for imported packages. Source revisions, source/resource hashes
and generated-header hashes are pinned in Core/native/additions.lock.json:

| Source | Release | Exact commit |
| --- | --- | --- |
| ChoralRoot | 0.26.1 | `070bd306ed63c7d08f09bb6ea937262b52b1a6c0` |
| GHOULBOX | 2.3.2 | `9f2b9a22e456d7fe9552b40f7a1d0c13200913bb` |

Original release package hashes select the source-native catalogue engines;
explicitly imported binaries still use the separate binary runtime. ChoralRoot's
source-host second-part job is synchronous on the engine worker. Its process-exit
and at-exit hooks are disabled for embedding. GHOULBOX user sample references
use checked flash offsets instead of truncated host pointers. The pristine source
cache is retained; adaptations apply only to ignored build copies. Original
licensing notices and component texts are retained in licenses/.

## Emulator refresh and JIANT — 2026-10-23

fm1-emu is pinned to `eb0d897cf6e0ebf4c412e2389a113d878ec28dd0`;
see `Vendor/UPSTREAM.md` for local embedding adaptations.
JIANT 0.9.9-par12 (kind 16) uses the published stable installer source
`527c7e462a1542cbbde2c2cbf6b4bf151954127d`, rather than the later development
branch. Its exact public package selects the matching source-native build.
The upstream `web/emu/felucca_web.c` bridge supplies DSP, UI, keys and NOR storage;
our additions builder namespaces symbols and isolates writable state per instance.
Source, generated resources, package and component notices are pinned in the
additions/package locks. It uses no PI32 interpreter or JIT. USB, serial, hardware
update and device-specific behavior remain outside this host bridge.

The 2026-10-28 refresh checked each bundled firmware’s upstream release or
installer. Prereleases are included: Melodee is pinned to 1.6.0-rc.5. JIANT
retains the Standard edition at the installer’s latest Standard build,
0.9.9-par12 (527c7e4); the newer Sample preview is a separate edition.
Stock V15, FM-1+VA 102, X0X 1.1.0, FoMni 0.4 and Jangada 1.0.1 remained
current. FM1 Open V14’s browser source fingerprint matches the publisher’s
workbench metadata; native compilation remains a portable preview.

Stock V16 (kind 17) is the unmodified 718196-byte package linked from M-VAVE’s
official downloads page (release 2026-10-21):
https://yms-file-store.oss-cn-hongkong.aliyuncs.com/software/firmware/FM-1.fwsc .
SHA256: `d017fb65e70aca97ee29d944470c3515a74581df5c0576bf70de917c7b2b61d5`.
It is bundled alongside V15 and Baud Girl 102 through the CPU/SoC emulator,
with its own save slot. Baud Girl’s publisher still lists 102 as the latest
FM-1 release as checked on 2026-10-28. Attribution and firmware terms are in
`licenses/NOTICE-stock-v16`; V15-specific recovery terms are not extended to V16.
