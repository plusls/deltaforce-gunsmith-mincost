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

const STALE_LOCKED_SLOT: &str = "锁定槽位已失效，请解除锁定后重试";
const STALE_LOCKED_TUNING: &str = "锁定精校已失效，请解除锁定后重试";
const LOCK_CONFLICT: &str = "当前配装与锁定条件不一致";

/// Splits a locked tuning key into its `slotPath\0partId\0tuneId` fields and
/// applies the site's implicit slot lock: a locked tuning value only exists
/// for a part installed in the current build, so its slot is locked to that
/// part. Returns the parsed triples for validation against the assembly.
fn expand_locked_tuning(
    locked_paths: &mut FxHashMap<String, Option<String>>,
    locked_tuning: &FxHashMap<String, f64>,
) -> Result<Vec<(String, String, String)>, String> {
    let mut parsed = Vec::with_capacity(locked_tuning.len());
    for key in locked_tuning.keys() {
        let mut fields = key.split('\0');
        let (path, part_id, tune_id) =
            match (fields.next(), fields.next(), fields.next(), fields.next()) {
                (Some(path), Some(part_id), Some(tune_id), None)
                    if !path.is_empty() && !part_id.is_empty() && !tune_id.is_empty() =>
                {
                    (path, part_id, tune_id)
                }
                _ => return Err(format!("{STALE_LOCKED_TUNING}: {key}")),
            };
        match locked_paths.get(path) {
            Some(Some(locked)) if locked == part_id => {}
            Some(_) => return Err(format!("{STALE_LOCKED_TUNING}: {path}")),
            None => {
                locked_paths.insert(path.to_string(), Some(part_id.to_string()));
            }
        }
        parsed.push((
            path.to_string(),
            part_id.to_string(),
            tune_id.to_string(),
        ));
    }
    Ok(parsed)
}

/// Structural check that a locked path names slots and host parts that exist
/// in the payload. Socket claims from other locks can hide a slot from the
/// resolver, so existence is checked on the slot graph instead of on the
/// resolved assembly; the search still reports such contradictions as a build
/// with no solution.
fn locked_path_exists(payload: &WeaponPayload, weapon_id: &str, path: &str) -> bool {
    let root = format!("weapon:{}", assembly::enc(weapon_id));
    let mut segments = path.split('/');
    if segments.next() != Some(root.as_str()) {
        return false;
    }
    let mut host = weapon_id.to_string();
    loop {
        let Some(segment) = segments.next() else {
            return false;
        };
        let Some(rest) = segment.strip_prefix("slot:") else {
            return false;
        };
        let Some((socket, _name)) = rest.split_once(':') else {
            return false;
        };
        let Some(slots) = payload.slots_by_host.get(&host) else {
            return false;
        };
        if !slots.iter().any(|slot| assembly::enc(&slot.id) == socket) {
            return false;
        }
        let Some(part_segment) = segments.next() else {
            return true;
        };
        let Some(encoded_part) = part_segment.strip_prefix("part:") else {
            return false;
        };
        let Some((part_id, _)) = payload
            .parts
            .iter()
            .find(|(id, _)| assembly::enc(id) == encoded_part)
        else {
            return false;
        };
        host = part_id.clone();
    }
}

/// Request-level lock validation matching the site optimizer. The site rejects
/// a lock whose slot is no longer resolvable, a locked part that is also
/// excluded, and a locked tuning that does not belong to the installed part,
/// instead of searching with a silently different build.
fn validate_locks(
    payload: &WeaponPayload,
    weapon_id: &str,
    locked_paths: &FxHashMap<String, Option<String>>,
    excluded: &FxHashSet<String>,
    locked_tuning: &[(String, String, String)],
) -> Result<(), String> {
    for (path, expected) in locked_paths {
        if !locked_path_exists(payload, weapon_id, path) {
            return Err(format!("{STALE_LOCKED_SLOT}: {path}"));
        }
        let Some(part_id) = expected else {
            continue;
        };
        if excluded.contains(part_id) {
            return Err(format!(
                "{LOCK_CONFLICT}: locked part {part_id} at {path} is excluded"
            ));
        }
        if !payload.parts.contains_key(part_id) {
            return Err(format!(
                "{LOCK_CONFLICT}: locked part {part_id} at {path} is not in the payload"
            ));
        }
    }
    let selections: Map<String, Option<String>> = locked_paths
        .iter()
        .map(|(path, part)| (path.clone(), part.clone()))
        .collect();
    let assembler = assembly::Assembler::new(payload, weapon_id);
    let assembly = assembler.resolve(&selections);
    let installed_at = |path: &str| -> Option<&str> {
        if let Some(slot) = assembly
            .visible_slots
            .iter()
            .find(|slot| slot.path_key == path)
        {
            return slot
                .selected_part_id
                .as_deref()
                .or(slot.internal_part_id.as_deref());
        }
        assembly
            .internal_parts
            .iter()
            .find(|part| part.slot_path_key == path)
            .map(|part| part.part_id.as_str())
    };
    for (path, part_id, tune_id) in locked_tuning {
        if installed_at(path) != Some(part_id.as_str()) {
            return Err(format!("{STALE_LOCKED_TUNING}: {path}"));
        }
        let known = payload
            .parts
            .get(part_id)
            .is_some_and(|part| part.tuning_options.iter().any(|tune| tune.tune_id == *tune_id));
        if !known {
            return Err(format!("{STALE_LOCKED_TUNING}: {path}"));
        }
    }
    Ok(())
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
    let mut locked_paths = req.locked_paths.clone();
    let locked_tuning = expand_locked_tuning(&mut locked_paths, &req.locked_tuning)?;
    validate_locks(
        &req.payload,
        &weapon_id,
        &locked_paths,
        &excluded,
        &locked_tuning,
    )?;
    let options = SolveOptions {
        mode,
        laser_enabled: req.laser_enabled,
        bipod_deployed: req.bipod_deployed,
        locked_paths,
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
