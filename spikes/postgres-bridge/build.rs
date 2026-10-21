// Embedding Python on macOS links its framework; the spike's binaries need
// the rpath to find it at run time. A no-op elsewhere, and without Python.
fn main() {
    #[cfg(feature = "python")]
    pyo3_build_config::add_python_framework_link_args();
}
