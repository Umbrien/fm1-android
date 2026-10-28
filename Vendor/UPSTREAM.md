# fm1-emu core provenance

Source: https://github.com/joakimeriksson/fm1-emu
Revision: eb0d897cf6e0ebf4c412e2389a113d878ec28dd0 (2026-10-28 retrieval; post-0.3.0 master).
License: MIT, Joakim Eriksson. See LICENSE-fm1-emu; also included in the app.
Only fm1-isa, fm1-core and fm1-soc are vendored; no JUCE or firmware banks.
Cargo metadata uses local workspace versions and the upstream MIT license.

The upstream CPU/JIT, ISA and SoC replace the previous 7ca5b96-based local
interpreter/JIT and experimental dual-core scheduler. This includes upstream
register caching, compiled flag handling, computed jumps, polling-loop skipping,
CPU1 release latching, and in-block self-modifying RAM-code checks.

Local embedding changes retained/adapted:
- Android JIT code reserves inaccessible pages, writes fresh whole pages RW,
  flushes instructions and publishes RX; never RWX. Failed publication does not
  revoke existing blocks. Code pages and boxed helper/chain data belong to the CPU.
- Rust helper panics are contained until generated frames return.
- Actual JIT instruction counts remain available without helper profiling;
  compilation timing and architectural snapshots support local diagnostics/tests.
- Firmware parsing rejects truncated/out-of-range app headers and marker overflow.
- Flash changes to fetched XIP words invalidate decode/block/JIT caches for each
  core; saved flash restore requires the complete 1 MiB image at the C ABI.

The old optional Apple static-kernel experiments are not enabled by this upstream
runtime. Their historical helper sources remain for offline research only.
Core/src/lib.rs adapts upstream machine boot/execution to the instance C ABI.
Source-native engines remain separate, pinned GPL-compatible implementations.

2026-10-28 update: includes the 0.3.0 stock V16 protected-image/EXT_RESERVED
unpacking fix, FPU rounding and byte-reversal instructions, delayed SPI1 LCD
interrupts, the P33 watchdog/chip-reset model, and the subsequent CPU/SoC
performance fixes. Upstream release and implementation notes:
https://github.com/joakimeriksson/fm1-emu/releases/tag/v0.3.0
https://github.com/joakimeriksson/fm1-emu/blob/eb0d897cf6e0ebf4c412e2389a113d878ec28dd0/docs/stock-firmware.md

The Android backend retains heap-owned indirect chain slots rather than
upstream's newly patched direct branch instructions. This preserves the existing
RW-to-RX publication policy: linking changes data and never writes an RX code
page or makes a published code page writable. Other supported hosts use the
new upstream branch linking. Existing allocation ownership, panic containment,
package bounds checks and fetched-XIP invalidation remain in place.
