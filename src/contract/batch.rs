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

impl IngestBatch {
    /// `None` when the batch breaks a limit (500 observations, 50 coverage reports), is
    /// empty, or the id is not a standard UUID.
    pub fn new(
        installation_id: uuid::Uuid,
        observations: Vec<UsageObservation>,
        coverage: Vec<CoverageReport>,
    ) -> Option<Self> {
        let standard = installation_id.is_nil()
            || installation_id.is_max()
            || ((1..=8).contains(&installation_id.get_version_num())
                && installation_id.get_variant() == uuid::Variant::RFC4122);
        (standard
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
