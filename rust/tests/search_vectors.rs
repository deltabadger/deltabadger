//! The asset search's string arithmetic against what Ruby computed (script/rust/record_search_vectors.rb):
//! the jaro_winkler gem's similarity, String#downcase, and the lazy frame's next-page query.
use deltabadger::web::bot::search;
use serde_json::Value;

fn vectors() -> Value {
    serde_json::from_str(include_str!("fixtures/search_vectors.json")).unwrap()
}

#[test]
fn recorded_from_the_rails_files_of_this_tree() {
    use sha2::{Digest, Sha256};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, hash) in vectors()["sources"].as_object().unwrap() {
        assert_eq!(format!("{:x}", Sha256::digest(std::fs::read(root.join(path)).unwrap())), hash.as_str().unwrap(), "{path}: script/rust/repin");
    }
}

#[test]
fn jaro_winkler_scores_as_the_gem() {
    let rows = vectors()["jaro_winkler"].as_array().unwrap().clone();
    assert!(rows.len() >= 497);
    for row in rows {
        let (a, b, want) = (row[0].as_str().unwrap(), row[1].as_str().unwrap(), row[2].as_str().unwrap().parse::<f64>().unwrap());
        assert_eq!(search::jaro_winkler(a, b), want, "{a:?} / {b:?}");
    }
    assert_eq!(search::jaro_winkler("ab", "ba"), 0.0);
}

#[test]
fn downcase_maps_as_ruby() {
    for row in vectors()["downcase"].as_array().unwrap() {
        assert_eq!(search::downcase(row[0].as_str().unwrap()), row[1].as_str().unwrap(), "{row}");
    }
}

#[test]
fn next_page_query_is_rails_to_query() {
    for row in vectors()["next_page_query"].as_array().unwrap() {
        assert_eq!(search::next_page_query(row[0].as_str().unwrap(), 20), row[1].as_str().unwrap(), "{row}");
    }
}
