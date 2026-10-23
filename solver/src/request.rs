use crate::model::WeaponPayload;
use crate::search::Constraint;
use rustc_hash::FxHashMap;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SolveRequestOptions {
    #[serde(default)]
    pub prices: FxHashMap<String, f64>,
    #[serde(rename = "weaponId", default)]
    pub weapon_id: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(rename = "laserEnabled", default)]
    pub laser_enabled: bool,
    #[serde(rename = "bipodDeployed", default)]
    pub bipod_deployed: bool,
    #[serde(rename = "lockedPathKeys", default)]
    pub locked_paths: FxHashMap<String, Option<String>>,
    #[serde(rename = "lockedTuningKeys", default)]
    pub locked_tuning: FxHashMap<String, f64>,
    #[serde(rename = "excludedPartIds", default)]
    pub excluded_part_ids: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<Constraint>,
    #[serde(rename = "maxEvaluations", default = "default_max_nodes")]
    pub max_nodes: u64,
    #[serde(rename = "maxLeaves", default = "default_max_leaves")]
    pub max_leaves: u64,
    #[serde(rename = "maxResults", default = "default_max_results")]
    pub max_results: usize,
    #[serde(rename = "costCap", default)]
    pub cost_cap: Option<i64>,
    #[serde(rename = "firstSolution", default)]
    pub first_solution: bool,
    #[serde(rename = "deadlineMs", default)]
    pub deadline_ms: Option<u64>,
}

fn default_max_nodes() -> u64 {
    5_000_000
}
fn default_max_leaves() -> u64 {
    2_000_000
}
fn default_max_results() -> usize {
    10
}

#[derive(Debug, Clone, Deserialize)]
pub struct SolveRequest {
    #[serde(rename = "weaponPayload", alias = "payload")]
    pub payload: WeaponPayload,
    #[serde(flatten)]
    pub options: SolveRequestOptions,
}

impl std::ops::Deref for SolveRequest {
    type Target = SolveRequestOptions;

    fn deref(&self) -> &Self::Target {
        &self.options
    }
}
