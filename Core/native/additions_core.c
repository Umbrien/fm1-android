/* SPDX-License-Identifier: GPL-3.0-only
 * Native Android bridge for the pinned ChoralRoot and GHOULBOX source hosts.
 * The upstream bridge compiles the real firmware DSP, UI and NOR storage, without
 * a PI32 CPU or JIT. All writable globals are isolated by fel_state.py.
 * Upstream bridge copyrights and component notices are retained in licenses/.
 */
#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <math.h>
#include <time.h>
#include <pthread.h>
#include "felucca_core_api.h"

static void host_exit(int status) { (void)status; }
static int host_atexit(void (*callback)(void)) { (void)callback; return 0; }
static char *host_getenv(const char *name) { (void)name; return NULL; }
#define exit host_exit
#define atexit host_atexit
#define getenv host_getenv

#if FM1_CHORALROOT
#include "../upstream/tools/emu/web/emu_web.c"
#else
#if !FM1_JIANT
static uint8_t host_sample_byte(uint32_t off);
#endif
#include "../upstream/web/emu/felucca_web.c"
#if !FM1_JIANT
static uint8_t host_sample_byte(uint32_t off) {
    if (off & 0x80000000u) {
        off &= 0x7fffffffu;
        return off < sizeof nor ? nor[off] : 0;
    }
    return off < sizeof SMP_DATA ? SMP_DATA[off] : 0;
}
#endif
#endif
#undef exit
#undef atexit
#undef getenv
#undef __attribute__

static uint32_t host_keys, host_buttons;
static uint64_t host_frames;
#pragma clang section bss = "" data = ""

void FEL(init)(void) { web_boot(); }
uint32_t FEL(ctl)(void) { return 128; }
uint32_t FEL(rate)(void) { return FS; }
void FEL(render)(int32_t *out, uint32_t frames) {
    for (uint32_t done = 0; done < frames;) {
        uint32_t n = frames - done;
        if (n > 1024) n = 1024;
        web_render(n);
        const float *l = web_out_l(), *r = web_out_r();
        for (uint32_t i = 0; i < n; i++) {
            out[2 * (done + i)] = (int32_t)(l[i] * 65536.0f);
            out[2 * (done + i) + 1] = (int32_t)(r[i] * 65536.0f);
        }
        done += n;
        host_frames += n;
    }
}
void FEL(draw)(uint16_t *out) {
#if FM1_CHORALROOT
    memcpy(out, web_fb(), 240 * 240 * sizeof *out);
#else
    memcpy(out, web_screen(), 240 * 240 * sizeof *out);
#endif
}
void FEL(physical_key)(uint32_t id, int down) {
    if (id >= 41) return;
    uint32_t *mask = id < 14 ? &host_buttons : &host_keys;
    uint32_t bit = 1u << (id < 14 ? id : id - 14);
    if (down) *mask |= bit; else *mask &= ~bit;
    web_buttons(host_buttons);
    web_keys(host_keys);
}
void FEL(release_all)(void) {
    host_keys = host_buttons = 0;
    web_buttons(0);
    web_keys(0);
}
void FEL(physical_encoder)(uint32_t id, int32_t delta) {
    /* Android physical IDs: SELECT, ALGO, K1..K4, PRESETS.
     * Source-host roles: SELECT, ALGO, PRESETS, K1..K4. */
    static const uint8_t role[7] = {0, 1, 3, 4, 5, 6, 2};
    if (id < 7) web_enc(role[id], delta);
}
void FEL(volume)(uint32_t value) { web_master(value > 1023 ? 1023 : value); }
void FEL(physical_leds)(float *out) {
#if FM1_CHORALROOT
    for (uint32_t id = 0; id < 41; id++) {
        float v = 0;
        for (uint32_t row = 1; row < 5; row++) for (uint32_t col = 0; col < EMU_NCOL; col++)
            if (emu_keymap[row][col] == (int8_t)id)
                v = (emu_hal.led[col] & (1u << row)) ? 0.086f :
                    (emu_hal.led_dim[col] & (1u << row)) ? 0.004f : 0;
        out[id] = v;
    }
#else
    uint32_t lit_b = web_lit_buttons(), dim_b = web_dim_buttons();
    uint32_t lit_k = web_lit_keys(), dim_k = web_dim_keys();
    for (uint32_t id = 0; id < 41; id++) {
        uint32_t bit = 1u << (id < 14 ? id : id - 14);
        uint32_t lit = id < 14 ? lit_b : lit_k, dim = id < 14 ? dim_b : dim_k;
        out[id] = (lit & bit) ? 0.086f : (dim & bit) ? 0.004f : 0;
    }
#endif
}
void FEL(flash_load)(const uint8_t *bytes) {
#if FM1_CHORALROOT
    memcpy(web_flash_stage(), bytes, 1048576);
    web_flash_stage_len(1048576);
    memcpy(web_flash(), bytes, 1048576);
#else
    memcpy(web_nor(), bytes, 1048576);
#endif
}
void FEL(flash_copy)(uint8_t *bytes) {
#if FM1_CHORALROOT
    memcpy(bytes, web_flash(), 1048576);
#else
    memcpy(bytes, web_nor(), 1048576);
#endif
}
uint32_t FEL(workload_metrics)(uint64_t *out, uint32_t capacity) {
    uint32_t n = capacity < 16 ? capacity : 16;
    for (uint32_t i = 0; i < n; i++) out[i] = i == 0 ? host_frames : 0;
    return n;
}
