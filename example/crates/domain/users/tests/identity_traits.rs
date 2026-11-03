//! Account validation must accept exactly the traits the identity schema
//! (`infra/kratos/identity.schema.json`) accepts. The same case list was
//! checked against a real Kratos registration flow.
use exemplum_dmn_users::{accounts::validate_username, models::validate_name};
use serde_json::Value;

const CASES: &str = include_str!("identity_trait_cases.json");

fn cases(kind: &str) -> Vec<(String, bool)> {
    let cases: Value = serde_json::from_str(CASES).unwrap();
    cases[kind]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| {
            (
                case["value"].as_str().unwrap().to_owned(),
                case["valid"].as_bool().unwrap(),
            )
        })
        .collect()
}

#[test]
fn display_names_match_the_identity_schema() {
    for (name, valid) in cases("display_names") {
        assert_eq!(validate_name(&name).is_ok(), valid, "{name:?}");
    }
}

#[test]
fn usernames_match_the_identity_schema() {
    for (username, valid) in cases("usernames") {
        assert_eq!(validate_username(&username).is_ok(), valid, "{username:?}");
    }
}
