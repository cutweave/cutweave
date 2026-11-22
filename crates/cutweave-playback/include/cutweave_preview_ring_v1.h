/* SPDX-License-Identifier: GPL-3.0-or-later */
/*
 * Cutweave preview frame ring, RING_LAYOUT_V1: C mirror of the binary contract.
 *
 * Normative text: ../RING.md. Rust source of truth: ../src/shm/layout.rs. A unit test in
 * cutweave-playback checks every CWPR_* #define below against the Rust constants, so this
 * file cannot drift silently. Readers (the napi addon, WP-1H.03) mirror these structs or
 * use the offsets directly; every field both sides touch must be accessed atomically
 * (C11 <stdatomic.h> / std::atomic_ref / Rust atomics), little-endian, naturally aligned.
 */
#ifndef CUTWEAVE_PREVIEW_RING_V1_H
#define CUTWEAVE_PREVIEW_RING_V1_H

#include <stddef.h>
#include <stdint.h>

/* --- Identity ----------------------------------------------------------------------- */
#define CWPR_MAGIC 0x52505743u /* "CWPR" as little-endian u32 */
#define CWPR_LAYOUT_VERSION 1u
#define CWPR_KNOWN_REQUIRED_FEATURES 0u

/* --- Geometry ----------------------------------------------------------------------- */
#define CWPR_HEADER_SIZE 1024u
#define CWPR_SLOT_DESC_SIZE 128u
#define CWPR_MIN_SLOTS 4u
#define CWPR_MAX_SLOTS 6u
#define CWPR_PAYLOAD_ALIGN 16384u
#define CWPR_NAME_FIELD_LEN 32u
#define CWPR_MAX_DIMENSION 8192u
#define CWPR_FLICKS_PER_SECOND 705600000
#define CWPR_HEARTBEAT_TIMEOUT_NS 500000000u
#define CWPR_MAX_CLOCK_EXTRAPOLATION_NS 100000000u
#define CWPR_RATE_MILLI_1X 1000
#define CWPR_RING_NAME_LEN 29u /* "cwp-" + 8 hex pid + "-" + 16 hex random */

/* --- Enumerations ------------------------------------------------------------------- */
#define CWPR_WRITER_INITIALIZING 0u
#define CWPR_WRITER_LIVE 1u
#define CWPR_WRITER_RETIRED 2u
#define CWPR_WRITER_CLOSED 3u

#define CWPR_CLOCK_STOPPED 0u
#define CWPR_CLOCK_PLAYING 1u
#define CWPR_CLOCK_SCRUBBING 2u

#define CWPR_HOST_CLOCK_WINDOWS_QPC 1u
#define CWPR_HOST_CLOCK_LINUX_MONOTONIC 2u
#define CWPR_HOST_CLOCK_MAC_UPTIME_RAW 3u

#define CWPR_FORMAT_RGBA8 1u
#define CWPR_FORMAT_NV12 2u

#define CWPR_FLAG_LOWRES 1u

/* --- RingHeader field offsets (bytes from the start of the mapping) ------------------ */
#define CWPR_OFF_MAGIC 0u
#define CWPR_OFF_LAYOUT_VERSION 4u
#define CWPR_OFF_REQUIRED_FEATURES 8u
#define CWPR_OFF_HEADER_SIZE 12u
#define CWPR_OFF_SLOT_DESC_SIZE 16u
#define CWPR_OFF_SLOT_COUNT 20u
#define CWPR_OFF_PAYLOAD_OFFSET 24u
#define CWPR_OFF_SLOT_STRIDE 32u
#define CWPR_OFF_SLOT_CAPACITY 40u
#define CWPR_OFF_MAPPING_SIZE 48u
#define CWPR_OFF_RING_EPOCH 56u
#define CWPR_OFF_WRITER_PID 64u
#define CWPR_OFF_HOST_CLOCK 68u
#define CWPR_OFF_WRITE_INDEX 128u
#define CWPR_OFF_WRITER_STATE 136u
#define CWPR_OFF_HEARTBEAT_NS 144u
#define CWPR_OFF_FRAMES_DROPPED 152u
#define CWPR_OFF_CLOCK_LOCK 256u
#define CWPR_OFF_CLOCK_FLICKS 264u
#define CWPR_OFF_CLOCK_HOST_NS 272u
#define CWPR_OFF_CLOCK_RATE_MILLI 280u
#define CWPR_OFF_CLOCK_STATE 284u
#define CWPR_OFF_CONFIG_LOCK 384u
#define CWPR_OFF_CONFIG_GEN 392u
#define CWPR_OFF_PREVIEW_WIDTH 396u
#define CWPR_OFF_PREVIEW_HEIGHT 400u
#define CWPR_OFF_PREVIEW_FORMAT 404u
#define CWPR_OFF_SUCCESSOR_EPOCH 512u
#define CWPR_OFF_SUCCESSOR_NAME 520u

