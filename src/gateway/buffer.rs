use crate::{
    GatewayError,
    gateway::DbStatus,
    generated::gateway_proto::{QueryRequest, QueryResponse},
};
use lockfree::stack::Stack;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::sync::{
    oneshot,
    watch::{Receiver, Sender, error::SendError},
};

pub type BufferedResponse = Result<QueryResponse, GatewayError>;

pub struct BufferedRequest {
    pub(crate) request: QueryRequest,
    pub(crate) enqueued_at: std::time::Instant,
    response_tx: oneshot::Sender<BufferedResponse>,
}

impl BufferedRequest {
    /// Send response back to waiting handler. Consumes self.
    pub fn respond(self, response: BufferedResponse) -> Result<(), BufferedResponse> {
        self.response_tx.send(response)
    }

    /// Check if receiver was dropped (client disconnected)
    pub fn is_cancelled(&self) -> bool {
        self.response_tx.is_closed()
    }

    pub fn request(&self) -> &QueryRequest {
        &self.request
    }
}

#[derive(Default)]
pub struct Buffer {
    queue: Stack<BufferedRequest>,
    max_size: Option<usize>,
    max_duration: Option<Duration>,
    len: AtomicUsize,
    watcher: (Sender<DbStatus>, Option<Receiver<DbStatus>>),
}

impl Buffer {
    pub fn new() -> Self {
        Buffer::default()
    }

    pub fn max_size(mut self, max_size: usize) -> Self {
        self.max_size = Some(max_size);
        self
    }

    pub fn max_duration(mut self, max_duration: Duration) -> Self {
        self.max_duration = Some(max_duration);
        self
    }

    pub fn set_watcher(mut self, watcher: (Sender<DbStatus>, Receiver<DbStatus>)) -> Self {
        self.watcher = (watcher.0, Some(watcher.1));
        self
    }

    pub fn enqueue(
        &self,
        request: QueryRequest,
    ) -> Result<oneshot::Receiver<BufferedResponse>, GatewayError> {
        if let Some(max) = self.max_size {
            if self.len() >= max {
                return Err(GatewayError::BufferFull);
            }
        }

        let (tx, rx) = oneshot::channel();
        self.queue.push(BufferedRequest {
            request,
            enqueued_at: std::time::Instant::now(),
            response_tx: tx,
        });
        self.len.fetch_add(1, Ordering::Relaxed);
        Ok(rx)
    }

    pub fn dequeue(&self) -> Option<BufferedRequest> {
        self.queue.pop()
    }

    pub fn len(&self) -> usize {
        self.len.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn update_watcher(&self, status: DbStatus) -> Result<(), SendError<DbStatus>> {
        if let Some(rx) = &self.watcher.1
            && *rx.borrow() != status
        {
            self.watcher.0.send(status)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_request(query: &str) -> QueryRequest {
        QueryRequest {
            request_type: 0,
            query: query.to_string(),
            parameters: None,
            embeddings: None,
        }
    }

    #[tokio::test]
    async fn test_enqueue_and_respond() {
        let buffer = Buffer::new();
        let rx = buffer.enqueue(make_test_request("test")).unwrap();

        let req = buffer.dequeue().unwrap();
        let response = QueryResponse {
            data: vec![1, 2, 3],
            status: 200,
        };
        req.respond(Ok(response)).unwrap();

        let received = rx.await.unwrap().unwrap();
        assert_eq!(received.status, 200);
        assert_eq!(received.data, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_is_cancelled_when_receiver_dropped() {
        let buffer = Buffer::new();
        let rx = buffer.enqueue(make_test_request("test")).unwrap();
        drop(rx);

        let req = buffer.dequeue().unwrap();
        assert!(req.is_cancelled());
    }

    #[tokio::test]
    async fn test_buffer_full_error() {
        let buffer = Buffer::new().max_size(1);
        let _rx1 = buffer.enqueue(make_test_request("first")).unwrap();

        let result = buffer.enqueue(make_test_request("second"));
        assert!(result.is_err());
    }

    #[test]
    fn test_len_tracking() {
        let buffer = Buffer::new();
        assert_eq!(buffer.len(), 0);
        assert!(buffer.is_empty());

        let _rx1 = buffer.enqueue(make_test_request("first")).unwrap();
        assert_eq!(buffer.len(), 1);

        let _rx2 = buffer.enqueue(make_test_request("second")).unwrap();
        assert_eq!(buffer.len(), 2);
    }

    #[tokio::test]
    async fn test_error_response() {
        let buffer = Buffer::new();
        let rx = buffer.enqueue(make_test_request("test")).unwrap();

        let req = buffer.dequeue().unwrap();
        req.respond(Err(GatewayError::BackendUnavailable)).unwrap();

        let received = rx.await.unwrap();
        assert!(received.is_err());
    }

    #[tokio::test]
    async fn test_lifo_order() {
        let buffer = Buffer::new();
        let _rx_a = buffer.enqueue(make_test_request("A")).unwrap();
        let _rx_b = buffer.enqueue(make_test_request("B")).unwrap();

        // Stack is LIFO, so B should come out first
        let first = buffer.dequeue().unwrap();
        assert_eq!(first.request().query, "B");

        let second = buffer.dequeue().unwrap();
        assert_eq!(second.request().query, "A");
    }

    #[test]
    fn test_request_accessor() {
        let buffer = Buffer::new();
        let _rx = buffer.enqueue(make_test_request("my_query")).unwrap();

        let req = buffer.dequeue().unwrap();
        assert_eq!(req.request().query, "my_query");
    }
}
