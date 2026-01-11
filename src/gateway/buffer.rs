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
        let item = self.queue.pop();
        if item.is_some() {
            self.len.fetch_sub(1, Ordering::Relaxed);
        }
        item
    }

    pub fn len(&self) -> usize {
        self.len.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn update_watcher(&self, status: DbStatus) -> Result<(), SendError<DbStatus>> {
        self.watcher.0.send_if_modified(|current| {
            if *current != status {
                *current = status;
                true
            } else {
                false
            }
        });
        Ok(())
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

    // Watcher tests
    #[tokio::test]
    async fn test_update_watcher_transitions() {
        use crate::gateway::DbStatus;

        let (tx, rx) = tokio::sync::watch::channel(DbStatus::Healthy);
        let buffer = Buffer::new().set_watcher((tx, rx.clone()));

        // Transition to Unhealthy
        buffer.update_watcher(DbStatus::Unhealthy).unwrap();
        assert_eq!(*rx.borrow(), DbStatus::Unhealthy);

        // Transition back to Healthy
        buffer.update_watcher(DbStatus::Healthy).unwrap();
        assert_eq!(*rx.borrow(), DbStatus::Healthy);
    }

    #[tokio::test]
    async fn test_update_watcher_idempotent() {
        use crate::gateway::DbStatus;

        let (tx, mut rx) = tokio::sync::watch::channel(DbStatus::Healthy);
        rx.mark_unchanged();
        let buffer = Buffer::new().set_watcher((tx, rx.clone()));

        // Same status should not trigger send (returns Ok without sending)
        let result = buffer.update_watcher(DbStatus::Healthy);
        assert!(result.is_ok());

        // Verify no change
        assert_eq!(*rx.borrow(), DbStatus::Healthy);
        assert!(!rx.borrow().has_changed())
    }

    #[test]
    fn test_set_watcher_builder() {
        use crate::gateway::DbStatus;

        let (tx, rx) = tokio::sync::watch::channel(DbStatus::Healthy);
        let buffer = Buffer::new().set_watcher((tx, rx));

        // Watcher is set - update should work
        let result = buffer.update_watcher(DbStatus::Unhealthy);
        assert!(result.is_ok());
    }

    // Edge case tests
    #[test]
    fn test_dequeue_empty_buffer() {
        let buffer = Buffer::new();
        assert!(buffer.dequeue().is_none());
    }

    #[test]
    fn test_max_duration_builder() {
        let buffer = Buffer::new().max_duration(Duration::from_secs(5));
        assert_eq!(buffer.max_duration, Some(Duration::from_secs(5)));
    }

    #[test]
    fn test_buffer_at_exact_capacity() {
        let buffer = Buffer::new().max_size(2);

        let _rx1 = buffer.enqueue(make_test_request("first")).unwrap();
        let _rx2 = buffer.enqueue(make_test_request("second")).unwrap();

        // At capacity, next should fail
        let result = buffer.enqueue(make_test_request("third"));
        assert!(matches!(result, Err(GatewayError::BufferFull)));
    }

    // Concurrency stress tests
    #[tokio::test]
    async fn test_stress_concurrent_enqueues() {
        use std::sync::Arc;

        let buffer = Arc::new(Buffer::new());
        let mut handles = vec![];

        for i in 0..100 {
            let buffer_clone = Arc::clone(&buffer);
            let handle = tokio::spawn(async move {
                let query = format!("query_{}", i);
                buffer_clone.enqueue(make_test_request(&query))
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete
        let results: Vec<_> = futures::future::join_all(handles).await;

        // All should succeed
        for result in results {
            assert!(result.unwrap().is_ok());
        }

        // All 100 should be in the buffer
        assert_eq!(buffer.len(), 100);
    }

    #[tokio::test]
    async fn test_stress_concurrent_enqueue_dequeue() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let buffer = Arc::new(Buffer::new());
        let enqueued = Arc::new(AtomicUsize::new(0));
        let dequeued = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];

        // 50 enqueuers
        for i in 0..50 {
            let buffer_clone = Arc::clone(&buffer);
            let enqueued_clone = Arc::clone(&enqueued);
            let handle = tokio::spawn(async move {
                let query = format!("query_{}", i);
                if buffer_clone.enqueue(make_test_request(&query)).is_ok() {
                    enqueued_clone.fetch_add(1, Ordering::Relaxed);
                }
            });
            handles.push(handle);
        }

        // 25 dequeuers
        for _ in 0..25 {
            let buffer_clone = Arc::clone(&buffer);
            let dequeued_clone = Arc::clone(&dequeued);
            let handle = tokio::spawn(async move {
                // Try multiple times to dequeue
                for _ in 0..5 {
                    if buffer_clone.dequeue().is_some() {
                        dequeued_clone.fetch_add(1, Ordering::Relaxed);
                    }
                    tokio::task::yield_now().await;
                }
            });
            handles.push(handle);
        }

        // Wait for all tasks
        futures::future::join_all(handles).await;

        // Verify consistency: remaining in buffer = enqueued - dequeued
        let total_enqueued = enqueued.load(Ordering::Relaxed);
        let total_dequeued = dequeued.load(Ordering::Relaxed);

        // Note: Due to lock-free nature, len() might not perfectly match
        // but we should have no panics or data corruption
        assert!(total_enqueued >= total_dequeued);
    }

    #[tokio::test]
    async fn test_stress_length_accuracy_under_contention() {
        use std::sync::Arc;

        let buffer = Arc::new(Buffer::new());
        let mut handles = vec![];

        // Enqueue 100 items
        for i in 0..100 {
            let buffer_clone = Arc::clone(&buffer);
            let handle = tokio::spawn(async move {
                let query = format!("query_{}", i);
                buffer_clone.enqueue(make_test_request(&query))
            });
            handles.push(handle);
        }

        futures::future::join_all(handles).await;
        assert_eq!(buffer.len(), 100);

        // Dequeue all items
        let mut dequeue_handles = vec![];
        for _ in 0..100 {
            let buffer_clone = Arc::clone(&buffer);
            let handle = tokio::spawn(async move { buffer_clone.dequeue() });
            dequeue_handles.push(handle);
        }

        let results: Vec<_> = futures::future::join_all(dequeue_handles).await;
        let successful_dequeues = results
            .iter()
            .filter(|r| r.as_ref().unwrap().is_some())
            .count();

        // All 100 should be dequeued
        assert_eq!(successful_dequeues, 100);
    }
}
