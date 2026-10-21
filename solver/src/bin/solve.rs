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
    let mut prices = std::collections::BTreeMap::new();
    if let Some(path) = prices_path {
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let map = value
            .get("prices")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        for (k, v) in map {
            if let Some(n) = v.as_f64() {
                prices.insert(k, n);
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
         --deadline-ms N stops after N milliseconds and returns the best build found."
    );
}
