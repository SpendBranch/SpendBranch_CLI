//! Deserialization helpers that keep memory flat: only allowlisted fields are kept, and
//! everything else (conversation text above all) is skipped without being stored.
use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, MapAccess, SeqAccess, Visitor, value::MapAccessDeserializer},
};
use std::{fmt, marker::PhantomData};

/// Longest string kept. Every field we read is far shorter; anything longer is invalid
/// for its contract pattern anyway, so it is treated as "present but unusable".
const MAX_KEPT: usize = 4096;

/// One JSON value of a field we read. `Missing` is an absent key, `Null` a literal null.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Leaf {
    #[default]
    Missing,
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Other,
}

impl Leaf {
    pub fn str(&self) -> Option<&str> {
        match self {
            Leaf::Str(s) => Some(s),
            _ => None,
        }
    }

    /// `Number.isSafeInteger` in the reference: an integer value within +-(2^53 - 1).
    pub fn safe_int(&self) -> Option<i64> {
        match self {
            Leaf::Num(n) if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0 => {
                Some(*n as i64)
            }
            _ => None,
        }
    }

    pub fn is_present(&self) -> bool {
        !matches!(self, Leaf::Missing | Leaf::Null)
    }
}

struct LeafVisitor;

impl<'de> Visitor<'de> for LeafVisitor {
    type Value = Leaf;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any JSON value")
    }
    fn visit_bool<E>(self, v: bool) -> Result<Leaf, E> {
        Ok(Leaf::Bool(v))
    }
    fn visit_i64<E>(self, v: i64) -> Result<Leaf, E> {
        Ok(Leaf::Num(v as f64))
    }
    fn visit_u64<E>(self, v: u64) -> Result<Leaf, E> {
        Ok(Leaf::Num(v as f64))
    }
    fn visit_f64<E>(self, v: f64) -> Result<Leaf, E> {
        Ok(Leaf::Num(v))
    }
    fn visit_str<E>(self, v: &str) -> Result<Leaf, E> {
        Ok(if v.len() <= MAX_KEPT {
            Leaf::Str(v.to_owned())
        } else {
            Leaf::Other
        })
    }
    fn visit_unit<E>(self) -> Result<Leaf, E> {
        Ok(Leaf::Null)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Leaf, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Leaf::Other)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Leaf, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Leaf::Other)
    }
}

impl<'de> Deserialize<'de> for Leaf {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(LeafVisitor)
    }
}

/// A field that should hold an object. Anything else (string, array, null...) becomes
/// `Obj(None)` instead of an error, like the reference's `record()`.
#[derive(Debug)]
pub struct Obj<T>(pub Option<T>);

impl<T> Default for Obj<T> {
    fn default() -> Self {
        Obj(None)
    }
}

struct ObjVisitor<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjVisitor<T> {
    type Value = Obj<T>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any JSON value")
    }
    fn visit_bool<E>(self, _: bool) -> Result<Obj<T>, E> {
        Ok(Obj(None))
    }
    fn visit_i64<E>(self, _: i64) -> Result<Obj<T>, E> {
        Ok(Obj(None))
    }
    fn visit_u64<E>(self, _: u64) -> Result<Obj<T>, E> {
        Ok(Obj(None))
    }
    fn visit_f64<E>(self, _: f64) -> Result<Obj<T>, E> {
        Ok(Obj(None))
    }
    fn visit_str<E>(self, _: &str) -> Result<Obj<T>, E> {
        Ok(Obj(None))
    }
    fn visit_unit<E>(self) -> Result<Obj<T>, E> {
        Ok(Obj(None))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Obj<T>, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Obj(None))
    }
    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Obj<T>, A::Error> {
        Ok(Obj(Some(T::deserialize(MapAccessDeserializer::new(map))?)))
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Obj<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(ObjVisitor(PhantomData))
    }
}

/// A usage counter as the reference reads it: absent or null is unknown, a safe
/// non-negative integer up to the contract ceiling is a value, anything else is bad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Counter {
    Unknown,
    Value(u64),
    Bad,
}

pub fn counter(leaf: &Leaf) -> Counter {
    match leaf {
        Leaf::Missing | Leaf::Null => Counter::Unknown,
        other => match other.safe_int() {
            Some(n) if (0..=crate::contract::MAX_TOKENS_PER_COUNTER as i64).contains(&n) => {
                Counter::Value(n as u64)
            }
            _ => Counter::Bad,
        },
    }
}
