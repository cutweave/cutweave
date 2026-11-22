// SPDX-License-Identifier: GPL-3.0-or-later
// Shared colour math in f32, prepended to every pass shader. Written independently of
// src/color.rs (the f64 CPU side used by the reference renderer); only the constants are common.
// Transfer ids: 0 linear, 1 sRGB, 2 BT.709, 3 gamma 2.2, 4 gamma 2.8, 5 SMPTE 240M, 6 PQ, 7 HLG.

const BT709_ALPHA: f32 = 1.09929682680944;
const BT709_BETA: f32 = 0.018053968510807;
const S240_ALPHA: f32 = 1.1115;
const S240_BETA: f32 = 0.0228;

const PQ_M1: f32 = 0.1593017578125;
const PQ_M2: f32 = 78.84375;
const PQ_C1: f32 = 0.8359375;
const PQ_C2: f32 = 18.8515625;
const PQ_C3: f32 = 18.6875;

const HLG_A: f32 = 0.17883277;
const HLG_B: f32 = 0.28466892;
const HLG_C: f32 = 0.55991073;

fn safe_pow(x: f32, y: f32) -> f32 {
  if (x <= 0.0) {
    return 0.0;
  }
  return pow(x, y);
}

// ST 2084: signal -> luminance / 10000 cd/m2.
fn pq_eotf(e: f32) -> f32 {
  let ep = safe_pow(e, 1.0 / PQ_M2);
  return safe_pow(max(ep - PQ_C1, 0.0) / (PQ_C2 - PQ_C3 * ep), 1.0 / PQ_M1);
}

// ST 2084 inverse: luminance / 10000 cd/m2 -> signal.
fn pq_inv_eotf(y: f32) -> f32 {
  let yp = safe_pow(y, PQ_M1);
  return safe_pow((PQ_C1 + PQ_C2 * yp) / (1.0 + PQ_C3 * yp), PQ_M2);
}

fn to_linear(v: f32, tf: u32) -> f32 {
  switch tf {
    case 1u: {
      if (v <= 0.04045) {
        return v / 12.92;
      }
      return pow((v + 0.055) / 1.055, 2.4);
    }
    case 2u: {
      if (v < 4.5 * BT709_BETA) {
        return v / 4.5;
      }
      return pow((v + (BT709_ALPHA - 1.0)) / BT709_ALPHA, 1.0 / 0.45);
    }
    case 3u: {
      return safe_pow(v, 2.2);
    }
    case 4u: {
      return safe_pow(v, 2.8);
    }
    case 5u: {
      if (v < 4.0 * S240_BETA) {
        return v / 4.0;
      }
      return pow((v + (S240_ALPHA - 1.0)) / S240_ALPHA, 1.0 / 0.45);
    }
    case 6u: {
      return pq_eotf(v);
    }
    case 7u: {
      if (v <= 0.5) {
        return v * v / 3.0;
      }
      return (exp((v - HLG_C) / HLG_A) + HLG_B) / 12.0;
    }
    default: {
      return v;
    }
  }
}

// Output encodings only: linear, sRGB, BT.709.
fn from_linear(l: f32, tf: u32) -> f32 {
  switch tf {
    case 1u: {
      if (l <= 0.0031308) {
        return l * 12.92;
      }
      return 1.055 * pow(l, 1.0 / 2.4) - 0.055;
    }
    case 2u: {
      if (l < BT709_BETA) {
        return l * 4.5;
      }
      return BT709_ALPHA * pow(l, 0.45) - (BT709_ALPHA - 1.0);
    }
    default: {
      return l;
    }
  }
}
