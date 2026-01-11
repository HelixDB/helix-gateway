//! Gateway Performance Benchmark
//!
//! Tests throughput and latency for:
//! 1. Healthy gateway - direct request processing
//! 2. Unhealthy gateway - requests being buffered
//! 3. Recovery scenario - buffer drains after recovery

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use hdrhistogram::Histogram;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot};
use tonic::transport::Server;
use tonic::{Response, Status};
use tower::ServiceExt;

use helix_gateway::config::{Config, GrpcConfig};
use helix_gateway::format::Format;
use helix_gateway::gateway::DbStatus;
use helix_gateway::gateway::buffer::Buffer;
use helix_gateway::gateway::introspection::{DbQuery, Introspection};
use helix_gateway::gateway::routes::{create_router, process_buffer};
use helix_gateway::gateway::state::AppState;
use helix_gateway::generated::gateway_proto::backend_service_server::{
    BackendService, BackendServiceServer,
};
use helix_gateway::generated::gateway_proto::{
    HealthRequest, HealthResponse, QueryRequest, QueryResponse, RequestType,
};

const TOTAL_REQUESTS: usize = 10;
const CONCURRENCY: usize = 1;

// ============================================================================
// Dynamic Mock Backend Service
// ============================================================================

struct DynamicMockBackendService {
    healthy: Arc<AtomicBool>,
    request_count: Arc<AtomicUsize>,
}

impl DynamicMockBackendService {
    fn new() -> (Self, Arc<AtomicBool>, Arc<AtomicUsize>) {
        let healthy = Arc::new(AtomicBool::new(true));
        let request_count = Arc::new(AtomicUsize::new(0));
        (
            Self {
                healthy: Arc::clone(&healthy),
                request_count: Arc::clone(&request_count),
            },
            healthy,
            request_count,
        )
    }
}

#[tonic::async_trait]
impl BackendService for DynamicMockBackendService {
    async fn query(
        &self,
        request: tonic::Request<QueryRequest>,
    ) -> Result<Response<QueryResponse>, Status> {
        self.request_count.fetch_add(1, Ordering::Relaxed);

        if !self.healthy.load(Ordering::Relaxed) {
            return Err(Status::unavailable("database unavailable"));
        }

        let req = request.into_inner();
        Ok(Response::new(QueryResponse {
            data: format!("Result: {}", req.query).into_bytes(),
            status: 200,
        }))
    }

    async fn health(
        &self,
        _request: tonic::Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        Ok(Response::new(HealthResponse {
            healthy: self.healthy.load(Ordering::Relaxed),
            version: "bench".to_string(),
        }))
    }
}

// ============================================================================
// Benchmark Result
// ============================================================================

struct BenchmarkResult {
    scenario: String,
    total_requests: usize,
    successful: usize,
    failed: usize,
    duration: Duration,
    latency_histogram: Histogram<u64>,
}

impl BenchmarkResult {
    fn throughput(&self) -> f64 {
        self.successful as f64 / self.duration.as_secs_f64()
    }

    fn p50(&self) -> Duration {
        Duration::from_micros(self.latency_histogram.value_at_quantile(0.50))
    }

    fn p95(&self) -> Duration {
        Duration::from_micros(self.latency_histogram.value_at_quantile(0.95))
    }

    fn p99(&self) -> Duration {
        Duration::from_micros(self.latency_histogram.value_at_quantile(0.99))
    }

    fn max(&self) -> Duration {
        Duration::from_micros(self.latency_histogram.max())
    }

    fn print(&self) {
        println!("Scenario: {}", self.scenario);
        println!("  Total requests:  {:>12}", format_num(self.total_requests));
        println!("  Successful:      {:>12}", format_num(self.successful));
        println!("  Failed:          {:>12}", format_num(self.failed));
        println!("  Duration:        {:>12.2}s", self.duration.as_secs_f64());
        println!("  Throughput:      {:>12.0} req/s", self.throughput());
        println!(
            "  Latency p50:     {:>12.2}ms",
            self.p50().as_secs_f64() * 1000.0
        );
        println!(
            "  Latency p95:     {:>12.2}ms",
            self.p95().as_secs_f64() * 1000.0
        );
        println!(
            "  Latency p99:     {:>12.2}ms",
            self.p99().as_secs_f64() * 1000.0
        );
        println!(
            "  Latency max:     {:>12.2}ms",
            self.max().as_secs_f64() * 1000.0
        );
        println!();
    }
}

