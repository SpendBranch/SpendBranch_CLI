//! The contract's time type.
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use std::fmt;

/// A point in time with millisecond precision, written as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
/// Limited to 1970..9999 so the text is always a valid contract datetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Iso(pub(super) i64);

impl Iso {
    const MAX_MS: i64 = 253_402_300_799_999;
    pub const EPOCH: Iso = Iso(0);

    pub fn from_millis(ms: i64) -> Option<Self> {
        (0..=Self::MAX_MS).contains(&ms).then_some(Self(ms))
    }

    /// RFC 3339 with an offset or `Z` (what the agents write) and a real calendar date.
    /// Anything else is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let ts: jiff::Timestamp = text.parse().ok()?;
        Self::from_millis(ts.as_millisecond())
    }

    pub fn millis(self) -> i64 {
        self.0
    }
}

impl fmt::Display for Iso {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ts = jiff::Timestamp::from_millisecond(self.0).map_err(|_| fmt::Error)?;
        write!(f, "{}", ts.strftime("%Y-%m-%dT%H:%M:%S.%3fZ"))
    }
}

impl<'de> Deserialize<'de> for Iso {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Iso::parse(&text).ok_or_else(|| D::Error::custom("invalid datetime"))
    }
}

impl Serialize for Iso {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}
