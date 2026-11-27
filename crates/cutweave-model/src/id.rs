// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed entity IDs (arch §4.2) and ID generation, including the deterministic test mode
//! (`--deterministic-ids <seed>`, arch §5.1, ADR-0015).
//!
//! An ID is a UUIDv7 written as `<prefix>_<32 lowercase hex digits>`, e.g.
//! `itm_0190f5c2a1b27c3d8e4f5a6b7c8d9e0f`. The prefix names the entity kind, so passing a media
//! ID where an item ID is expected fails to parse. IDs are `Copy` (a `u128` in memory) and
//! order by creation time, because a UUIDv7 starts with its millisecond timestamp.
//!
//! IDs are minted through an [`IdGen`] owned by the engine (never by the document, so undo
//! never rewinds it and an ID is never reused): [`Uuid7Gen`] in production, [`SeededGen`] for
//! byte-reproducible documents in tests.

use std::{borrow::Cow, fmt, hash::Hash, str::FromStr};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// The kind of entity an ID names.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
    specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// A `MediaAsset` (`med_`).
    Media,
    /// A `Sequence` (`seq_`).
    Sequence,
    /// A `Track` (`trk_`).
    Track,
    /// A timeline `Item` (`itm_`).
    Item,
    /// A linked group of items that edit together (`grp_`).
    Group,
    /// A timeline or item marker (`mrk_`).
    Marker,
    /// A folder of the media bin (`fld_`, ADR-0020).
    Folder,
    /// A collection of media (`col_`, ADR-0036).
    Collection,
}

impl EntityKind {
    /// The ID prefix without the underscore.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            EntityKind::Media => "med",
            EntityKind::Sequence => "seq",
            EntityKind::Track => "trk",
            EntityKind::Item => "itm",
            EntityKind::Group => "grp",
            EntityKind::Marker => "mrk",
            EntityKind::Folder => "fld",
            EntityKind::Collection => "col",
        }
    }

    /// Lowercase name for messages (`"media"`, `"item"`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            EntityKind::Media => "media",
            EntityKind::Sequence => "sequence",
            EntityKind::Track => "track",
            EntityKind::Item => "item",
            EntityKind::Group => "group",
            EntityKind::Marker => "marker",
            EntityKind::Folder => "folder",
            EntityKind::Collection => "collection",
        }
    }
}

impl fmt::Display for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A string that is not a valid ID of the expected kind.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("expected a {kind} id (`{prefix}_` followed by 32 lowercase hex digits), got `{input}`", prefix = kind.prefix())]
pub struct IdError {
    /// The expected kind.
    pub kind: EntityKind,
    /// The rejected input.
    pub input: String,
}

mod sealed {
    pub trait Sealed {}
}

/// Implemented by every typed ID ([`MediaId`], [`SequenceId`], [`TrackId`], [`ItemId`],
/// [`GroupId`], [`MarkerId`], [`FolderId`], [`CollectionId`]). Sealed: the set of ID kinds is part of the project
/// format.
pub trait EntityId:
    Copy
    + Eq
    + Ord
    + Hash
    + fmt::Debug
    + fmt::Display
    + FromStr<Err = IdError>
    + Send
    + Sync
    + 'static
    + sealed::Sealed
{
    /// The kind of entity.
    const KIND: EntityKind;

    /// Wraps a raw 128-bit UUID value.
    fn from_u128(raw: u128) -> Self;

    /// The raw 128-bit UUID value.
    fn as_u128(self) -> u128;

    /// A fresh ID from `generator`.
    fn new_with<G: IdGen + ?Sized>(generator: &mut G) -> Self {
        Self::from_u128(generator.next_u128())
    }
}

fn parse_id(kind: EntityKind, s: &str) -> Result<u128, IdError> {
    let err = || IdError {
        kind,
        input: s.to_owned(),
    };
    let hex = s
        .strip_prefix(kind.prefix())
        .and_then(|rest| rest.strip_prefix('_'))
        .ok_or_else(err)?;
    if hex.len() != 32 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(err());
    }
    u128::from_str_radix(hex, 16).map_err(|_| err())
}

