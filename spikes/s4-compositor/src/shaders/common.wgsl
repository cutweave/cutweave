// Shared colour math, f32. Written independently of src/color.rs (the CPU reference); only the
// numeric constants are common. Prepended to every pass shader.

const TF_LINEAR: u32 = 0u;
const TF_SRGB: u32 = 1u;
const TF_BT709: u32 = 2u;
const TF_BT1886: u32 = 3u;
const TF_PQ: u32 = 4u;

const BT709_ALPHA: f32 = 1.09929682680944;
const BT709_BETA: f32 = 0.018053968510807;

const PQ_M1: f32 = 0.1593017578125;
const PQ_M2: f32 = 78.84375;
const PQ_C1: f32 = 0.8359375;
const PQ_C2: f32 = 18.8515625;
const PQ_C3: f32 = 18.6875;

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
      return safe_pow(v, 2.4);
    }
    case 4u: {
      return pq_eotf(v);
    }
    default: {
      return v;
    }
  }
}

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
    case 3u: {
      return safe_pow(l, 1.0 / 2.4);
    }
    case 4u: {
      return pq_inv_eotf(l);
    }
    default: {
      return l;
    }
  }
}

fn to_linear3(v: vec3<f32>, tf: u32) -> vec3<f32> {
  return vec3<f32>(to_linear(v.r, tf), to_linear(v.g, tf), to_linear(v.b, tf));
}

fn from_linear3(v: vec3<f32>, tf: u32) -> vec3<f32> {
  return vec3<f32>(from_linear(v.r, tf), from_linear(v.g, tf), from_linear(v.b, tf));
}
