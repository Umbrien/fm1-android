# fm1-emu core provenance

Source: https://github.com/joakimeriksson/fm1-emu
Revision: c1c045fc067da024d686e7a690806e2e55795042 (2026-10-23 retrieval).
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
