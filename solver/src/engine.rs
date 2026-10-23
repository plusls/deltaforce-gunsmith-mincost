use crate::locks::{expand_locked_tuning, validate_locks};
use crate::request::SolveRequest;
use crate::search::{self, SolveOptions, SolveResult, Solver};
use rustc_hash::{FxHashMap, FxHashSet};

pub fn solve_request(req: SolveRequest) -> Result<SolveResult, String> {
    let options = req.options;
    for c in &options.constraints {
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
    let weapon_id = options
        .weapon_id
        .clone()
        .or_else(|| req.payload.weapon_id.clone())
        .or_else(|| req.payload.weapon_ids.first().cloned())
        .ok_or_else(|| "weapon id is missing".to_string())?;
    if !req.payload.parts.contains_key(&weapon_id) {
        return Err(format!("weapon {} not found in payload", weapon_id));
    }
    let mode = options
        .mode
        .clone()
        .unwrap_or_else(|| "operations".to_string());
    let prices: FxHashMap<String, i64> = options
        .prices
        .iter()
        .map(|(k, v)| (k.clone(), v.round() as i64))
        .collect();
    let excluded: FxHashSet<String> = options.excluded_part_ids.iter().cloned().collect();
    let mut locked_paths = options.locked_paths.clone();
    let locked_tuning = expand_locked_tuning(&mut locked_paths, &options.locked_tuning)?;
    validate_locks(
        &req.payload,
        &weapon_id,
        &locked_paths,
        &excluded,
        &locked_tuning,
    )?;
    let options = SolveOptions {
        mode,
        laser_enabled: options.laser_enabled,
        bipod_deployed: options.bipod_deployed,
        locked_paths,
        locked_tuning: options.locked_tuning,
        excluded_part_ids: excluded,
        constraints: options.constraints,
        max_nodes: options.max_nodes,
        max_leaves: options.max_leaves,
        max_results: options.max_results,
        fixed_cost_cap: options.cost_cap,
        first_solution: options.first_solution,
        deadline_ms: options.deadline_ms,
    };
    let payload = &req.payload;
    let solver = Solver::new(payload, &weapon_id, &prices, &options);
    Ok(solver.solve())
}
