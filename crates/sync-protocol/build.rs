fn main() -> Result<(), Box<dyn std::error::Error>> {
    let schema = "../../protocol/photo-sync.proto";
    println!("cargo:rerun-if-changed={schema}");
    prost_build::compile_protos(&[schema], &["../../protocol"])?;
    Ok(())
}
