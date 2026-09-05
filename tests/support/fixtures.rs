use serde_json::Value;
use std::path::PathBuf;

pub fn fixture(name: &str) -> Value {
    let filename = match name {
        "moviebox-search.json"
        | "moviebox-details.json"
        | "moviebox-resources.json"
        | "moviebox-captions.json"
        | "moviebox-play-info.json" => name,
        _ => panic!("{name}"),
    };

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(filename);
    let contents = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{name}"));
    serde_json::from_str(&contents).unwrap_or_else(|_| panic!("{name}"))
}
