//! Exact package/source pins select separate compiled firmware implementations.
#[cfg(fm1_native_additions)]
mod choralroot {
    use super::Api;
    extern "C" {
        fn chr_state_size() -> u64;
        fn chr_state_init(state: *mut u8) -> i32;
        fn chr_bind(state: *mut u8);
        fn chr_init();
        fn chr_ctl() -> u32;
        fn chr_rate() -> u32;
        fn chr_render(out: *mut i32, frames: u32);
        fn chr_draw(out: *mut u16);
        fn chr_physical_key(id: u32, down: i32);
        fn chr_release_all();
        fn chr_physical_encoder(id: u32, delta: i32);
        fn chr_physical_leds(out: *mut f32);
        fn chr_volume(value: u32);
        fn chr_flash_load(bytes: *const u8);
        fn chr_flash_copy(bytes: *mut u8);
        fn chr_workload_metrics(out: *mut u64, capacity: u32) -> u32;
    }
    pub const API: Api = Api {
        kind: 14,
        flash_size: 1048576,
        alignment: 16,
        state_size: chr_state_size,
        state_init: chr_state_init,
        bind: chr_bind,
        init: chr_init,
        ctl: chr_ctl,
        rate: chr_rate,
        render: chr_render,
        draw: chr_draw,
        key: chr_physical_key,
        release_all: chr_release_all,
        encoder: chr_physical_encoder,
        leds: chr_physical_leds,
        volume: chr_volume,
        flash_load: chr_flash_load,
        flash_copy: chr_flash_copy,
        metrics: chr_workload_metrics,
        destroy: None,
    };
}
#[cfg(fm1_native_additions)]
mod ghoulbox {
    use super::Api;
    extern "C" {
        fn ghb_state_size() -> u64;
        fn ghb_state_init(state: *mut u8) -> i32;
        fn ghb_bind(state: *mut u8);
        fn ghb_init();
        fn ghb_ctl() -> u32;
        fn ghb_rate() -> u32;
        fn ghb_render(out: *mut i32, frames: u32);
        fn ghb_draw(out: *mut u16);
        fn ghb_physical_key(id: u32, down: i32);
        fn ghb_release_all();
        fn ghb_physical_encoder(id: u32, delta: i32);
        fn ghb_physical_leds(out: *mut f32);
        fn ghb_volume(value: u32);
        fn ghb_flash_load(bytes: *const u8);
        fn ghb_flash_copy(bytes: *mut u8);
        fn ghb_workload_metrics(out: *mut u64, capacity: u32) -> u32;
    }
    pub const API: Api = Api {
        kind: 15,
        flash_size: 1048576,
        alignment: 16,
        state_size: ghb_state_size,
        state_init: ghb_state_init,
        bind: ghb_bind,
        init: ghb_init,
        ctl: ghb_ctl,
        rate: ghb_rate,
        render: ghb_render,
        draw: ghb_draw,
        key: ghb_physical_key,
        release_all: ghb_release_all,
        encoder: ghb_physical_encoder,
        leds: ghb_physical_leds,
        volume: ghb_volume,
        flash_load: ghb_flash_load,
        flash_copy: ghb_flash_copy,
        metrics: ghb_workload_metrics,
        destroy: None,
    };
}
#[cfg(fm1_native_additions)]
mod jiant {
    use super::Api;
    extern "C" {
        fn jnt_state_size() -> u64;
        fn jnt_state_init(state: *mut u8) -> i32;
        fn jnt_bind(state: *mut u8);
        fn jnt_init();
        fn jnt_ctl() -> u32;
        fn jnt_rate() -> u32;
        fn jnt_render(out: *mut i32, frames: u32);
        fn jnt_draw(out: *mut u16);
        fn jnt_physical_key(id: u32, down: i32);
        fn jnt_release_all();
        fn jnt_physical_encoder(id: u32, delta: i32);
        fn jnt_physical_leds(out: *mut f32);
        fn jnt_volume(value: u32);
        fn jnt_flash_load(bytes: *const u8);
        fn jnt_flash_copy(bytes: *mut u8);
        fn jnt_workload_metrics(out: *mut u64, capacity: u32) -> u32;
    }
    pub const API: Api = Api {
        kind: 16,
        flash_size: 1048576,
        alignment: 16,
        state_size: jnt_state_size,
        state_init: jnt_state_init,
        bind: jnt_bind,
        init: jnt_init,
        ctl: jnt_ctl,
        rate: jnt_rate,
        render: jnt_render,
        draw: jnt_draw,
        key: jnt_physical_key,
        release_all: jnt_release_all,
        encoder: jnt_physical_encoder,
        leds: jnt_physical_leds,
        volume: jnt_volume,
        flash_load: jnt_flash_load,
        flash_copy: jnt_flash_copy,
        metrics: jnt_workload_metrics,
        destroy: None,
    };
}
use super::native_sloop::Api;
#[cfg(fm1_native_clipper)]
mod clipper {
    use super::Api;
    extern "C" {
        fn clp_state_size() -> u64;
        fn clp_state_init(state: *mut u8) -> i32;
        fn clp_bind(state: *mut u8);
        fn clp_init();
        fn clp_ctl() -> u32;
        fn clp_rate() -> u32;
        fn clp_render(out: *mut i32, frames: u32);
        fn clp_draw(out: *mut u16);
        fn clp_physical_key(id: u32, down: i32);
        fn clp_release_all();
        fn clp_physical_encoder(id: u32, delta: i32);
        fn clp_physical_leds(out: *mut f32);
        fn clp_volume(value: u32);
        fn clp_flash_load(bytes: *const u8);
        fn clp_flash_copy(bytes: *mut u8);
        fn clp_workload_metrics(out: *mut u64, capacity: u32) -> u32;
    }
    pub const API: Api = Api {
        kind: 10,
        flash_size: 1048576,
        alignment: 16,
        state_size: clp_state_size,
        state_init: clp_state_init,
        bind: clp_bind,
        init: clp_init,
        ctl: clp_ctl,
        rate: clp_rate,
        render: clp_render,
        draw: clp_draw,
        key: clp_physical_key,
        release_all: clp_release_all,
        encoder: clp_physical_encoder,
        leds: clp_physical_leds,
        volume: clp_volume,
        flash_load: clp_flash_load,
        flash_copy: clp_flash_copy,
        metrics: clp_workload_metrics,
        destroy: None,
    };
}
#[cfg(fm1_native_family)]
mod felucca {
    use super::Api;
    extern "C" {
        fn fel_state_size() -> u64;
        fn fel_state_init(state: *mut u8) -> i32;
        fn fel_bind(state: *mut u8);
        fn fel_init();
        fn fel_ctl() -> u32;
        fn fel_rate() -> u32;
        fn fel_render(out: *mut i32, frames: u32);
        fn fel_draw(out: *mut u16);
        fn fel_physical_key(id: u32, down: i32);
        fn fel_release_all();
        fn fel_physical_encoder(id: u32, delta: i32);
        fn fel_physical_leds(out: *mut f32);
        fn fel_volume(value: u32);
        fn fel_flash_load(bytes: *const u8);
        fn fel_flash_copy(bytes: *mut u8);
        fn fel_workload_metrics(out: *mut u64, capacity: u32) -> u32;
    }
    pub const API: Api = Api {
        kind: 3,
        flash_size: 1048576,
        alignment: 16,
        state_size: fel_state_size,
        state_init: fel_state_init,
        bind: fel_bind,
        init: fel_init,
        ctl: fel_ctl,
        rate: fel_rate,
        render: fel_render,
        draw: fel_draw,
        key: fel_physical_key,
        release_all: fel_release_all,
        encoder: fel_physical_encoder,
        leds: fel_physical_leds,
        volume: fel_volume,
        flash_load: fel_flash_load,
        flash_copy: fel_flash_copy,
        metrics: fel_workload_metrics,
        destroy: None,
    };
}
#[cfg(fm1_native_family)]
mod x0x {
    use super::Api;
    extern "C" {
        fn x0x_state_size() -> u64;
        fn x0x_state_init(state: *mut u8) -> i32;
        fn x0x_bind(state: *mut u8);
        fn x0x_init();
        fn x0x_ctl() -> u32;
        fn x0x_rate() -> u32;
        fn x0x_render(out: *mut i32, frames: u32);
        fn x0x_draw(out: *mut u16);
        fn x0x_physical_key(id: u32, down: i32);
        fn x0x_release_all();
        fn x0x_physical_encoder(id: u32, delta: i32);
        fn x0x_physical_leds(out: *mut f32);
        fn x0x_volume(value: u32);
        fn x0x_flash_load(bytes: *const u8);
        fn x0x_flash_copy(bytes: *mut u8);
        fn x0x_workload_metrics(out: *mut u64, capacity: u32) -> u32;
    }
    pub const API: Api = Api {
        kind: 4,
        flash_size: 1048576,
        alignment: 16,
        state_size: x0x_state_size,
        state_init: x0x_state_init,
        bind: x0x_bind,
        init: x0x_init,
        ctl: x0x_ctl,
        rate: x0x_rate,
        render: x0x_render,
        draw: x0x_draw,
        key: x0x_physical_key,
        release_all: x0x_release_all,
        encoder: x0x_physical_encoder,
        leds: x0x_physical_leds,
        volume: x0x_volume,
        flash_load: x0x_flash_load,
        flash_copy: x0x_flash_copy,
        metrics: x0x_workload_metrics,
        destroy: None,
    };
}
#[cfg(fm1_native_family)]
mod melodee {
    use super::Api;
    extern "C" {
        fn mel_state_size() -> u64;
        fn mel_state_init(state: *mut u8) -> i32;
        fn mel_bind(state: *mut u8);
        fn mel_init();
        fn mel_ctl() -> u32;
        fn mel_rate() -> u32;
        fn mel_render(out: *mut i32, frames: u32);
        fn mel_draw(out: *mut u16);
        fn mel_physical_key(id: u32, down: i32);
        fn mel_release_all();
        fn mel_physical_encoder(id: u32, delta: i32);
        fn mel_physical_leds(out: *mut f32);
        fn mel_volume(value: u32);
        fn mel_flash_load(bytes: *const u8);
        fn mel_flash_copy(bytes: *mut u8);
        fn mel_workload_metrics(out: *mut u64, capacity: u32) -> u32;
    }
    pub const API: Api = Api {
        kind: 5,
        flash_size: 1048576,
        alignment: 16,
        state_size: mel_state_size,
        state_init: mel_state_init,
        bind: mel_bind,
        init: mel_init,
        ctl: mel_ctl,
        rate: mel_rate,
        render: mel_render,
        draw: mel_draw,
        key: mel_physical_key,
        release_all: mel_release_all,
        encoder: mel_physical_encoder,
        leds: mel_physical_leds,
        volume: mel_volume,
        flash_load: mel_flash_load,
        flash_copy: mel_flash_copy,
        metrics: mel_workload_metrics,
        destroy: None,
    };
}
#[cfg(fm1_native_official_preview)]
mod official {
    use super::Api;
    extern "C" {
        fn ofp_state_size() -> u64;
        fn ofp_state_init(state: *mut u8) -> i32;
        fn ofp_bind(state: *mut u8);
        fn ofp_init();
        fn ofp_ctl() -> u32;
        fn ofp_rate() -> u32;
        fn ofp_render(out: *mut i32, frames: u32);
        fn ofp_draw(out: *mut u16);
        fn ofp_physical_key(id: u32, down: i32);
        fn ofp_release_all();
        fn ofp_physical_encoder(id: u32, delta: i32);
        fn ofp_physical_leds(out: *mut f32);
        fn ofp_volume(value: u32);
        fn ofp_flash_load(bytes: *const u8);
        fn ofp_flash_copy(bytes: *mut u8);
        fn ofp_workload_metrics(out: *mut u64, capacity: u32) -> u32;
        fn ofp_destroy();
    }
    pub const API: Api = Api {
        kind: 6,
        flash_size: 536576,
        alignment: 64,
        state_size: ofp_state_size,
        state_init: ofp_state_init,
        bind: ofp_bind,
        init: ofp_init,
        ctl: ofp_ctl,
        rate: ofp_rate,
        render: ofp_render,
        draw: ofp_draw,
        key: ofp_physical_key,
        release_all: ofp_release_all,
        encoder: ofp_physical_encoder,
        leds: ofp_physical_leds,
        volume: ofp_volume,
        flash_load: ofp_flash_load,
        flash_copy: ofp_flash_copy,
        metrics: ofp_workload_metrics,
        destroy: Some(ofp_destroy),
    };
}
#[cfg(fm1_native_skiff)]
mod skiff {
    use super::Api;
    extern "C" {
        fn skf_state_size() -> u64;
        fn skf_state_init(state: *mut u8) -> i32;
        fn skf_bind(state: *mut u8);
        fn skf_init();
        fn skf_ctl() -> u32;
        fn skf_rate() -> u32;
        fn skf_render(out: *mut i32, frames: u32);
        fn skf_draw(out: *mut u16);
        fn skf_physical_key(id: u32, down: i32);
        fn skf_release_all();
        fn skf_physical_encoder(id: u32, delta: i32);
        fn skf_physical_leds(out: *mut f32);
        fn skf_volume(value: u32);
        fn skf_flash_load(bytes: *const u8);
        fn skf_flash_copy(bytes: *mut u8);
        fn skf_workload_metrics(out: *mut u64, capacity: u32) -> u32;
    }
    pub const API: Api = Api {
        kind: 7,
        flash_size: 1048576,
        alignment: 16,
        state_size: skf_state_size,
        state_init: skf_state_init,
        bind: skf_bind,
        init: skf_init,
        ctl: skf_ctl,
        rate: skf_rate,
        render: skf_render,
        draw: skf_draw,
        key: skf_physical_key,
        release_all: skf_release_all,
        encoder: skf_physical_encoder,
        leds: skf_physical_leds,
        volume: skf_volume,
        flash_load: skf_flash_load,
        flash_copy: skf_flash_copy,
        metrics: skf_workload_metrics,
        destroy: None,
    };
}
#[cfg(fm1_native_expansion)]
mod fomni {
    use super::Api;
    extern "C" {
        fn omf_state_size() -> u64;
        fn omf_state_init(state: *mut u8) -> i32;
        fn omf_bind(state: *mut u8);
        fn omf_init();
        fn omf_ctl() -> u32;
        fn omf_rate() -> u32;
        fn omf_render(out: *mut i32, frames: u32);
        fn omf_draw(out: *mut u16);
        fn omf_physical_key(id: u32, down: i32);
        fn omf_release_all();
        fn omf_physical_encoder(id: u32, delta: i32);
        fn omf_physical_leds(out: *mut f32);
        fn omf_volume(value: u32);
        fn omf_flash_load(bytes: *const u8);
        fn omf_flash_copy(bytes: *mut u8);
        fn omf_workload_metrics(out: *mut u64, capacity: u32) -> u32;
    }
    pub const API: Api = Api {
        kind: 8,
        flash_size: 1048576,
        alignment: 16,
        state_size: omf_state_size,
        state_init: omf_state_init,
        bind: omf_bind,
        init: omf_init,
        ctl: omf_ctl,
        rate: omf_rate,
        render: omf_render,
        draw: omf_draw,
        key: omf_physical_key,
        release_all: omf_release_all,
        encoder: omf_physical_encoder,
        leds: omf_physical_leds,
        volume: omf_volume,
        flash_load: omf_flash_load,
        flash_copy: omf_flash_copy,
        metrics: omf_workload_metrics,
        destroy: None,
    };
}
#[cfg(fm1_native_expansion)]
mod jangada {
    use super::Api;
    extern "C" {
        fn jng_state_size() -> u64;
        fn jng_state_init(state: *mut u8) -> i32;
        fn jng_bind(state: *mut u8);
        fn jng_init();
        fn jng_ctl() -> u32;
        fn jng_rate() -> u32;
        fn jng_render(out: *mut i32, frames: u32);
        fn jng_draw(out: *mut u16);
        fn jng_physical_key(id: u32, down: i32);
        fn jng_release_all();
        fn jng_physical_encoder(id: u32, delta: i32);
        fn jng_physical_leds(out: *mut f32);
        fn jng_volume(value: u32);
        fn jng_flash_load(bytes: *const u8);
        fn jng_flash_copy(bytes: *mut u8);
        fn jng_workload_metrics(out: *mut u64, capacity: u32) -> u32;
    }
    pub const API: Api = Api {
        kind: 9,
        flash_size: 1048576,
        alignment: 16,
        state_size: jng_state_size,
        state_init: jng_state_init,
        bind: jng_bind,
        init: jng_init,
        ctl: jng_ctl,
        rate: jng_rate,
        render: jng_render,
        draw: jng_draw,
        key: jng_physical_key,
        release_all: jng_release_all,
        encoder: jng_physical_encoder,
        leds: jng_physical_leds,
        volume: jng_volume,
        flash_load: jng_flash_load,
        flash_copy: jng_flash_copy,
        metrics: jng_workload_metrics,
        destroy: None,
    };
}
pub(super) fn for_package(bytes: &[u8]) -> Option<Api> {
    for_digest(super::sha256::digest(bytes), bytes.len())
}
pub(super) fn for_digest(digest: [u8; 32], len: usize) -> Option<Api> {
    #[cfg(fm1_native_sloop)]
    if (len == 0 || len == 610052) && digest == [84, 68, 175, 211, 198, 156, 64, 147, 82, 225, 174, 211, 90, 86, 32, 95, 19, 241, 30, 96, 121, 13, 225, 70, 89, 60, 70, 24, 164, 63, 53, 40]
    {
        return Some(super::native_sloop::SLOOP);
    }
    #[cfg(fm1_native_clipper)]
    if (len == 0 || len == 610072) && digest == [154, 36, 213, 221, 32, 165, 191, 248, 99, 112, 157, 50, 143, 44, 39, 127, 167, 16, 221, 49, 124, 137, 123, 204, 14, 132, 12, 93, 48, 64, 162, 230]
    {
        return Some(clipper::API);
    }
    #[cfg(fm1_native_family)]
    if (len == 0 || len == 610286) && digest == [191, 247, 198, 23, 207, 178, 200, 142, 102, 178, 42, 166, 145, 203, 121, 141, 250, 13, 156, 177, 47, 249, 167, 61, 234, 181, 29, 64, 78, 130, 129, 124]
    {
        return Some(felucca::API);
    }
    #[cfg(fm1_native_family)]
    if (len == 0 || len == 609684)
        && digest
            == [
                160, 194, 157, 235, 147, 12, 101, 241, 212, 11, 86, 145, 183, 214, 115, 248, 97,
                65, 139, 67, 139, 126, 101, 2, 124, 185, 198, 113, 230, 244, 79, 91,
            ]
    {
        return Some(x0x::API);
    }
    #[cfg(fm1_native_family)]
    if (len == 0 || len == 610583) && digest == [17, 104, 156, 65, 229, 217, 226, 27, 63, 32, 126, 28, 240, 25, 47, 190, 3, 122, 16, 8, 110, 148, 176, 184, 230, 26, 210, 30, 171, 148, 136, 121]
    {
        return Some(melodee::API);
    }
    #[cfg(fm1_native_official_preview)]
    if (len == 0 || len == OFFICIAL_PREVIEW_PACKAGE_SIZE)
        && digest == OFFICIAL_PREVIEW_PACKAGE_SHA256
    {
        return Some(official::API);
    }
    #[cfg(fm1_native_expansion)]
    if (len == 0 || len == 609661)
        && digest
            == [
                113, 250, 131, 4, 185, 27, 255, 137, 7, 139, 253, 39, 233, 129, 237, 40, 202, 79,
                13, 60, 95, 64, 110, 143, 219, 90, 42, 178, 64, 251, 219, 181,
            ]
    {
        return Some(fomni::API);
    }
    #[cfg(fm1_native_expansion)]
    if (len == 0 || len == 610133)
        && digest
            == [
                195, 130, 227, 52, 5, 123, 209, 7, 41, 73, 64, 228, 86, 229, 193, 20, 7, 84, 145,
                123, 161, 82, 161, 149, 138, 243, 170, 22, 150, 51, 53, 136,
            ]
    {
        return Some(jangada::API);
    }
    #[cfg(fm1_native_skiff)]
    if (len == 0 || len == 610429) && digest == [227, 89, 46, 92, 224, 72, 35, 223, 138, 199, 43, 144, 246, 156, 58, 77, 87, 92, 177, 52, 73, 204, 59, 185, 232, 69, 207, 27, 250, 61, 84, 106]
    {
        return Some(skiff::API);
    }
    #[cfg(fm1_native_additions)]
    if (len == 0 || len == 610313) && digest == [160, 159, 158, 25, 182, 106, 145, 53, 29, 227, 124, 83, 46, 13, 21, 205, 192, 63, 249, 233, 200, 42, 11, 21, 44, 219, 102, 202, 63, 220, 3, 38] {
        return Some(choralroot::API);
    }
    #[cfg(fm1_native_additions)]
    if (len == 0 || len == 610086) && digest == [146, 99, 10, 226, 120, 212, 160, 180, 35, 63, 242, 81, 43, 98, 173, 155, 93, 190, 151, 106, 76, 144, 131, 4, 98, 34, 217, 41, 58, 141, 247, 73] {
        return Some(ghoulbox::API);
    }
    #[cfg(fm1_native_additions)]
    if (len == 0 || len == 610280) && digest == [151, 56, 244, 115, 248, 181, 190, 212, 186, 106, 175, 182, 19, 47, 82, 13, 3, 224, 150, 131, 82, 26, 50, 9, 7, 1, 251, 244, 235, 106, 187, 60] {
        return Some(jiant::API);
    }
    None
}