fn format_num(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

// ============================================================================
// Test Infrastructure
// ============================================================================

async fn start_mock_grpc_server(
    service: DynamicMockBackendService,
) -> (SocketAddr, oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    tokio::spawn(async move {
        Server::builder()
            .add_service(BackendServiceServer::new(service))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async {
                    shutdown_rx.await.ok();
                },
            )
            .await
            .unwrap();
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, shutdown_tx)
}

fn create_test_queries() -> Introspection {
    let mut queries = HashMap::new();
    queries.insert(
        "bench_query".to_string(),
        DbQuery {
            request_type: RequestType::Read,
            parameters: pbjson_types::Struct::default(),
            return_types: pbjson_types::Struct::default(),
            embedding_config: None,
        },
    );
    Introspection {
        queries,
        schema: HashMap::new(),
    }
}

async fn create_app_state_with_buffer(
    backend_addr: &str,
    max_buffer_size: usize,
) -> (
    AppState,
    Arc<Buffer>,
    tokio::sync::watch::Receiver<DbStatus>,
) {
    let grpc_config = GrpcConfig::default().with_backend_addr(backend_addr);
    let client = helix_gateway::client::ProtoClient::connect(&grpc_config)
        .await
        .expect("Failed to connect to mock backend");

    let (tx, mut rx) = tokio::sync::watch::channel(DbStatus::Healthy);
    rx.mark_changed();

    let buffer = Arc::new(
        Buffer::new()
            .max_size(max_buffer_size)
            .max_duration(Duration::from_secs(60))
            .set_watcher((tx, rx.clone())),
    );

    let state = AppState::new(client)
        .with_config(Config::default())
        .with_format(Format::Json)
        .with_introspection(create_test_queries())
        .with_buffer(Arc::clone(&buffer));

    (state, buffer, rx)
}

async fn make_request(router: Router, query: &str) -> (StatusCode, Duration) {
    let body = format!(
        r#"{{"request_type": 0, "query": "{}", "parameters": {{}}}}"#,
        query
    );

    let request = Request::builder()
        .method("POST")
        .uri("/bench_query")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let start = Instant::now();
    let response = router.oneshot(request).await.unwrap();
    let latency = start.elapsed();

    (response.status(), latency)
}

// ============================================================================
// Benchmark Scenarios
// ============================================================================

async fn run_healthy_benchmark() -> BenchmarkResult {
    let (service, _healthy_flag, _request_count) = DynamicMockBackendService::new();
    let (addr, _shutdown) = start_mock_grpc_server(service).await;

    let (state, _buffer, _rx) =
        create_app_state_with_buffer(&format!("http://{}", addr), TOTAL_REQUESTS).await;

    let requests_per_worker = TOTAL_REQUESTS / CONCURRENCY;
    let histogram = Arc::new(Mutex::new(Histogram::<u64>::new(3).unwrap()));
    let successful = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));

    println!(
        "  Starting {} workers with {} requests each...",
        CONCURRENCY, requests_per_worker
    );

    let start = Instant::now();

    let handles: Vec<_> = (0..CONCURRENCY)
        .map(|_| {
            let state = state.clone();
            let histogram = Arc::clone(&histogram);
            let successful = Arc::clone(&successful);
            let failed = Arc::clone(&failed);

            tokio::spawn(async move {
                let mut local_histogram = Histogram::<u64>::new(3).unwrap();

                for _ in 0..requests_per_worker {
                    let router = create_router().with_state(state.clone());
                    let (status, latency) = make_request(router, "bench_query").await;

                    if status == StatusCode::OK {
                        successful.fetch_add(1, Ordering::Relaxed);
                        local_histogram.record(latency.as_micros() as u64).ok();
                    } else {
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                }

                histogram.lock().await.add(&local_histogram).ok();
            })
        })
        .collect();

    futures::future::join_all(handles).await;
    let duration = start.elapsed();

    BenchmarkResult {
        scenario: "Healthy Gateway".to_string(),
        total_requests: TOTAL_REQUESTS,
        successful: successful.load(Ordering::Relaxed),
        failed: failed.load(Ordering::Relaxed),
        duration,
        latency_histogram: Arc::try_unwrap(histogram).unwrap().into_inner(),
    }
}

