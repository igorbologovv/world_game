fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/economy/v1/economy.proto");
    prost_build::compile_protos(&["proto/economy/v1/economy.proto"], &["proto"])?;
    Ok(())
}
