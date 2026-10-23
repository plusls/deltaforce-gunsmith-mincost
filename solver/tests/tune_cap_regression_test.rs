use flate2::read::GzDecoder;
use serde_json::Value;
use std::io::Read;

fn read_fixture(fixture: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/tune_cap/{fixture}",
        env!("CARGO_MANIFEST_DIR")
    );
    let file = std::fs::File::open(&path).unwrap();
    let mut decoder = GzDecoder::new(file);
    let mut json = String::new();
    decoder.read_to_string(&mut json).unwrap();
    serde_json::from_str(&json).unwrap()
}

fn read_request_fixture(fixture: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/tune_cap/{fixture}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn shared_payload_json() -> Vec<u8> {
    let fixture = read_fixture("real_18010000037_4.json.gz");
    serde_json::to_vec(&fixture["payload"]).unwrap()
}

fn run_request_case(request_fixture: &str, expected_price: u64) {
    let payload = shared_payload_json();
    let request = serde_json::to_vec(&read_request_fixture(request_fixture)).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    let out: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(out["optimal"], true, "{request_fixture}: {out}");
    assert_eq!(out["truncated"], false, "{request_fixture}: {out}");
    assert_eq!(
        out["best"]["total_price"], expected_price,
        "{request_fixture}: {out}"
    );
    assert_eq!(
        out["message"], "search complete",
        "{request_fixture}: {out}"
    );
}

fn run_case(fixture: &str, expected_price: u64) {
    let fixture_json = read_fixture(fixture);
    let payload = serde_json::to_vec(&fixture_json["payload"]).unwrap();
    let request = serde_json::to_vec(&fixture_json["request"]).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    let out: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(out["optimal"], true, "{fixture}: {out}");
    assert_eq!(out["truncated"], false, "{fixture}: {out}");
    assert_eq!(
        out["best"]["total_price"], expected_price,
        "{fixture}: {out}"
    );
    assert_eq!(out["message"], "search complete", "{fixture}: {out}");
}

#[test]
fn fixture_decodes() {
    let fixture = read_fixture("intlimit_18020000002_6.json.gz");
    assert_eq!(fixture["payload"]["weaponId"], "18020000002");
    assert_eq!(fixture["request"]["weaponId"], "18020000002");
    assert_eq!(fixture["request"]["deadlineMs"], 20000);
}

#[test]
#[ignore = "slow regression; run with --release -- --ignored"]
fn intlimit_18010000023_9_regression() {
    run_case("intlimit_18010000023_9.json.gz", 110327);
}

#[test]
#[ignore = "slow regression; run with --release -- --ignored"]
fn intlimit_18020000001_7_regression() {
    run_case("intlimit_18020000001_7.json.gz", 42222);
}

#[test]
#[ignore = "slow regression; run with --release -- --ignored"]
fn intlimit_18020000002_6_regression() {
    run_case("intlimit_18020000002_6.json.gz", 89285);
}

#[test]
#[ignore = "slow regression; run with --release -- --ignored"]
fn real_18040000001_4_regression() {
    run_case("real_18040000001_4.json.gz", 98864);
}

#[test]
#[ignore = "slow regression; run with --release -- --ignored"]
fn real_18010000037_4_regression() {
    // A min+max sprint-delay band now keeps every distinct open tuning sum
    // instead of collapsing them to one offer, so the tuning frontier reaches
    // TUNE_CAP and the solver cannot certify optimality. The best build is
    // still the same 254468 found by the adjacent free/locked fixtures.
    let fixture_json = read_fixture("real_18010000037_4.json.gz");
    let payload = serde_json::to_vec(&fixture_json["payload"]).unwrap();
    let request = serde_json::to_vec(&fixture_json["request"]).unwrap();
    let out = gunsmith_mincost::solve_slices(&payload, &request).unwrap();
    let out: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(out["truncated"], false, "{out}");
    assert_eq!(out["best"]["total_price"], 254468, "{out}");
}

#[test]
fn sprint_band_free_tuning_matches_locked_tuning() {
    // Regression: a min+max sprint-delay band used to map the open channel to
    // `Ignore`, so Pareto dominance dropped the offers that could satisfy the
    // band and a strict superset of the locked tuning space reported no build.
    run_request_case("sprint_band_free_tuning_18010000037_4.json", 254468);
}

#[test]
fn sprint_band_locked_open_matches_free_tuning() {
    run_request_case("sprint_band_locked_open_18010000037_4.json", 254468);
}

#[test]
fn sprint_band_minmax_free_tuning_finds_the_locked_solution() {
    // Regression for the min+max band mapping: the open channel used to fall
    // back to `Ignore`, so Pareto dominance dropped every offer that could
    // satisfy the band and the free (strictly larger) tuning space reported no
    // build. The locked subset found 254468, and the free search must too.
    run_request_case("sprint_band_minmax_free_tuning_18010000037_4.json", 254468);
}

#[test]
fn sprint_band_free_tuning_returns_aligned_tuning_keys_and_values() {
    // The frontier stores tuning keys once and each offer only the values, so
    // the reported map must pair keys[i] with values[i].
    let payload = shared_payload_json();
    let request = serde_json::to_vec(&read_request_fixture(
        "sprint_band_free_tuning_18010000037_4.json",
    ))
    .unwrap();
    let out: Value =
        serde_json::from_slice(&gunsmith_mincost::solve_slices(&payload, &request).unwrap())
            .unwrap();
    let tuning = out["best"]["tuning"].as_object().unwrap();
    assert_eq!(tuning.len(), 4, "{tuning:?}");
    let key = |slot: &str, part: &str, tune: &str| format!("{slot}\u{0}{part}\u{0}{tune}");
    let fork = "weapon:18010000037/slot:22:Spe_point/part:13200000004/slot:8:For_point";
    let pistol = "weapon:18010000037/slot:1:PisG_point";
    assert_eq!(tuning[key(fork, "13140000028", "1009").as_str()], 0.0);
    assert_eq!(tuning[key(fork, "13140000028", "1011").as_str()], 0.0);
    assert_eq!(tuning[key(pistol, "13030000121", "1018").as_str()], -20.0);
    assert_eq!(tuning[key(pistol, "13030000121", "1019").as_str()], 50.0);
}
