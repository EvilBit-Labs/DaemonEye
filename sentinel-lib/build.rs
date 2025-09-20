use std::io::Result;

fn main() -> Result<()> {
    // Configure prost to generate code for our protobuf files
    prost_build::Config::new()
        // Generate serde derives for JSON serialization
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        // Add clippy allow attributes for generated code
        .type_attribute(".", "#[allow(clippy::doc_markdown)]")
        .type_attribute(".", "#[allow(clippy::missing_const_for_fn)]")
        .type_attribute(".", "#[allow(clippy::trivially_copy_pass_by_ref)]")
        .type_attribute(".", "#[allow(clippy::pattern_type_mismatch)]")
        // Enable optional features for proto3 optional fields
        .protoc_arg("--experimental_allow_proto3_optional")
        // Compile our protobuf files
        .compile_protos(&["proto/ipc.proto"], &["proto/"])?;

    println!("cargo:rerun-if-changed=proto/");
    println!("cargo:rerun-if-changed=proto/common.proto");
    println!("cargo:rerun-if-changed=proto/ipc.proto");

    Ok(())
}