async fn run_unhealthy_benchmark() -> BenchmarkResult {
    let (service, healthy_flag, _request_count) = DynamicMockBackendService::new();
    let (addr, _shutdown) = start_mock_grpc_server(service).await;

    // Start unhealthy
    healthy_flag.store(false, Ordering::SeqCst);

    let (state, buffer, _rx) =
        create_app_state_with_buffer(&format!("http://{}", addr), TOTAL_REQUESTS * 2).await;

    let requests_per_worker = TOTAL_REQUESTS / CONCURRENCY;
    let histogram = Arc::new(Mutex::new(Histogram::<u64>::new(3).unwrap()));
    let enqueued = Arc::new(AtomicUsize::new(0));

    println!(
        "  Starting {} workers (DB unhealthy, requests will buffer)...",
        CONCURRENCY
    );

    let start = Instant::now();

    // Spawn workers that will have their requests buffered
    let handles: Vec<_> = (0..CONCURRENCY)
        .map(|_| {
            let state = state.clone();
            let histogram = Arc::clone(&histogram);
            let enqueued = Arc::clone(&enqueued);

            tokio::spawn(async move {
                let mut local_histogram = Histogram::<u64>::new(3).unwrap();

                for _ in 0..requests_per_worker {
                    let router = create_router().with_state(state.clone());
                    let req_start = Instant::now();

                    // This will buffer since DB is unhealthy
                    // We use a timeout to not wait forever
                    // Timeout must exceed retry strategy (~700ms) so requests reach buffer.enqueue()
                    let result = tokio::time::timeout(
                        Duration::from_millis(2000),
                        make_request(router, "bench_query"),
                    )
                    .await;

                    let latency = req_start.elapsed();

                    // Request got buffered (timeout means it's waiting)
                    if result.is_err() {
                        enqueued.fetch_add(1, Ordering::Relaxed);
                        local_histogram.record(latency.as_micros() as u64).ok();
                    }
                }

                histogram.lock().await.add(&local_histogram).ok();
            })
        })
        .collect();

    futures::future::join_all(handles).await;
    let duration = start.elapsed();

    let buffered = buffer.len();
    println!("  Buffer size after fill: {}", format_num(buffered));

    BenchmarkResult {
        scenario: "Unhealthy Gateway (Buffering)".to_string(),
        total_requests: TOTAL_REQUESTS,
        successful: 0,
        failed: enqueued.load(Ordering::Relaxed),
        duration,
        latency_histogram: Arc::try_unwrap(histogram).unwrap().into_inner(),
    }
}

