//! The batch that goes to the server.
use super::*;
use serde::{Serialize, Serializer};
use std::{borrow::Borrow, marker::PhantomData};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestBatch {
    contract_version: &'static str,
    installation_id: InstallationId,
    observations: Vec<UsageObservation>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    coverage: Vec<CoverageReport>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct InstallationId(uuid::Uuid);

impl Serialize for InstallationId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&self.0.hyphenated())
    }
}

/// Whether the id can go in a batch: nil, max, or a version 1 to 8 RFC 4122 UUID.
pub fn standard_installation_id(id: uuid::Uuid) -> bool {
    id.is_nil()
        || id.is_max()
        || ((1..=8).contains(&id.get_version_num()) && id.get_variant() == uuid::Variant::RFC4122)
}

/// The parts a batch is made of.
pub trait BatchPart: Serialize + private::Sealed {}
impl BatchPart for UsageObservation {}
impl BatchPart for CoverageReport {}
mod private {
    pub trait Sealed {}
    impl Sealed for super::UsageObservation {}
    impl Sealed for super::CoverageReport {}
}

/// An observation or a coverage report already turned into its JSON, from the typed value and
/// nothing else: a batch is put together from these without serializing each part again.
#[derive(Debug, PartialEq, Eq)]
pub struct Encoded<T: BatchPart>(String, PhantomData<T>);

impl<T: BatchPart> Clone for Encoded<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<T: BatchPart> Encoded<T> {
    pub fn new(part: &T) -> Self {
        // The parts hold only strings, numbers and string-keyed objects: serializing them
        // cannot fail.
        Self(serde_json::to_string(part).unwrap_or_default(), PhantomData)
    }

    /// The JSON text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl IngestBatch {
    /// The JSON of the batch these parts make, the same bytes serializing an `IngestBatch` of
    /// them writes. `None` on the limits of `new`.
    pub fn encode(
        installation_id: uuid::Uuid,
        observations: &[impl Borrow<Encoded<UsageObservation>>],
        coverage: &[impl Borrow<Encoded<CoverageReport>>],
    ) -> Option<Vec<u8>> {
        let mut out =
            Vec::with_capacity(Self::encoded_len(installation_id, observations, coverage)?);
        out.extend_from_slice(br#"{"contractVersion":""#);
        out.extend_from_slice(CONTRACT_VERSION.as_bytes());
        out.extend_from_slice(br#"","installationId":""#);
        out.extend_from_slice(installation_id.hyphenated().to_string().as_bytes());
        out.extend_from_slice(br#"","observations":"#);
        list(&mut out, observations);
        if !coverage.is_empty() {
            out.extend_from_slice(br#","coverage":"#);
            list(&mut out, coverage);
        }
        out.push(b'}');
        Some(out)
    }

    /// The length of what `encode` would write, without writing it.
    pub fn encoded_len(
        installation_id: uuid::Uuid,
        observations: &[impl Borrow<Encoded<UsageObservation>>],
        coverage: &[impl Borrow<Encoded<CoverageReport>>],
    ) -> Option<usize> {
        let fits = standard_installation_id(installation_id)
            && observations.len() <= MAX_BATCH_OBSERVATIONS
            && coverage.len() <= MAX_BATCH_COVERAGE
            && observations.len() + coverage.len() > 0;
        if !fits {
            return None;
        }
        let head = r#"{"contractVersion":"","installationId":"","observations":}"#.len()
            + CONTRACT_VERSION.len()
            + uuid::fmt::Hyphenated::LENGTH;
        let coverage_len = if coverage.is_empty() {
            0
        } else {
            r#","coverage":"#.len() + list_len(coverage)
        };
        Some(head + list_len(observations) + coverage_len)
    }

    /// `None` when the batch breaks a limit (500 observations, 50 coverage reports), is
    /// empty, or the id is not a standard UUID.
    pub fn new(
        installation_id: uuid::Uuid,
        observations: Vec<UsageObservation>,
        coverage: Vec<CoverageReport>,
    ) -> Option<Self> {
        (standard_installation_id(installation_id)
            && observations.len() <= MAX_BATCH_OBSERVATIONS
            && coverage.len() <= MAX_BATCH_COVERAGE
            && observations.len() + coverage.len() > 0)
            .then_some(Self {
                contract_version: CONTRACT_VERSION,
                installation_id: InstallationId(installation_id),
                observations,
                coverage,
            })
    }
}

fn list<T: BatchPart>(out: &mut Vec<u8>, parts: &[impl Borrow<Encoded<T>>]) {
    out.push(b'[');
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.extend_from_slice(part.borrow().0.as_bytes());
    }
    out.push(b']');
}

fn list_len<T: BatchPart>(parts: &[impl Borrow<Encoded<T>>]) -> usize {
    2 + parts.iter().map(|p| p.borrow().0.len()).sum::<usize>() + parts.len().saturating_sub(1)
}
