use crate::generated::gateway_proto::backend_service_client::BackendServiceClient;
use eyre::Result;
use tonic::transport::Channel;

#[derive(Clone)]
pub struct ProtoClient {
    client: BackendServiceClient<Channel>,
}

impl ProtoClient {
    pub async fn connect(addr: &str) -> Result<Self> {
        let channel = Channel::from_shared(addr.to_string())?.connect().await?;

        Ok(Self {
            client: BackendServiceClient::new(channel),
        })
    }

    pub fn inner(&mut self) -> &mut BackendServiceClient<Channel> {
        &mut self.client
    }

    pub fn client(&self) -> BackendServiceClient<Channel> {
        self.client.clone()
    }
}
