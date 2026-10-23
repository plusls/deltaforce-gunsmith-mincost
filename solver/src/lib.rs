pub mod assembly;
pub mod constraint_reduction;
pub mod metrics;
pub mod model;
pub mod search;

use model::{Map, WeaponPayload};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Deserialize;
use search::{Constraint, SolveOptions, SolveResult, Solver};
use std::io::Write;

#[derive(Debug, Clone, Deserialize)]
pub struct SolveRequest {
    #[serde(rename = "weaponPayload", alias = "payload")]
    pub payload: WeaponPayload,
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

pub fn solve_request(req: SolveRequest) -> Result<SolveResult, String> {
    for c in &req.constraints {
        if !search::is_supported_constraint(&c.id) {
            return Err(format!(
                "unsupported constraint {}; supported: {}, {}, {}",
                c.id,
                search::RECOIL_METRICS.join(", "),
                search::PANEL_ATTRS.join(", "),
                crate::metrics::DETAIL_METRICS.join(", ")
            ));
        }
        if c.min.is_none() && c.max.is_none() {
            return Err(format!("constraint {} needs min or max", c.id));
        }
        if let (Some(min), Some(max)) = (c.min, c.max) {
            if min > max + 1e-9 {
                return Err(format!("constraint {} has min > max", c.id));
            }
        }
    }
    let weapon_id = req
        .weapon_id
        .clone()
        .or_else(|| req.payload.weapon_id.clone())
        .or_else(|| req.payload.weapon_ids.first().cloned())
        .ok_or_else(|| "weapon id is missing".to_string())?;
    if !req.payload.parts.contains_key(&weapon_id) {
        return Err(format!("weapon {} not found in payload", weapon_id));
    }
    let mode = req.mode.clone().unwrap_or_else(|| "operations".to_string());
    let prices: FxHashMap<String, i64> = req
        .prices
        .iter()
        .map(|(k, v)| (k.clone(), v.round() as i64))
        .collect();
    let excluded: FxHashSet<String> = req.excluded_part_ids.iter().cloned().collect();
    let options = SolveOptions {
        mode,
        laser_enabled: req.laser_enabled,
        bipod_deployed: req.bipod_deployed,
        locked_paths: req.locked_paths,
        locked_tuning: req.locked_tuning,
        excluded_part_ids: excluded,
        constraints: req.constraints,
        max_nodes: req.max_nodes,
        max_leaves: req.max_leaves,
        max_results: req.max_results,
        fixed_cost_cap: req.cost_cap,
        first_solution: req.first_solution,
        deadline_ms: req.deadline_ms,
    };
    let payload = &req.payload;
    let solver = Solver::new(payload, &weapon_id, &prices, &options);
    Ok(solver.solve())
}

pub fn solve_slices(payload_json: &[u8], request_json: &[u8]) -> Result<Vec<u8>, String> {
    let payload: WeaponPayload =
        serde_json::from_slice(payload_json).map_err(|e| format!("payload: {e}"))?;
    #[derive(Deserialize)]
    struct Wrapper {
        #[serde(default)]
        prices: FxHashMap<String, f64>,
        #[serde(rename = "weaponId", default)]
        weapon_id: Option<String>,
        #[serde(default)]
        mode: Option<String>,
        #[serde(rename = "laserEnabled", default)]
        laser_enabled: bool,
        #[serde(rename = "bipodDeployed", default)]
        bipod_deployed: bool,
        #[serde(rename = "lockedPathKeys", default)]
        locked_paths: FxHashMap<String, Option<String>>,
        #[serde(rename = "lockedTuningKeys", default)]
        locked_tuning: FxHashMap<String, f64>,
        #[serde(rename = "excludedPartIds", default)]
        excluded_part_ids: Vec<String>,
        #[serde(default)]
        constraints: Vec<Constraint>,
        #[serde(rename = "maxEvaluations", default = "default_max_nodes")]
        max_nodes: u64,
        #[serde(rename = "maxLeaves", default = "default_max_leaves")]
        max_leaves: u64,
        #[serde(rename = "maxResults", default = "default_max_results")]
        max_results: usize,
        #[serde(rename = "costCap", default)]
        cost_cap: Option<i64>,
        #[serde(rename = "firstSolution", default)]
        first_solution: bool,
        #[serde(rename = "deadlineMs", default)]
        deadline_ms: Option<u64>,
    }
    let wrap: Wrapper =
        serde_json::from_slice(request_json).map_err(|e| format!("request: {e}"))?;
    let req = SolveRequest {
        payload,
        prices: wrap.prices,
        weapon_id: wrap.weapon_id,
        mode: wrap.mode,
        laser_enabled: wrap.laser_enabled,
        bipod_deployed: wrap.bipod_deployed,
        locked_paths: wrap.locked_paths,
        locked_tuning: wrap.locked_tuning,
        excluded_part_ids: wrap.excluded_part_ids,
        constraints: wrap.constraints,
        max_nodes: wrap.max_nodes,
        max_leaves: wrap.max_leaves,
        max_results: wrap.max_results,
        cost_cap: wrap.cost_cap,
        first_solution: wrap.first_solution,
        deadline_ms: wrap.deadline_ms,
    };
    let result = solve_request(req)?;
    serde_json::to_vec(&result).map_err(|e| e.to_string())
}

pub fn solve_json_str(payload_json: &str, request_json: &str) -> Result<String, String> {
    let bytes = solve_slices(payload_json.as_bytes(), request_json.as_bytes())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use std::alloc::{alloc, dealloc, Layout};

    #[no_mangle]
    pub extern "C" fn dws_alloc(len: usize) -> *mut u8 {
        unsafe {
            let layout = Layout::from_size_align(len.max(1), 8).unwrap();
            alloc(layout)
        }
    }

    #[no_mangle]
    pub extern "C" fn dws_dealloc(ptr: *mut u8, len: usize) {
        unsafe {
            let layout = Layout::from_size_align(len.max(1), 8).unwrap();
            dealloc(ptr, layout);
        }
    }

    #[no_mangle]
    pub extern "C" fn dws_solve(
        payload_ptr: *const u8,
        payload_len: usize,
        request_ptr: *const u8,
        request_len: usize,
    ) -> *mut u8 {
        let payload = unsafe { std::slice::from_raw_parts(payload_ptr, payload_len) };
        let request = unsafe { std::slice::from_raw_parts(request_ptr, request_len) };
        let out = match solve_slices(payload, request) {
            Ok(v) => v,
            Err(e) => serde_json::to_vec(&serde_json::json!({ "error": e })).unwrap(),
        };
        let len = out.len();
        let total = len + 4;
        let ptr = dws_alloc(total);
        if ptr.is_null() {
            return std::ptr::null_mut();
        }
        unsafe {
            let len_bytes = (len as u32).to_le_bytes();
            std::ptr::copy_nonoverlapping(len_bytes.as_ptr(), ptr, 4);
            std::ptr::copy_nonoverlapping(out.as_ptr(), ptr.add(4), len);
        }
        ptr
    }
}

#[allow(dead_code)]
pub fn write_result<W: Write>(w: &mut W, result: &SolveResult) -> std::io::Result<()> {
    serde_json::to_writer(w, result)?;
    Ok(())
}

#[allow(dead_code)]
pub fn empty_map() -> Map<String, String> {
    Map::default()
}