#[cfg(all(test, fm1_native_family))]
mod tests {
    use super::*;
    use crate::native_sloop::Native;
    fn api(kind: u32) -> Api {
        match kind {
            3 => felucca::API,
            4 => x0x::API,
            5 => melodee::API,
            #[cfg(fm1_native_skiff)]
            7 => skiff::API,
            #[cfg(fm1_native_expansion)]
            8 => fomni::API,
            #[cfg(fm1_native_expansion)]
            9 => jangada::API,
            #[cfg(fm1_native_clipper)]
            10 => clipper::API,
            #[cfg(fm1_native_additions)]
            14 => choralroot::API,
            #[cfg(fm1_native_additions)]
            15 => ghoulbox::API,
            #[cfg(fm1_native_additions)]
            16 => jiant::API,
            _ => panic!("kind"),
        }
    }
    fn kinds() -> Vec<u32> {
        let mut kinds = vec![3, 4, 5];
        #[cfg(fm1_native_skiff)]
        kinds.push(7);
        #[cfg(fm1_native_expansion)]
        kinds.extend([8, 9]);
        #[cfg(fm1_native_clipper)]
        kinds.push(10);
        #[cfg(fm1_native_additions)]
        kinds.extend([14, 15, 16]);
        kinds
    }
    fn run(m: &mut Native, frames: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * 2);
        let mut done = 0;
        for count in [1usize, 31, 257, 513].into_iter().cycle() {
            let n = count.min(frames - done);
            let mut b = vec![0.; n * 2];
            assert_eq!(m.render(&mut b), n);
            assert!(b.iter().all(|x| x.is_finite() && x.abs() <= 1.));
            out.extend(b);
            done += n;
            if done == frames {
                break;
            }
        }
        out
    }
    #[test]
    fn source_firmwares_sound_draw_and_roundtrip_complete_flash() {
        for kind in kinds() {
            let mut blank = vec![0xff; 1 << 20];
            blank[123] = kind as u8;
            let mut m = Native::with_api(&blank, api(kind)).unwrap();
            assert_eq!(m.kind(), kind);
            // Let the source hosts finish their boot splashes before note input.
            run(&mut m, 88200);
            // Every note is exercised, including the drum-first X0X power-on page.
            for k in 14..41 {
                m.key(k, true);
            }
            let audio = run(&mut m, 44100);
            assert!(
                audio.iter().filter(|v| **v != 0.).count() > 100,
                "native {kind} must synthesize real PCM"
            );
            let mut screen = vec![0u32; 240 * 240];
            let mut leds = vec![0.; 41];
            unsafe {
                m.snapshot(screen.as_mut_ptr(), leds.as_mut_ptr());
            }
            assert!(
                screen.windows(2).any(|p| p[0] != p[1]),
                "native {kind} display"
            );
            m.release_all();
            run(&mut m, 44100);
            let mut flash = vec![0; 1 << 20];
            unsafe {
                assert_eq!(m.flash(flash.as_mut_ptr(), flash.len()), 1 << 20);
            }
            assert_eq!(flash[123], kind as u8, "unrelated firmware byte retained");
            assert!(!m.restore(&flash[..1024]));
            assert!(m.restore(&flash));
            let mut after = vec![0; flash.len()];
            unsafe {
                m.flash(after.as_mut_ptr(), after.len());
            }
            assert_eq!(
                flash, after,
                "complete raw flash must survive fresh instance restore"
            );
        }
    }
    #[test]
    fn source_instances_remain_independent_when_interleaved() {
        for kind in kinds() {
            let blank = vec![0xff; 1 << 20];
            let mut a = Native::with_api(&blank, api(kind)).unwrap();
            let mut b = Native::with_api(&blank, api(kind)).unwrap();
            let mut reference = Native::with_api(&blank, api(kind)).unwrap();
            run(&mut a, 88200);
            run(&mut b, 88200);
            run(&mut reference, 88200);
            // X0X's black keys trigger drums; its white keys edit steps.
            let first_key = if kind == 4 { 15 } else { 21 };
            let second_key = if kind == 4 { 17 } else { 26 };
            a.key(first_key, true);
            reference.key(first_key, true);
            b.key(second_key, true);
            let mut nonzero = 0;
            for _ in 0..20 {
                let expected = run(&mut reference, 513);
                nonzero += expected.iter().filter(|v| **v != 0.).count();
                run(&mut b, 257);
                assert_eq!(
                    run(&mut a, 513),
                    expected,
                    "native {kind} instance state leak"
                );
            }
            assert!(
                nonzero > 100,
                "native {kind}: isolation must compare real sound, not silence"
            );
        }
    }
    #[cfg(fm1_native_expansion)]
    #[test]
    fn new_source_projects_autosave_and_restore_the_edited_logical_state() {
        fn signature(m: &mut Native) -> u64 {
            let mut values = [0u64; 16];
            unsafe {
                m.metrics(values.as_mut_ptr(), 16);
            }
            values[12]
        }
        for kind in [8, 9] {
            let mut blank = vec![255; 1 << 20];
            blank[123] = 77;
            let mut original = Native::with_api(&blank, api(kind)).unwrap();
            run(&mut original, 1024);
            let initial = signature(&mut original);
            original.encoder(6, 1); // Physical PRESETS, one actual firmware edit.
            run(&mut original, 44100 * 30); // Original quiet/gap autosave policy.
            let edited = signature(&mut original);
            assert_ne!(
                initial, edited,
                "native {kind}: the physical knob must change the project"
            );
            let mut flash = vec![0; 1 << 20];
            unsafe {
                original.flash(flash.as_mut_ptr(), flash.len());
            }
            assert_ne!(flash, blank, "upstream storage must commit a real object");
            assert_eq!(flash[123], 77, "unrelated flash area survives the commit");
            drop(original);
            let mut reopened = Native::with_api(&flash, api(kind)).unwrap();
            run(&mut reopened, 1024);
            assert_eq!(
                signature(&mut reopened),
                edited,
                "native {kind}: edited project must reload semantically"
            );
        }
    }
    #[test]
    fn catalogue_native_hash_lookup_is_exact_and_rejects_renamed_content() {
        let root = std::path::PathBuf::from(
            std::env::var_os("FM1_NATIVE_FAMILY").expect("configured native source root"),
        );
        let mut cases = vec![
            (3, root.join("felucca-1.7.0.fwsc")),
            (4, root.join("x0x-1.1.0.fwsc")),
            (5, root.join("melodee-1.6.0-rc.5.fwsc")),
        ];
        #[cfg(fm1_native_expansion)]
        {
            let extra = std::path::PathBuf::from(std::env::var_os("FM1_NATIVE_EXPANSION").unwrap());
            cases.extend([
                (8, extra.join("fomni-0.4.fwsc")),
                (9, extra.join("jangada-1.0.1.fwsc")),
            ]);
        }
        #[cfg(fm1_native_skiff)]
        {
            let extra = std::path::PathBuf::from(std::env::var_os("FM1_NATIVE_SKIFF").unwrap());
            cases.push((7, extra.parent().unwrap().join("skiff-2.0.2.fwsc")));
        }
        #[cfg(fm1_native_additions)]
        {
            let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
            cases.extend([
                (14, root.join("Research/ChoralRoot/choralroot-0.27.fwsc")),
                (15, root.join("Research/Ghoulbox/ghoulbox-2.4.fwsc")),
                (16, root.join("Research/Jiant/jiant-0.9.9-par12.fwsc")),
            ]);
        }
        for (kind, path) in cases {
            let mut package = std::fs::read(path).unwrap();
            assert_eq!(for_package(&package).unwrap().kind, kind);
            let m = crate::Instrument::load(&package).unwrap();
            assert_eq!(unsafe { crate::instrument_native_kind(&m) }, kind);
            package[100] ^= 1;
            assert!(for_package(&package).is_none());
        }
    }
}

#[cfg(fm1_native_official_preview)]
const OFFICIAL_PREVIEW_PACKAGE_SIZE: usize = 492084;
#[cfg(fm1_native_official_preview)]
const OFFICIAL_PREVIEW_PACKAGE_SHA256: [u8; 32] = [126, 123, 109, 38, 1, 99, 205, 249, 215, 22, 247, 250, 180, 79, 25, 190, 96, 14, 222, 255, 205, 57, 13, 231, 67, 112, 184, 60, 145, 127, 208, 198];
