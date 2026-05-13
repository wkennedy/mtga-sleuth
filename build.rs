//! Build script. The interesting part is the optional `bundled-cards` feature:
//! when enabled, we fetch Scryfall's `default_cards` bulk file at build time,
//! filter it down to entries with an `arena_id` (the same shape `cards/mod.rs`
//! caches at runtime), and write the result to `$OUT_DIR/cards-bundle.json`.
//!
//! `src/cards/mod.rs` then `include_bytes!`s that file behind a `cfg` flag and
//! materializes it to the user's cache dir on first launch when no on-disk
//! cache exists. This means CI release builds carry an immediately-usable card
//! database without forcing every fresh install to do the ~8 MB Scryfall round
//! trip themselves.
//!
//! Without the feature this script is essentially a no-op: it writes an empty
//! placeholder so `include_bytes!` always resolves, and exits.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_BUNDLED_CARDS");
    println!("cargo:rerun-if-env-changed=BUNDLED_CARDS_FORCE_REFRESH");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR set by cargo"));
    let bundle_path = out_dir.join("cards-bundle.json");

    let bundled_enabled = env::var_os("CARGO_FEATURE_BUNDLED_CARDS").is_some();
    if !bundled_enabled {
        // Placeholder so include_bytes! still compiles. Empty array = "no cards".
        if !bundle_path.exists() {
            fs::write(&bundle_path, b"[]").expect("write empty card bundle placeholder");
        }
        return;
    }

    if bundle_path.exists() && env::var_os("BUNDLED_CARDS_FORCE_REFRESH").is_none() {
        eprintln!("build.rs: reusing existing card bundle at {}", bundle_path.display());
        return;
    }

    eprintln!("build.rs: fetching Scryfall bulk-data index…");
    let index = http_get_json("https://api.scryfall.com/bulk-data");
    let download_uri = index["data"]
        .as_array()
        .expect("bulk-data .data is array")
        .iter()
        .find(|e| e["type"] == "default_cards")
        .and_then(|e| e["download_uri"].as_str())
        .expect("default_cards entry not found");

    eprintln!("build.rs: downloading {download_uri}");
    let raw = http_get(download_uri);
    let parsed: serde_json::Value = serde_json::from_slice(&raw).expect("parse default_cards");
    let arr = parsed.as_array().expect("default_cards is array");

    eprintln!("build.rs: filtering {} cards to arena entries…", arr.len());
    let filtered: Vec<serde_json::Value> = arr
        .iter()
        .filter(|c| c.get("arena_id").is_some())
        .map(|c| {
            // Mirror the field set persisted by cards/mod.rs::Card so the
            // bundled JSON can be loaded by the same deserializer at runtime.
            serde_json::json!({
                "arena_id": c["arena_id"],
                "name": c["name"],
                "mana_cost": c.get("mana_cost"),
                "type_line": c.get("type_line"),
                "colors": c.get("colors"),
                "rarity": c.get("rarity"),
                "set": c.get("set"),
                "collector_number": c.get("collector_number"),
                "cmc": c.get("cmc"),
                "image_small": c.get("image_uris").and_then(|u| u.get("small")),
                "image_normal": c.get("image_uris").and_then(|u| u.get("normal")),
                "scryfall_uri": c.get("scryfall_uri"),
            })
        })
        .collect();

    let out = serde_json::to_vec(&filtered).expect("serialize bundle");
    eprintln!(
        "build.rs: writing {} arena cards ({} bytes) to {}",
        filtered.len(),
        out.len(),
        bundle_path.display()
    );
    fs::write(&bundle_path, &out).expect("write card bundle");
}

fn http_get(url: &str) -> Vec<u8> {
    // `ureq` would be tidier but adding a build-only dep just for this is
    // overkill. Shell out to curl, which is available everywhere release CI
    // runs (and we explicitly install it in the workflow if missing).
    let curl = env::var("CURL").unwrap_or_else(|_| "curl".to_string());
    let output = std::process::Command::new(&curl)
        .args(["-fsSL", "-A", "mtga-sleuth-build/0.1", "-H", "Accept: */*", url])
        .output()
        .unwrap_or_else(|e| panic!("failed to run {curl}: {e}"));
    if !output.status.success() {
        panic!(
            "{} {url} exited {}: {}",
            curl,
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    output.stdout
}

fn http_get_json(url: &str) -> serde_json::Value {
    let bytes = http_get(url);
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        let preview = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]);
        panic!("failed to parse JSON from {url}: {e}\nbody preview: {preview}")
    })
}

