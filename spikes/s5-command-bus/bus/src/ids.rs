//! Prefixed string IDs (arch §4.2: `med_…`, `seq_…`, `trk_…`, `itm_…`) and id generation,
//! including the deterministic test mode (arch §5.1, `--deterministic-ids <seed>`).
//!
//! IDs are UUIDv7-shaped (`<prefix>_<32 hex>`). Deserialization checks the prefix, so passing a
//! media id where an item id is expected is an `Invalid` error at the right JSON path.

use std::{borrow::Cow, fmt};

use schemars::{JsonSchema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// Implemented by every ID newtype.
pub trait EntityId: Clone + Ord + fmt::Display {
    /// Type prefix without the underscore.
    const PREFIX: &'static str;
    /// Wraps an already-formatted id. Callers guarantee the format.
    fn from_raw(raw: String) -> Self;
}

fn valid_suffix(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

macro_rules! entity_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal, $what:literal) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, specta::Type)]
        pub struct $name(String);

        impl $name {
            /// Parses and validates `<prefix>_<32 lowercase hex>`.
            pub fn parse(s: &str) -> Result<Self, String> {
                match s.strip_prefix(concat!($prefix, "_")) {
                    Some(rest) if valid_suffix(rest) => Ok(Self(s.to_owned())),
                    _ => Err(format!(
                        concat!("expected ", $what, " id (`", $prefix, "_` + 32 hex chars), got `{}`"),
                        s
                    )),
                }
            }

            /// The id string.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl EntityId for $name {
            const PREFIX: &'static str = $prefix;
            fn from_raw(raw: String) -> Self {
                Self(raw)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = Cow::<'de, str>::deserialize(d)?;
                Self::parse(&s).map_err(de::Error::custom)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }
            fn inline_schema() -> bool {
                true
            }
            fn json_schema(_: &mut SchemaGenerator) -> schemars::Schema {
                json_schema!({
                    "type": "string",
                    "pattern": concat!("^", $prefix, "_[0-9a-f]{32}$"),
                    "description": concat!("A", " ", $what, " id.")
                })
            }
        }
    };
}

entity_id!(
    /// A media asset id (`med_…`).
    MediaId,
    "med",
    "media"
);
entity_id!(
    /// A sequence id (`seq_…`).
    SequenceId,
    "seq",
    "sequence"
);
entity_id!(
    /// A track id (`trk_…`).
    TrackId,
    "trk",
    "track"
);
entity_id!(
    /// A timeline item id (`itm_…`).
    ItemId,
    "itm",
    "item"
);

/// How new ids are minted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdMode {
    /// Seeded, reproducible UUIDv7-shaped ids (parity and golden tests byte-compare documents).
    Deterministic {
        /// Seed.
        seed: u64,
    },
    /// Real UUIDv7 (needs OS entropy, so unavailable on wasm32-unknown-unknown).
    #[cfg(not(target_arch = "wasm32"))]
    Random,
}

/// Mints ids. Owned by the engine, not the document: undo never rewinds it, so an id is never
/// reused for a different entity.
#[derive(Clone, Debug)]
pub struct IdGen {
    mode: IdMode,
    counter: u64,
}

impl IdGen {
    /// New generator.
    #[must_use]
    pub fn new(mode: IdMode) -> IdGen {
        IdGen { mode, counter: 0 }
    }

    /// Next id of type `I`.
    pub fn mint<I: EntityId>(&mut self) -> I {
        self.counter += 1;
        let hex = match self.mode {
            IdMode::Deterministic { seed } => {
                let r = splitmix64(seed ^ self.counter.wrapping_mul(0x9E37_79B9_7F4A_7C15));
                // 48-bit "timestamp" = counter keeps ids sortable by creation, like real UUIDv7.
                uuid7_hex(self.counter, (r >> 52) as u16, r)
            }
            #[cfg(not(target_arch = "wasm32"))]
            IdMode::Random => uuid::Uuid::now_v7().simple().to_string(),
        };
        I::from_raw(format!("{}_{hex}", I::PREFIX))
    }
}

fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Formats UUIDv7 bits: 48-bit ms timestamp, version 7, 12 bits rand_a, variant 0b10, 62 bits rand_b.
fn uuid7_hex(ts_ms: u64, rand_a: u16, rand_b: u64) -> String {
    let hi = ((ts_ms & 0xFFFF_FFFF_FFFF) << 16) | 0x7000 | u64::from(rand_a & 0x0FFF);
    let lo = (rand_b & 0x3FFF_FFFF_FFFF_FFFF) | 0x8000_0000_0000_0000;
    format!("{hi:016x}{lo:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_ids_are_reproducible_and_well_formed() {
        let mut a = IdGen::new(IdMode::Deterministic { seed: 7 });
        let mut b = IdGen::new(IdMode::Deterministic { seed: 7 });
        let mut c = IdGen::new(IdMode::Deterministic { seed: 8 });
        let ia: ItemId = a.mint();
        let ib: ItemId = b.mint();
        let ic: ItemId = c.mint();
        assert_eq!(ia, ib);
        assert_ne!(ia, ic);
        assert!(ItemId::parse(ia.as_str()).is_ok());
        assert_eq!(&ia.as_str()[16..17], "7", "version nibble");
        let next: ItemId = a.mint();
        assert!(next > ia, "sortable by creation");
    }

    #[test]
    fn prefix_is_checked_on_deserialize() {
        let mut g = IdGen::new(IdMode::Deterministic { seed: 1 });
        let m: MediaId = g.mint();
        let as_item = serde_json::from_value::<ItemId>(serde_json::json!(m.as_str()));
        assert!(as_item.is_err());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn random_ids_parse() {
        let mut g = IdGen::new(IdMode::Random);
        let t: TrackId = g.mint();
        assert!(TrackId::parse(t.as_str()).is_ok(), "{t}");
    }
}
