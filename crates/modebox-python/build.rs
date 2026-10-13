fn main() {
    // On macOS a plain `cargo build` must leave Python's symbols for the
    // interpreter to provide at import time, as maturin does.
    pyo3_build_config::add_extension_module_link_args();
}
