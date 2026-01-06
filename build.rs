use std::{env, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("proto");
    // let proto_files = vec![root.join("gateway.proto")];

    // Tell cargo to recompile if any of these proto files are changed
    // for proto_file in &proto_files {
    //     println!("cargo:rerun-if-changed={}", proto_file.display());
    // }

    let descriptor_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("proto_descriptor.bin");

    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_well_known_types(true)
        .extern_path(".google.protobuf", "::pbjson_types")
        .file_descriptor_set_path(&descriptor_path)
        .compile_protos(&["proto/gateway.proto"], &["."])?;

    let descriptor_set = std::fs::read(descriptor_path)?;
    pbjson_build::Builder::new()
        .register_descriptors(&descriptor_set)?
        .build(&[".gateway"])?;
    Ok(())
}
