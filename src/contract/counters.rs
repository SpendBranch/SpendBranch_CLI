//! Token counters: unknown is never zero.
use super::*;
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

/// A token counter. `None` means unknown and is sent as `null`: unknown is never zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Count(Option<u64>);

impl<'de> Deserialize<'de> for Count {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Option::<u64>::deserialize(d)? {
            None => Ok(Count::UNKNOWN),
            Some(n) => Count::new(n).ok_or_else(|| D::Error::custom("counter above the ceiling")),
        }
    }
}

/// A counter that is present even when it is `null` (unknown): present and unknown differ
/// from absent only in the text, and we keep the text as it was written.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Count>, D::Error> {
    Count::deserialize(d).map(Some)
}

impl Count {
    pub const UNKNOWN: Count = Count(None);
    /// `None` above 1e12, the contract's ceiling.
    pub fn new(value: u64) -> Option<Self> {
        (value <= MAX_TOKENS_PER_COUNTER).then_some(Self(Some(value)))
    }
    pub fn get(self) -> Option<u64> {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageCounters {
    pub input_tokens: Count,
    pub output_tokens: Count,
    pub cache_read_tokens: Count,
    pub cache_write_tokens: Count,
    pub reasoning_tokens: Count,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub cache_write_1h_tokens: Option<Count>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub web_search_requests: Option<Count>,
}
