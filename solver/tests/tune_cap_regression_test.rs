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

fn run_case(fixture: &str, expected_price: u64) {
    let fixture_json = read_fixture(fixture);
    let payload = serde_json::to_vec(&fixture_json["payload"]).unwrap();
    let request = serde_json::to_vec(&fixture_json["request"]).unwrap();
    let out = delta_weapon_solver::solve_slices(&payload, &request).unwrap();
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
    run_case("real_18010000037_4.json.gz", 254468);
}
