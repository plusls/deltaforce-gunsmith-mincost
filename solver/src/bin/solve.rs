use delta_weapon_solver::search::Constraint;
use delta_weapon_solver::{solve_slices, SolveRequest};
use std::fs;
use std::io::{self, Read, Write};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut payload_path: Option<String> = None;
    let mut request_path: Option<String> = None;
    let mut constraints: Vec<Constraint> = Vec::new();
    let mut mode = "operations".to_string();
    let mut weapon_id: Option<String> = None;
    let mut prices_path: Option<String> = None;
    let mut max_nodes = 5_000_000u64;
    let mut max_leaves = 2_000_000u64;
    let mut laser = true;
    let mut bipod = false;
    let mut excluded: Vec<String> = Vec::new();
    let mut cost_cap: Option<i64> = None;
    let mut first_solution = false;
    let mut deadline_ms: Option<u64> = None;
    let mut allow_missing_prices = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--payload" => {
                i += 1;
                payload_path = args.get(i).cloned();
            }
            "--prices" => {
                i += 1;
                prices_path = args.get(i).cloned();
            }
            "--request" => {
                i += 1;
                request_path = args.get(i).cloned();
            }
            "--mode" => {
                i += 1;
                mode = args.get(i).cloned().unwrap_or(mode);
            }
            "--weapon" => {
                i += 1;
                weapon_id = args.get(i).cloned();
            }
            "--max-nodes" => {
                i += 1;
                max_nodes = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(max_nodes);
            }
            "--max-leaves" => {
                i += 1;
                max_leaves = args.get(i).and_then(|v| v.parse().ok()).unwrap_or(max_leaves);
            }
            "--laser-off" => laser = false,
            "--first" => first_solution = true,
            "--deadline-ms" => {
                i += 1;
                let value = args.get(i).ok_or("--deadline-ms needs a value")?;
                deadline_ms = Some(value.parse::<u64>().map_err(|e| format!("bad deadline {value}: {e}"))?);
            }
            "--bipod-deployed" => bipod = true,
            "--allow-missing-prices" => allow_missing_prices = true,
            "--cost-cap" => {
                i += 1;
                cost_cap = args.get(i).and_then(|v| v.parse().ok());
            }
            "--exclude" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    excluded.push(v.clone());
                }
            }
            "--constraint" => {
                i += 1;
                let spec = args.get(i).cloned().unwrap_or_default();
                constraints.push(parse_constraint(&spec)?);
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => {
                if other.starts_with('-') {
                    return Err(format!("unknown argument: {other}"));
                }
                if request_path.is_none() {
                    request_path = Some(other.to_string());
                }
            }
        }
        i += 1;
    }

    if let Some(request_path) = request_path {
        if prices_path.is_some() {
            return Err(
                "--prices cannot be combined with --request; put prices in the request JSON"
                    .to_string(),
            );
        }
        if payload_path.is_none() {
            let text = fs::read_to_string(&request_path).map_err(|e| e.to_string())?;
            let out = delta_weapon_solver::solve_request(
                serde_json::from_str::<SolveRequest>(&text).map_err(|e| e.to_string())?,
            )?;
            println!("{}", serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?);
            return Ok(());
        }
        let payload = fs::read(payload_path.unwrap()).map_err(|e| e.to_string())?;
        let request = fs::read(&request_path).map_err(|e| e.to_string())?;
        let out = solve_slices(&payload, &request)?;
        io::stdout().write_all(&out).map_err(|e| e.to_string())?;
        println!();
        return Ok(());
    }

    let Some(payload_path) = payload_path else {
        let mut text = String::new();
        io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| e.to_string())?;
        let out = delta_weapon_solver::solve_request(
            serde_json::from_str::<SolveRequest>(&text).map_err(|e| e.to_string())?,
        )?;
        println!("{}", serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?);
        return Ok(());
    };

    let payload = fs::read(&payload_path).map_err(|e| e.to_string())?;
    let payload_model: delta_weapon_solver::model::WeaponPayload =
        serde_json::from_slice(&payload).map_err(|e| e.to_string())?;
    let mut prices = std::collections::BTreeMap::new();
    let mut strict_prices = false;
    if let Some(path) = prices_path {
        strict_prices = true;
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        prices = parse_price_map(&text)?;
    }
    if strict_prices && !allow_missing_prices {
        let resolved_weapon = weapon_id
            .clone()
            .or_else(|| payload_model.weapon_id.clone())
            .or_else(|| payload_model.weapon_ids.first().cloned());
        if let Some(weapon) = resolved_weapon.as_ref().and_then(|id| payload_model.parts.get(id)) {
            if !prices.contains_key(weapon.price_id()) {
                return Err(format!(
                    "weapon {} has no price; add it to the price file or pass --allow-missing-prices",
                    weapon.id
                ));
            }
        }
        for (id, part) in &payload_model.parts {
            if part.is_virtual() {
                continue;
            }
            if !prices.contains_key(part.price_id()) {
                excluded.push(id.clone());
            }
        }
    }
    let request = serde_json::json!({
        "prices": prices,
        "weaponId": weapon_id,
        "mode": mode,
        "laserEnabled": laser,
        "bipodDeployed": bipod,
        "excludedPartIds": excluded,
        "constraints": constraints,
        "maxEvaluations": max_nodes,
        "maxLeaves": max_leaves,
        "costCap": cost_cap,
        "firstSolution": first_solution,
        "deadlineMs": deadline_ms,
    });
    let request = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
    let out = solve_slices(&payload, &request)?;
    let value: serde_json::Value = serde_json::from_slice(&out).map_err(|e| e.to_string())?;
    println!("{}", serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?);
    Ok(())
}