macro_rules! entity_id {
    ($(#[$doc:meta])* $name:ident, $kind:ident, $pattern:literal) => {
        $(#[$doc])*
        ///
        /// Serialized as a string, exported to TypeScript as `string`.
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, specta::Type)]
        #[specta(type = String)]
        pub struct $name(u128);

        impl $name {
            /// A fresh time-ordered UUIDv7 ID from the process-wide [`Uuid7Gen`]. IDs created
            /// by this process sort by creation. Not available on `wasm32-unknown-unknown`,
            /// where the engine supplies an [`IdGen`].
            #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
            #[must_use]
            #[allow(clippy::new_without_default)] // a fresh ID is not a "default" value
            pub fn new() -> Self {
                <Self as EntityId>::new_with(&mut Uuid7Gen)
            }

            /// Wraps a raw 128-bit UUID value.
            #[must_use]
            pub const fn from_u128(raw: u128) -> Self {
                Self(raw)
            }

            /// The raw 128-bit UUID value.
            #[must_use]
            pub const fn as_u128(self) -> u128 {
                self.0
            }
        }

        impl sealed::Sealed for $name {}

        impl EntityId for $name {
            const KIND: EntityKind = EntityKind::$kind;

            fn from_u128(raw: u128) -> Self {
                Self(raw)
            }

            fn as_u128(self) -> u128 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}_{:032x}", EntityKind::$kind.prefix(), self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self, f)
            }
        }

        impl FromStr for $name {
            type Err = IdError;
            fn from_str(s: &str) -> Result<Self, IdError> {
                parse_id(EntityKind::$kind, s).map(Self)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = Cow::<'de, str>::deserialize(d)?;
                s.parse().map_err(de::Error::custom)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn inline_schema() -> bool {
                true
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "type": "string",
                    "pattern": $pattern,
                    "description": concat!("A ", stringify!($kind), " id: `", $pattern, "`.")
                })
            }
        }
    };
}

entity_id!(
    /// The ID of a `MediaAsset`: `med_…`.
    MediaId,
    Media,
    "^med_[0-9a-f]{32}$"
);
entity_id!(
    /// The ID of a `Sequence`: `seq_…`.
    SequenceId,
    Sequence,
    "^seq_[0-9a-f]{32}$"
);
entity_id!(
    /// The ID of a `Track`: `trk_…`.
    TrackId,
    Track,
    "^trk_[0-9a-f]{32}$"
);
entity_id!(
    /// The ID of a timeline `Item`: `itm_…`.
    ItemId,
    Item,
    "^itm_[0-9a-f]{32}$"
);
entity_id!(
    /// The ID of a linked group (items that move and delete together, e.g. the video and audio
    /// of one imported file): `grp_…`.
    GroupId,
    Group,
    "^grp_[0-9a-f]{32}$"
);
entity_id!(
    /// The ID of a marker on a sequence or an item: `mrk_…`.
    MarkerId,
    Marker,
    "^mrk_[0-9a-f]{32}$"
);
entity_id!(
    /// The ID of a folder of the media bin (ADR-0020): `fld_…`.
    FolderId,
    Folder,
    "^fld_[0-9a-f]{32}$"
);
entity_id!(
    /// The ID of a collection of media (ADR-0036): `col_…`.
    CollectionId,
    Collection,
    "^col_[0-9a-f]{32}$"
);

/// A source of fresh 128-bit UUIDv7(-shaped) values for new IDs.
///
/// All ID creation in commands goes through the engine's injected generator, so the
/// deterministic test mode covers every new entity.
pub trait IdGen {
    /// The next value. Successive values from one generator increase.
    fn next_u128(&mut self) -> u128;

    /// A fresh typed ID.
    fn mint<I: EntityId>(&mut self) -> I
    where
        Self: Sized,
    {
        I::new_with(self)
    }
}

impl<G: IdGen + ?Sized> IdGen for &mut G {
    fn next_u128(&mut self) -> u128 {
        (**self).next_u128()
    }
}

impl<G: IdGen + ?Sized> IdGen for Box<G> {
    fn next_u128(&mut self) -> u128 {
        (**self).next_u128()
    }
}

