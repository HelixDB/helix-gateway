// Proto generation is now disabled - using pre-generated code in src/generated/
// To regenerate, uncomment the code below and run `cargo build`

// use std::{env, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // let descriptor_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("proto_descriptor.bin");

    // tonic_prost_build::configure()
    //     .build_client(true)
    //     .build_server(true)
    //     .compile_well_known_types(true)
    //     .extern_path(".google.protobuf", "::pbjson_types")
    //     .file_descriptor_set_path(&descriptor_path)
    //     .compile_protos(&["proto/gateway.proto"], &["."])?;

    // let descriptor_set = std::fs::read(descriptor_path)?;
    // pbjson_build::Builder::new()
    //     .register_descriptors(&descriptor_set)?
    //     .build(&[".gateway"])?;
    Ok(())
}
