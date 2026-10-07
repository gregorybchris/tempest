//! Reads arbitrary text as the results API's query strings, and checks that each query it
//! accepts reads back the same from the query string it writes.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tempest_sim_api::query::{CompareQuery, ExperimentQuery, RunQuery};

fuzz_target!(|text: &str| {
    if let Ok(query) = RunQuery::parse(text) {
        assert_eq!(RunQuery::parse(&query.to_query()), Ok(query));
    }
    if let Ok(query) = ExperimentQuery::parse(text) {
        assert_eq!(ExperimentQuery::parse(&query.to_query()), Ok(query));
    }
    if let Ok(query) = CompareQuery::parse(text) {
        assert_eq!(CompareQuery::parse(&query.to_query()), Ok(query));
    }
});
