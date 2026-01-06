pub mod gateway_proto {
    tonic::include_proto!("gateway");
    include!(concat!(env!("OUT_DIR"), "/gateway.serde.rs"));
}
