fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    println!("cargo:rerun-if-changed={}", root.join("users.proto").display());
    let descriptors = protox::compile([root.join("users.proto")], [&root])?;
    tonic_build::configure().compile_fds(descriptors)?;
    Ok(())
}
