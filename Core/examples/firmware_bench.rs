//! Standalone, in-memory firmware benchmark. Never opens app saves or produces audio.
//! Usage: firmware_bench package.fwsc [jit|reference] [blocks=1000] [default|va|8bit] [chordPeriod=100] [held=75] [keys=21,25,28,32]
use fm1_ios_core::*;
use std::time::Instant;

unsafe fn blocks(m: *mut Instrument, count: usize) {
    let mut pcm = [0f32; 1024];
    for _ in 0..count { instrument_render(m, pcm.as_mut_ptr(), 512); }
}
unsafe fn press(m: *mut Instrument, id: u32) {
    instrument_key(m, id, true); blocks(m, 20);
    instrument_key(m, id, false); blocks(m, 20);
}
unsafe fn turn(m: *mut Instrument, delta: i32) {
    instrument_encoder(m, 0, delta); blocks(m, 30);
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert!(args.len() >= 2, "firmware_bench package.fwsc [jit|reference] [blocks] [default|va|8bit]");
    let bytes = std::fs::read(&args[1]).unwrap();
    let jit = args.get(2).map_or(true, |s| s == "jit");
    let count: usize = args.get(3).map_or(1000, |s| s.parse().unwrap());
    assert!(count > 0);
    let mode = args.get(4).map_or("default", String::as_str);
    let chord_period: usize=args.get(5).map_or(100,|s|s.parse().unwrap());
    let held: usize=args.get(6).map_or(75,|s|s.parse().unwrap());
    assert!(chord_period>0 && held>0 && held<chord_period);
    let keys: Vec<u32> = args.get(7).map_or_else(|| vec![21, 25, 28, 32],
        |value| value.split(',').map(|key| key.parse().expect("comma-separated key IDs")).collect());
    assert!(!keys.is_empty() && keys.len() <= 41 && keys.iter().all(|&key| key < 41));
    unsafe {
        let mut error = [0u8; 512];
        let start = Instant::now();
        let m = instrument_create_binary(bytes.as_ptr(), bytes.len(), jit, error.as_mut_ptr(), error.len());
        assert!(!m.is_null(), "{}", String::from_utf8_lossy(&error));
        blocks(m, 200);
        let boot_ms = start.elapsed().as_secs_f64() * 1000.0;
        // FM-1+VA 102 documented panel sequence, changing only this process's volatile state.
        if mode != "default" {
            assert!(mode == "va" || mode == "8bit");
            press(m, 6); turn(m, 32); turn(m, -1); press(m, 3); turn(m, 1); press(m, 3);
            for _ in 0..if mode == "va" { 1 } else { 2 } { turn(m, 1); }
            press(m, 3); press(m, 8);
        }
        let mut pcm = [0f32; 1024];
        let mut times = Vec::with_capacity(count);
        let mut frames = 0usize;
        let mut short = 0usize;
        let mut late = 0usize;
        let mut peak = 0f32;
        let mut hash = 0xcbf29ce484222325u64;
        let before = instrument_jit_instructions(m);
        let start = Instant::now();
        for i in 0..count {
            // Repeated four-note chords exercise note allocation and envelopes.
            if i % chord_period == 0 { for &key in &keys { instrument_key(m, key, true); } }
            if i % chord_period == held { for &key in &keys { instrument_key(m, key, false); } }
            let compile_before = instrument_jit_compilation_nanoseconds(m);
            let now = Instant::now();
            let n = instrument_render(m, pcm.as_mut_ptr(), 512);
            let ms = now.elapsed().as_secs_f64() * 1000.0;
            let compile_ms = instrument_jit_compilation_nanoseconds(m).saturating_sub(compile_before) as f64 / 1_000_000.0;
            times.push((ms, i, compile_ms));
            if ms > 512.0 / 44.1 { late += 1; }
            if n != 512 { short += 1; }
            frames += n;
            for s in &pcm[..n * 2] {
                assert!(s.is_finite()); peak = peak.max(s.abs());
                hash = (hash ^ s.to_bits() as u64).wrapping_mul(0x100000001b3);
            }
        }
        let elapsed = start.elapsed().as_secs_f64();
        times.sort_by(|a,b| a.0.total_cmp(&b.0));
        let worst: Vec<_> = times.iter().rev().take(10)
            .map(|&(ms, block, compile_ms)| format!("{{\"block\":{block},\"ms\":{ms:.3},\"compileMs\":{compile_ms:.3}}}"))
            .collect();
        println!("{{\"jit\":{jit},\"mode\":\"{mode}\",\"blocks\":{count},\"chordPeriodBlocks\":{chord_period},\"heldBlocks\":{held},\"keys\":{keys:?},\"boot_ms\":{boot_ms:.3},\"frames\":{frames},\"wall_seconds\":{elapsed:.6},\"realtime_ratio\":{:.4},\"p50_ms\":{:.3},\"p95_ms\":{:.3},\"p99_ms\":{:.3},\"max_ms\":{:.3},\"over_11_61_ms\":{late},\"short_blocks\":{short},\"peak\":{peak},\"pcm_hash\":\"{hash:016x}\",\"jit_instructions\":{}}}",
            frames as f64 / 44100.0 / elapsed, times[count / 2].0, times[(count * 95 / 100).min(count - 1)].0, times[(count * 99 / 100).min(count - 1)].0, times[count - 1].0, instrument_jit_instructions(m) - before);
        eprintln!("{{\"worstBlocks\":[{}]}}", worst.join(","));
        eprintln!("{}", (&*m).compiler_profile_json());
        instrument_destroy(m);
    }
}