fn parse_constraint(spec: &str) -> Result<Constraint, String> {
    let (id, rest) = spec
        .split_once(':')
        .ok_or_else(|| format!("constraint must be id:min:max or id:max, got {spec}"))?;
    let parts: Vec<&str> = rest.split(':').collect();
    let (min, max) = match parts.as_slice() {
        [max] => (None, Some(parse_num(max)?)),
        [min, max] => (Some(parse_num(min)?), Some(parse_num(max)?)),
        _ => return Err(format!("bad constraint: {spec}")),
    };
    Ok(Constraint::new(id, min, max))
}

fn parse_num(value: &str) -> Result<f64, String> {
    value
        .parse::<f64>()
        .map_err(|e| format!("bad number {value}: {e}"))
}

fn print_help() {
    println!(
        "usage: solve [--payload weapon.json] [--prices prices.json] [--request request.json]\n\
         \n\
         If --payload is omitted the request JSON is read from stdin.\n\
         Constraint syntax:\n\
           --constraint recoil-continuous-vertical:0.8\n\
           --constraint recoil-continuous-horizontal:0.5:0.9\n\
         Supported metrics: recoil-continuous-horizontal, recoil-continuous-vertical,\n\
           recoil-single-horizontal, recoil-single-vertical, recoil-fire-stability,\n\
           and panel attribute ids (10001, 10005, 10006, 10007, 10008).\n\
         --first stops at the first feasible build instead of proving optimality.\n\
         --deadline-ms N stops after N milliseconds and returns the best build found.
\
         With --prices, parts missing a price are excluded by default.
\
         Use --allow-missing-prices to treat them as price 0."
    );
}

/// Accepts both a bare `{ "partId": price }` map and a wrapped
/// `{ "prices": { ... } }` payload, since snapshots exist in both shapes.
fn parse_price_map(text: &str) -> Result<std::collections::BTreeMap<String, f64>, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let map = match value.get("prices") {
        Some(prices) => prices.as_object(),
        None => value.as_object(),
    };
    Ok(map
        .into_iter()
        .flatten()
        .filter_map(|(id, price)| price.as_f64().map(|value| (id.clone(), value)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::parse_price_map;

    #[test]
    fn price_map_accepts_a_bare_object() {
        let prices = parse_price_map(r#"{ "a": 1, "b": 2.5, "c": "x" }"#).unwrap();
        assert_eq!(prices.get("a"), Some(&1.0));
        assert_eq!(prices.get("b"), Some(&2.5));
        assert_eq!(prices.get("c"), None);
    }

    #[test]
    fn price_map_accepts_a_wrapped_object() {
        let prices = parse_price_map(r#"{ "dataVersion": "x", "prices": { "a": 3 } }"#).unwrap();
        assert_eq!(prices.len(), 1);
        assert_eq!(prices.get("a"), Some(&3.0));
    }

    #[test]
    fn price_map_ignores_metadata_outside_prices() {
        let prices = parse_price_map(r#"{ "prices": {}, "a": 9 }"#).unwrap();
        assert!(prices.is_empty());
    }
}
