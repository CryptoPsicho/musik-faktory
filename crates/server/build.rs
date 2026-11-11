// The UI is embedded from dist/public. Make sure the folder exists so the
// server also compiles before the UI has been built.
fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/public");
    let _ = std::fs::create_dir_all(&dir);
    println!("cargo:rerun-if-changed=../../dist/public");
    println!("cargo:rerun-if-changed=static");
}