/* --- SlotDesc field offsets (bytes from the start of the descriptor) ---------------- */
#define CWPR_OFF_SLOT_LOCK 0u
#define CWPR_OFF_SLOT_SEQ 8u
#define CWPR_OFF_SLOT_PTS 16u
#define CWPR_OFF_SLOT_DURATION 24u
#define CWPR_OFF_SLOT_REV 32u
#define CWPR_OFF_SLOT_WIDTH 40u
#define CWPR_OFF_SLOT_HEIGHT 44u
#define CWPR_OFF_SLOT_FORMAT 48u
#define CWPR_OFF_SLOT_FLAGS 52u
#define CWPR_OFF_SLOT_CONFIG_GEN 56u
#define CWPR_OFF_SLOT_PAYLOAD_LEN 64u
#define CWPR_OFF_SLOT_PLANE0_OFFSET 72u
#define CWPR_OFF_SLOT_PLANE1_OFFSET 80u
#define CWPR_OFF_SLOT_PLANE0_STRIDE 88u
#define CWPR_OFF_SLOT_PLANE1_STRIDE 92u
#define CWPR_OFF_SLOT_RENDER_NS 96u
#define CWPR_OFF_SLOT_PUBLISH_NS 104u

/* --- Structs (plain view; access every field atomically) ----------------------------- */
typedef struct cwpr_ring_header {
    /* Block A (0..128): identity and geometry, immutable once magic is set. */
    uint32_t magic;             /* CWPR_MAGIC, stored last at init (release) */
    uint32_t layout_version;    /* CWPR_LAYOUT_VERSION */
    uint32_t required_features; /* reject if any bit outside CWPR_KNOWN_REQUIRED_FEATURES */
    uint32_t header_size;       /* offset of the slot table */
    uint32_t slot_desc_size;    /* CWPR_SLOT_DESC_SIZE */
    uint32_t slot_count;
    uint64_t payload_offset;    /* offset of payload 0, multiple of 4096 */
    uint64_t slot_stride;       /* distance between payloads */
    uint64_t slot_capacity;     /* max payload_len, <= slot_stride */
    uint64_t mapping_size;      /* payload_offset + slot_count * slot_stride */
    uint64_t ring_epoch;        /* random; must equal the announced epoch */
    uint32_t writer_pid;
    uint32_t host_clock;        /* CWPR_HOST_CLOCK_* of every *_ns field */
    uint64_t reserved_a[7];
    /* Block B (128..256): writer progress and liveness. */
    uint64_t write_index;       /* seq of the newest frame; 0 = none (acquire) */
    uint32_t writer_state;      /* CWPR_WRITER_* (acquire) */
    uint32_t reserved_b0;
    uint64_t heartbeat_ns;      /* host clock of the last publish/heartbeat */
    uint64_t frames_dropped;    /* producer-side drops (diagnostics) */
    uint64_t reserved_b[12];
    /* Block C (256..384): audio clock, seqlock clock_lock. */
    uint64_t clock_lock;        /* odd while being written */
    int64_t clock_flicks;
    uint64_t clock_host_ns;
    int32_t clock_rate_milli;   /* 1000 = 1x */
    uint32_t clock_state;       /* CWPR_CLOCK_* */
    uint64_t reserved_c[12];
    /* Block D (384..512): negotiated preview configuration, seqlock config_lock. */
    uint64_t config_lock;
    uint32_t config_gen;
    uint32_t preview_width;
    uint32_t preview_height;
    uint32_t preview_format;    /* CWPR_FORMAT_* */
    uint64_t reserved_d[13];
    /* Block E (512..1024): successor ring, valid once writer_state == RETIRED. */
    uint64_t successor_epoch;
    uint8_t successor_name[CWPR_NAME_FIELD_LEN]; /* ASCII, NUL padded */
    uint64_t reserved_e[59];
} cwpr_ring_header;

typedef struct cwpr_slot_desc {
    uint64_t lock;              /* seqlock: odd while the writer fills the slot */
    uint64_t seq;               /* 0 = empty */
    int64_t pts;                /* flicks */
    int64_t duration;           /* flicks, 0 = unknown */
    uint64_t rev;
    uint32_t width;
    uint32_t height;
    uint32_t format;            /* CWPR_FORMAT_* */
    uint32_t flags;             /* CWPR_FLAG_* */
    uint32_t config_gen;
    uint32_t reserved0;
    uint64_t payload_len;
    uint64_t plane0_offset;
    uint64_t plane1_offset;     /* NV12 UV plane; 0 for RGBA8 */
    uint32_t plane0_stride;
    uint32_t plane1_stride;     /* 0 for RGBA8 */
    uint64_t render_ns;         /* 0 = unknown */
    uint64_t publish_ns;
    uint64_t reserved1[2];
} cwpr_slot_desc;

#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
_Static_assert(sizeof(cwpr_ring_header) == CWPR_HEADER_SIZE, "header size");
_Static_assert(sizeof(cwpr_slot_desc) == CWPR_SLOT_DESC_SIZE, "slot desc size");
_Static_assert(offsetof(cwpr_ring_header, write_index) == CWPR_OFF_WRITE_INDEX, "write_index");
_Static_assert(offsetof(cwpr_ring_header, clock_lock) == CWPR_OFF_CLOCK_LOCK, "clock_lock");
_Static_assert(offsetof(cwpr_ring_header, config_lock) == CWPR_OFF_CONFIG_LOCK, "config_lock");
_Static_assert(offsetof(cwpr_ring_header, successor_name) == CWPR_OFF_SUCCESSOR_NAME, "succ");
_Static_assert(offsetof(cwpr_slot_desc, payload_len) == CWPR_OFF_SLOT_PAYLOAD_LEN, "len");
_Static_assert(offsetof(cwpr_slot_desc, publish_ns) == CWPR_OFF_SLOT_PUBLISH_NS, "publish");
#endif

#endif /* CUTWEAVE_PREVIEW_RING_V1_H */
