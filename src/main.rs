// src/main.rs
//
// Thin native launcher. All real code lives in the library crate (src/lib.rs)
// so the WebAssembly build can share it. On wasm32 the binary is unused — the
// browser entry point is `hopf_sta_viz::start()` (a #[wasm_bindgen(start)] fn).

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    hopf_sta_viz::run_native()
}

#[cfg(target_arch = "wasm32")]
fn main() {}
