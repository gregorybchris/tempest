//! The gallery's browser tests, in one test binary (one wasm module) to link once.
//!
//! The browser configuration is set once here, for all of them.

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test_configure;

wasm_bindgen_test_configure!(run_in_browser);

mod live_view;
mod select;
