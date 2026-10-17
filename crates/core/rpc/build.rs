fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../proto");
    let inputs = [root.join("users.proto")];
    for input in &inputs {
        println!("cargo:rerun-if-changed={}", input.display());
    }
    let descriptors = protox::compile(inputs, [&root])?;
    tonic_prost_build::configure().compile_fds(descriptors)?;
    Ok(())
}
