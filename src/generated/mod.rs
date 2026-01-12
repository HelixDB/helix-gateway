#[allow(clippy::all)]
#[allow(warnings)]
mod gateway;
#[allow(clippy::all)]
#[allow(warnings)]
mod gateway_serde;

pub mod gateway_proto {
    pub use super::gateway::*;
}