async fn run_recovery_benchmark() -> BenchmarkResult {
    let (service, healthy_flag, _request_count) = DynamicMockBackendService::new();
    let (addr, _shutdown) = start_mock_grpc_server(service).await;

    // Start unhealthy
    healthy_flag.store(false, Ordering::SeqCst);

    let (state, buffer, _rx) =
        create_app_state_with_buffer(&format!("http://{}", addr), TOTAL_REQUESTS * 2).await;

    let half_requests = TOTAL_REQUESTS / 2;
    let requests_per_worker = half_requests / CONCURRENCY;
    let histogram = Arc::new(Mutex::new(Histogram::<u64>::new(3).unwrap()));
    let successful = Arc::new(AtomicUsize::new(0));
    let buffered_count = Arc::new(AtomicUsize::new(0));

    println!(
        "  Phase 1: Sending {} requests (DB unhealthy, will buffer)...",
        half_requests
    );

    let start = Instant::now();

    // Phase 1: Send requests that will buffer (with timeout so they don't block forever)
    let buffer_handles: Vec<_> = (0..CONCURRENCY)
        .map(|_| {
            let state = state.clone();
            let buffered_count = Arc::clone(&buffered_count);

            tokio::spawn(async move {
                for _ in 0..requests_per_worker {
                    let router = create_router().with_state(state.clone());

                    // Use timeout - if it times out, request was buffered
                    // Timeout must exceed retry strategy (~700ms) so requests reach buffer.enqueue()
                    let result = tokio::time::timeout(
                        Duration::from_millis(2000),
                        make_request(router, "bench_query"),
                    )
                    .await;

                    if result.is_err() {
                        buffered_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        })
        .collect();

    futures::future::join_all(buffer_handles).await;

    let actual_buffered = buffer.len();
    println!(
        "  Buffered {} requests (buffer.len() = {})",
        format_num(buffered_count.load(Ordering::Relaxed)),
        format_num(actual_buffered)
    );

    // Phase 2: Make DB healthy and process buffer
    println!("  Phase 2: Recovery - making DB healthy...");
    healthy_flag.store(true, Ordering::SeqCst);

    // Spawn buffer processor
    let buffer_clone = Arc::clone(&buffer);
    let grpc_config = GrpcConfig::default().with_backend_addr(format!("http://{}", addr));
    let client = helix_gateway::client::ProtoClient::connect(&grpc_config)
        .await
        .unwrap();

    let processor = tokio::spawn(async move {
        process_buffer(buffer_clone, Duration::from_secs(60), &client, None).await
    });

    // Phase 3: Send more requests while draining
    println!(
        "  Phase 3: Sending {} more requests while draining...",
        half_requests
    );

    let drain_handles: Vec<_> = (0..CONCURRENCY)
        .map(|_| {
            let state = state.clone();
            let histogram = Arc::clone(&histogram);
            let successful = Arc::clone(&successful);

            tokio::spawn(async move {
                let mut local_histogram = Histogram::<u64>::new(3).unwrap();

                for _ in 0..requests_per_worker {
                    let router = create_router().with_state(state.clone());
                    let (status, latency) = make_request(router, "bench_query").await;

                    if status == StatusCode::OK {
                        successful.fetch_add(1, Ordering::Relaxed);
                        local_histogram.record(latency.as_micros() as u64).ok();
                    }
                }

                histogram.lock().await.add(&local_histogram).ok();
            })
        })
        .collect();

    futures::future::join_all(drain_handles).await;
    let processed = processor.await.unwrap().unwrap_or(0);
    println!(
        "  Buffer processor drained {} requests",
        format_num(processed)
    );

    let duration = start.elapsed();

    BenchmarkResult {
        scenario: "Recovery (Unhealthy → Healthy)".to_string(),
        total_requests: TOTAL_REQUESTS,
        successful: successful.load(Ordering::Relaxed),
        failed: buffered_count.load(Ordering::Relaxed),
        duration,
        latency_histogram: Arc::try_unwrap(histogram).unwrap().into_inner(),
    }
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    println!();
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║     Gateway Performance Benchmark - 1M Requests @ 10K        ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();

    // Scenario 1: Healthy Gateway
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Running: Healthy Gateway Benchmark");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    let healthy_result = run_healthy_benchmark().await;
    healthy_result.print();

    // Scenario 2: Unhealthy Gateway (Buffering)
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Running: Unhealthy Gateway Benchmark (Buffering)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    let unhealthy_result = run_unhealthy_benchmark().await;
    unhealthy_result.print();

    // Scenario 3: Recovery
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Running: Recovery Benchmark (Unhealthy → Healthy)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    let recovery_result = run_recovery_benchmark().await;
    recovery_result.print();

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Benchmark Complete!");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}