/// Real UUIDv7 values from the system clock and OS entropy (the default generator).
///
/// Values created in one process are strictly ordered by creation, also within a millisecond.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[derive(Clone, Copy, Debug, Default)]
pub struct Uuid7Gen;

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl IdGen for Uuid7Gen {
    fn next_u128(&mut self) -> u128 {
        uuid::Uuid::now_v7().as_u128()
    }
}

/// Deterministic UUIDv7-shaped values for `--deterministic-ids <seed>` (WP-1B.08).
///
/// The 48-bit timestamp field holds a counter starting at 1 (not the wall clock), so IDs sort
/// by creation exactly like real UUIDv7s; the 74 random bits come from SplitMix64 over
/// `(seed, counter)`. The same seed always yields the same sequence, on every platform. Never
/// use it in production: IDs from two generators with the same seed collide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeededGen {
    seed: u64,
    counter: u64,
}

impl SeededGen {
    /// A generator for `seed`, positioned at its first value.
    #[must_use]
    pub const fn new(seed: u64) -> SeededGen {
        SeededGen { seed, counter: 0 }
    }

    /// The seed.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// How many values were issued so far.
    #[must_use]
    pub const fn issued(&self) -> u64 {
        self.counter
    }
}

const GOLDEN_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(GOLDEN_GAMMA);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Packs UUIDv7 fields: 48-bit timestamp, version 7, 12 bits `rand_a`, variant `0b10`, 62 bits
/// `rand_b`.
fn uuid7(timestamp: u64, rand_a: u64, rand_b: u64) -> u128 {
    let hi = ((timestamp & 0xFFFF_FFFF_FFFF) << 16) | 0x7000 | (rand_a & 0x0FFF);
    let lo = (rand_b & 0x3FFF_FFFF_FFFF_FFFF) | 0x8000_0000_0000_0000;
    (u128::from(hi) << 64) | u128::from(lo)
}

impl SeededGen {
    /// The value a generator for `seed` issues as its `counter`-th (1-based) ID.
    #[must_use]
    pub fn value_at(seed: u64, counter: u64) -> u128 {
        let base = seed ^ counter.wrapping_mul(GOLDEN_GAMMA);
        let a = splitmix64(base);
        let b = splitmix64(a ^ seed);
        uuid7(counter, a >> 52, b)
    }

    /// Which position of the stream for `seed` produced `raw`, or `None` if `raw` is not one
    /// of its values (an ID from another seed, or a real UUIDv7). Used to resume a stream past
    /// the IDs a saved document already holds (arch §4.4, WP-1B.08).
    #[must_use]
    pub fn counter_of(seed: u64, raw: u128) -> Option<u64> {
        let counter = u64::try_from(raw >> 80).ok()?;
        (counter > 0 && SeededGen::value_at(seed, counter) == raw).then_some(counter)
    }
}

impl IdGen for SeededGen {
    fn next_u128(&mut self) -> u128 {
        // 2^48 values per generator; far beyond any test script.
        self.counter += 1;
        SeededGen::value_at(self.seed, self.counter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_parse_round_trip() {
        let id = ItemId::from_u128(0x0190_f5c2_a1b2_7c3d_8e4f_5a6b_7c8d_9e0f);
        let text = id.to_string();
        assert_eq!(text, "itm_0190f5c2a1b27c3d8e4f5a6b7c8d9e0f");
        assert_eq!(text.parse::<ItemId>(), Ok(id));
        assert_eq!(format!("{id:?}"), text);
        assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{text}\""));
        assert_eq!(
            serde_json::from_str::<ItemId>(&format!("\"{text}\"")).unwrap(),
            id
        );
    }

    #[test]
    fn parsing_rejects_wrong_prefix_and_shape() {
        let hex = "0190f5c2a1b27c3d8e4f5a6b7c8d9e0f";
        assert!(format!("med_{hex}").parse::<ItemId>().is_err());
        assert!(format!("itm{hex}").parse::<ItemId>().is_err());
        assert!(
            format!("itm_{}", hex.to_uppercase())
                .parse::<ItemId>()
                .is_err()
        );
        assert!(format!("itm_{}", &hex[1..]).parse::<ItemId>().is_err());
        assert!(format!("itm_{hex}0").parse::<ItemId>().is_err());
        assert!(format!("itm_+{}", &hex[1..]).parse::<ItemId>().is_err());
        assert!(format!(" itm_{hex}").parse::<ItemId>().is_err());
        let err = format!("med_{hex}").parse::<TrackId>().unwrap_err();
        assert_eq!(err.kind, EntityKind::Track);
        assert!(err.to_string().contains("trk_"));
        assert!(serde_json::from_str::<MediaId>(&format!("\"seq_{hex}\"")).is_err());
        assert!(serde_json::from_str::<MediaId>("42").is_err());
    }

