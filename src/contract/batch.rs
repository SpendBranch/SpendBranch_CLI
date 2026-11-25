//! The batch that goes to the server.
use super::*;
use serde::{Serialize, Serializer};

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

impl IngestBatch {
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
