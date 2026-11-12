// Re-embed migrations when a file is added, not only when one changes.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