    #[test]
    fn every_kind_has_a_distinct_prefix() {
        let kinds = [
            EntityKind::Media,
            EntityKind::Sequence,
            EntityKind::Track,
            EntityKind::Item,
            EntityKind::Group,
            EntityKind::Marker,
            EntityKind::Folder,
            EntityKind::Collection,
        ];
        let prefixes: std::collections::BTreeSet<_> = kinds.iter().map(|k| k.prefix()).collect();
        assert_eq!(prefixes.len(), kinds.len());
        assert_eq!(MediaId::KIND.prefix(), "med");
        assert_eq!(
            FolderId::from_u128(2).to_string(),
            "fld_00000000000000000000000000000002"
        );
        assert_eq!(
            GroupId::from_u128(1).to_string(),
            "grp_00000000000000000000000000000001"
        );
    }

    #[test]
    fn seeded_ids_are_reproducible_well_formed_and_ordered() {
        let mut a = SeededGen::new(42);
        let mut b = SeededGen::new(42);
        let mut c = SeededGen::new(43);
        let ia: Vec<ItemId> = (0..100).map(|_| a.mint()).collect();
        let ib: Vec<ItemId> = (0..100).map(|_| b.mint()).collect();
        let ic: Vec<ItemId> = (0..100).map(|_| c.mint()).collect();
        assert_eq!(ia, ib);
        assert_ne!(ia, ic);
        assert!(ia.windows(2).all(|w| w[0] < w[1]));
        for id in &ia {
            let text = id.to_string();
            assert_eq!(&text[16..17], "7", "version nibble of {text}");
            assert!(
                matches!(&text[20..21], "8" | "9" | "a" | "b"),
                "variant of {text}"
            );
        }
        assert_eq!(a.issued(), 100);
        assert_eq!(a.seed(), 42);
    }

    #[test]
    fn counter_of_recognizes_only_the_streams_own_values() {
        let mut g = SeededGen::new(7);
        let ids: Vec<u128> = (0..50).map(|_| g.next_u128()).collect();
        for (i, raw) in ids.iter().enumerate() {
            assert_eq!(SeededGen::counter_of(7, *raw), Some(i as u64 + 1));
            assert_eq!(SeededGen::counter_of(8, *raw), None);
        }
        assert_eq!(SeededGen::counter_of(7, 0), None);
        assert_eq!(SeededGen::counter_of(7, Uuid7Gen.next_u128()), None);
    }

    #[test]
    fn seeded_ids_are_stable_across_releases() {
        // Golden values: changing the generator changes every deterministic test document.
        let mut g = SeededGen::new(42);
        let first: MediaId = g.mint();
        let second: TrackId = g.mint();
        assert_eq!(first.to_string(), "med_000000000001728ea8c5c5607e51aeb3");
        assert_eq!(second.to_string(), "trk_00000000000275fdb7ee40b8a0cac642");
    }

    #[test]
    fn generators_work_through_references_and_boxes() {
        fn mint_one<G: IdGen>(mut generator: G) -> ItemId {
            generator.mint()
        }
        let mut g = SeededGen::new(1);
        let by_ref = mint_one(&mut g);
        let mut boxed: Box<dyn IdGen> = Box::new(SeededGen::new(1));
        let by_box = ItemId::new_with(&mut boxed);
        assert_eq!(by_ref, by_box);
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    #[test]
    fn real_ids_are_v7_and_ordered() {
        let ids: Vec<SequenceId> = (0..1000).map(|_| SequenceId::new()).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        let text = ids[0].to_string();
        assert_eq!(&text[16..17], "7");
        assert_eq!(text.parse::<SequenceId>(), Ok(ids[0]));
    }
}
