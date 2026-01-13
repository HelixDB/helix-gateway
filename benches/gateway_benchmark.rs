// Copyright 2026 HelixDB
// SPDX-License-Identifier: Apache-2.0

//! Gateway benchmarks for stress testing and chaos/resilience testing.
//!
//! This benchmark suite includes:
//! 1. Stress test - Tests the gateway at different concurrency levels
//! 2. Duration-based throughput test - Measures sustained throughput over time
//! 3. Chaos/resilience test - Tests gateway behavior during backend failures

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use hdrhistogram::Histogram;
use http_body_util::BodyExt;
use rand::Rng;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tonic::transport::Server;
use tonic::{Response, Status};
use tower::ServiceExt;

use helix_gateway::db::{
    BackendService, BackendServiceServer, HealthRequest, HealthResponse, QueryRequest,
    QueryResponse, RequestType,
};
use helix_gateway::gateway::buffer::Buffer;
use helix_gateway::gateway::introspection::{DbQuery, Introspection};
use helix_gateway::gateway::routes::{create_router, process_buffer};
use helix_gateway::gateway::state::AppState;
use helix_gateway::gateway::{DbStatus, ProtoClient};
use helix_gateway::{Config, Format, GrpcConfig};

// ============================================================================
// Mock Backend Infrastructure
// ============================================================================

/// Dynamic mock backend service that can switch between healthy/unhealthy states.
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
        self.request_count.fetch_add(1, Ordering::SeqCst);

        if !self.healthy.load(Ordering::SeqCst) {
            return Err(Status::unavailable("database unavailable"));
        }

        let req = request.into_inner();
        Ok(Response::new(QueryResponse {
            data: format!("Query result for: {}", req.query).into_bytes(),
            status: 200,
        }))
    }

    async fn health(
        &self,
        _request: tonic::Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        if !self.healthy.load(Ordering::SeqCst) {
            return Err(Status::unavailable("database unavailable"));
        }

        Ok(Response::new(HealthResponse {
            healthy: true,
            version: "1.0.0-bench".to_string(),
        }))
    }
}

/// Starts a dynamic mock gRPC server.
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

// ============================================================================
// Test Helpers
// ============================================================================

/// Creates test queries for benchmarks.
fn create_test_queries() -> Introspection {
    let mut queries = HashMap::new();
    queries.insert("query".to_string(), test_query(RequestType::Read));
    queries.insert("get_users".to_string(), test_query(RequestType::Read));
    for i in 0..100 {
        queries.insert(format!("test_query_{}", i), test_query(RequestType::Read));
    }
    Introspection {
        queries,
        schema: HashMap::new(),
    }
}

/// Creates a test DbQuery with default values.
fn test_query(request_type: RequestType) -> DbQuery {
    DbQuery {
        request_type,
        parameters: pbjson_types::Struct::default(),
        return_types: pbjson_types::Struct::default(),
        embedding_config: None,
    }
}

/// Creates an AppState with a properly configured buffer and watcher.
async fn create_test_app_state_with_buffer(
    backend_addr: &str,
) -> (
    AppState,
    Arc<Buffer>,
    tokio::sync::watch::Receiver<DbStatus>,
) {
    let grpc_config = GrpcConfig::default().with_backend_addr(backend_addr);
    let client = ProtoClient::connect(&grpc_config)
        .await
        .expect("Failed to connect to mock backend");

    let (tx, mut rx) = tokio::sync::watch::channel(DbStatus::Healthy);
    rx.mark_changed();

    let buffer = Arc::new(
        Buffer::new()
            .max_size(100_000)
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

/// Makes a single request to the router.
async fn make_request(router: Router, query_name: &str) -> (StatusCode, Duration) {
    let body = format!(
        r#"{{"request_type": 0, "query": "{}", "parameters": {{}}}}"#,
        query_name
    );

    let request = Request::builder()
        .method("POST")
        .uri(format!("/{}", query_name))
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let start = Instant::now();
    let response = router.oneshot(request).await.unwrap();
    let latency = start.elapsed();
    let status = response.status();

    // Consume the body
    let _ = response.into_body().collect().await;

    (status, latency)
}

// ============================================================================
// Formatting Helpers
// ============================================================================

fn format_duration(d: Duration) -> String {
    if d.as_micros() < 1000 {
        format!("{:.2}us", d.as_nanos() as f64 / 1000.0)
    } else if d.as_millis() < 1000 {
        format!("{:.2}ms", d.as_micros() as f64 / 1000.0)
    } else {
        format!("{:.2}s", d.as_millis() as f64 / 1000.0)
    }
}

fn format_throughput(requests: usize, duration: Duration) -> String {
    let rps = requests as f64 / duration.as_secs_f64();
    if rps >= 1_000_000.0 {
        format!("{:.2}M req/s", rps / 1_000_000.0)
    } else if rps >= 1_000.0 {
        format!("{:.2}K req/s", rps / 1_000.0)
    } else {
        format!("{:.2} req/s", rps)
    }
}

// ============================================================================
// Benchmark 1: Stress Test (fixed request count)
// ============================================================================

async fn bench_healthy_gateway_stress(workers: usize, requests_per_worker: usize) {
    let total_requests = workers * requests_per_worker;
    println!("\n{}", "=".repeat(60));
    println!(
        "STRESS TEST: {} workers x {} requests = {} total",
        workers, requests_per_worker, total_requests
    );
    println!("{}", "=".repeat(60));

    // Setup mock backend
    let (service, _healthy_flag, request_count) = DynamicMockBackendService::new();
    let (addr, _shutdown) = start_mock_grpc_server(service).await;

    // Setup state with buffer
    let (state, _buffer, _rx) =
        create_test_app_state_with_buffer(&format!("http://{}", addr)).await;

    // Counters
    let successes = Arc::new(AtomicUsize::new(0));
    let failures = Arc::new(AtomicUsize::new(0));

    let start = Instant::now();

    // Create router once
    let router = create_router().with_state(state);

    let handles: Vec<_> = (0..workers)
        .map(|worker_id| {
            let router = router.clone();
            let successes = Arc::clone(&successes);
            let failures = Arc::clone(&failures);

            tokio::spawn(async move {
                let mut local_histogram = Histogram::<u64>::new(3).unwrap();
                let query_name = format!("test_query_{}", worker_id % 100);

                for _ in 0..requests_per_worker {
                    let (status, latency) = make_request(router.clone(), &query_name).await;

                    if status == StatusCode::OK {
                        successes.fetch_add(1, Ordering::Relaxed);
                        let _ = local_histogram.record(latency.as_micros() as u64);
                    } else {
                        failures.fetch_add(1, Ordering::Relaxed);
                    }
                }

                local_histogram
            })
        })
        .collect();

    // Collect results
    let mut histogram = Histogram::<u64>::new(3).unwrap();
    for handle in handles {
        let local = handle.await.unwrap();
        histogram.add(&local).unwrap();
    }

    let elapsed = start.elapsed();
    let total_successes = successes.load(Ordering::Relaxed);
    let total_failures = failures.load(Ordering::Relaxed);
    let backend_requests = request_count.load(Ordering::SeqCst);

    // Report results
    println!("\nResults:");
    println!("  Duration:         {}", format_duration(elapsed));
    println!(
        "  Throughput:       {}",
        format_throughput(total_successes, elapsed)
    );
    println!(
        "  Requests:         {} succeeded, {} failed",
        total_successes, total_failures
    );
    println!("  Backend calls:    {}", backend_requests);
    println!("\nLatency Distribution:");
    println!(
        "  p50:              {}",
        format_duration(Duration::from_micros(histogram.value_at_quantile(0.50)))
    );
    println!(
        "  p95:              {}",
        format_duration(Duration::from_micros(histogram.value_at_quantile(0.95)))
    );
    println!(
        "  p99:              {}",
        format_duration(Duration::from_micros(histogram.value_at_quantile(0.99)))
    );
    println!(
        "  max:              {}",
        format_duration(Duration::from_micros(histogram.max()))
    );
}

// ============================================================================
// Benchmark 1b: Duration-based Throughput Test
// ============================================================================

async fn bench_healthy_gateway_duration(workers: usize, duration_secs: u64) {
    println!("\n{}", "=".repeat(60));
    println!(
        "THROUGHPUT TEST: {} workers for {}s",
        workers, duration_secs
    );
    println!("{}", "=".repeat(60));

    // Setup mock backend
    let (service, _healthy_flag, request_count) = DynamicMockBackendService::new();
    let (addr, _shutdown) = start_mock_grpc_server(service).await;

    // Setup state with buffer
    let (state, _buffer, _rx) =
        create_test_app_state_with_buffer(&format!("http://{}", addr)).await;

    // Create router once
    let router = create_router().with_state(state);

    // Shared counters and histogram
    let successes = Arc::new(AtomicUsize::new(0));
    let failures = Arc::new(AtomicUsize::new(0));
    let shared_histogram = Arc::new(std::sync::Mutex::new(Histogram::<u64>::new(3).unwrap()));
    let running = Arc::new(AtomicBool::new(true));

    let start = Instant::now();
    let test_duration = Duration::from_secs(duration_secs);

    // Spawn workers
    let handles: Vec<_> = (0..workers)
        .map(|worker_id| {
            let router = router.clone();
            let successes = Arc::clone(&successes);
            let failures = Arc::clone(&failures);
            let histogram = Arc::clone(&shared_histogram);
            let running = Arc::clone(&running);

            tokio::spawn(async move {
                let query_name = format!("test_query_{}", worker_id % 100);

                while running.load(Ordering::Relaxed) {
                    let (status, latency) = make_request(router.clone(), &query_name).await;

                    if status == StatusCode::OK {
                        successes.fetch_add(1, Ordering::Relaxed);
                        if let Ok(mut hist) = histogram.lock() {
                            let _ = hist.record(latency.as_micros() as u64);
                        }
                    } else {
                        failures.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        })
        .collect();

    // Wait for test duration
    tokio::time::sleep(test_duration).await;
    running.store(false, Ordering::SeqCst);

    // Wait for workers to finish their current request
    for handle in handles {
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
    }

    let elapsed = start.elapsed();
    let total_successes = successes.load(Ordering::Relaxed);
    let total_failures = failures.load(Ordering::Relaxed);
    let backend_requests = request_count.load(Ordering::SeqCst);
    let histogram = shared_histogram.lock().unwrap();

    // Report results
    println!("\nResults:");
    println!("  Duration:         {}", format_duration(elapsed));
    println!(
        "  Throughput:       {}",
        format_throughput(total_successes, elapsed)
    );
    println!(
        "  Requests:         {} succeeded, {} failed",
        total_successes, total_failures
    );
    println!("  Backend calls:    {}", backend_requests);
    println!("\nLatency Distribution:");
    println!(
        "  p50:              {}",
        format_duration(Duration::from_micros(histogram.value_at_quantile(0.50)))
    );
    println!(
        "  p95:              {}",
        format_duration(Duration::from_micros(histogram.value_at_quantile(0.95)))
    );
    println!(
        "  p99:              {}",
        format_duration(Duration::from_micros(histogram.value_at_quantile(0.99)))
    );
    println!(
        "  max:              {}",
        format_duration(Duration::from_micros(histogram.max()))
    );
}

// ============================================================================
// Benchmark 2: Chaos/Resilience Test
// ============================================================================

async fn bench_chaos_resilience(workers: usize, duration_secs: u64) {
    let mut rng = rand::rng();
    let chaos_delay_secs: u64 = rng.random_range(5..=15);
    let recovery_delay_secs: u64 = rng.random_range(1..=30);

    println!("\n{}", "=".repeat(60));
    println!("CHAOS TEST: {} workers for {}s", workers, duration_secs);
    println!(
        "  Chaos at: {}s, Recovery after: {}s",
        chaos_delay_secs, recovery_delay_secs
    );
    println!("{}", "=".repeat(60));

    // Setup mock backend
    let (service, healthy_flag, request_count) = DynamicMockBackendService::new();
    let (addr, _shutdown) = start_mock_grpc_server(service).await;

    // Setup state with buffer
    let (state, buffer, mut rx) =
        create_test_app_state_with_buffer(&format!("http://{}", addr)).await;

    // Setup gRPC client for buffer processor
    let grpc_config = GrpcConfig::default().with_backend_addr(format!("http://{}", addr));
    let grpc_client = ProtoClient::connect(&grpc_config).await.unwrap();

    // Spawn buffer processor (like GatewayBuilder::run does)
    let buffer_for_processor = Arc::clone(&buffer);
    let grpc_client_for_processor = grpc_client.clone();
    tokio::spawn(async move {
        loop {
            let _ = rx.changed().await;
            if *rx.borrow() == DbStatus::Unhealthy {
                continue;
            }
            let _ = process_buffer(
                Arc::clone(&buffer_for_processor),
                Duration::from_secs(60),
                &grpc_client_for_processor,
                None,
            )
            .await;
        }
    });

    // Phase tracking
    let test_start = Instant::now();
    let test_duration = Duration::from_secs(duration_secs);
    let running = Arc::new(AtomicBool::new(true));

    // Metrics
    let healthy_requests = Arc::new(AtomicUsize::new(0));
    let unhealthy_requests = Arc::new(AtomicUsize::new(0));
    let recovery_requests = Arc::new(AtomicUsize::new(0));
    let total_successes = Arc::new(AtomicUsize::new(0));
    let total_failures = Arc::new(AtomicUsize::new(0));

    // Phase markers
    let chaos_started = Arc::new(AtomicBool::new(false));
    let recovery_done = Arc::new(AtomicBool::new(false));

    // Shared histogram (so we don't lose data when workers are aborted)
    let shared_histogram = Arc::new(std::sync::Mutex::new(Histogram::<u64>::new(3).unwrap()));

    // Chaos controller
    let healthy_flag_clone = Arc::clone(&healthy_flag);
    let chaos_started_clone = Arc::clone(&chaos_started);
    let recovery_done_clone = Arc::clone(&recovery_done);
    let buffer_for_chaos = Arc::clone(&buffer);
    let chaos_delay = Duration::from_secs(chaos_delay_secs);
    let recovery_delay = Duration::from_secs(recovery_delay_secs);

    tokio::spawn(async move {
        // Wait for chaos
        tokio::time::sleep(chaos_delay).await;
        healthy_flag_clone.store(false, Ordering::SeqCst);
        chaos_started_clone.store(true, Ordering::SeqCst);
        println!(
            "\n[{:.2}s] Backend went UNHEALTHY",
            test_start.elapsed().as_secs_f64()
        );

        // Wait for recovery
        tokio::time::sleep(recovery_delay).await;
        healthy_flag_clone.store(true, Ordering::SeqCst);
        recovery_done_clone.store(true, Ordering::SeqCst);
        // Manually trigger buffer watcher to wake up buffer processor
        // (since all workers may be blocked waiting on buffered requests)
        let _ = buffer_for_chaos.update_watcher(DbStatus::Healthy);
        println!(
            "[{:.2}s] Backend RECOVERED",
            test_start.elapsed().as_secs_f64()
        );
    });

    // Create router once
    let router = create_router().with_state(state);

    // Spawn workers
    let handles: Vec<_> = (0..workers)
        .map(|worker_id| {
            let router = router.clone();
            let running = Arc::clone(&running);
            let healthy_requests = Arc::clone(&healthy_requests);
            let unhealthy_requests = Arc::clone(&unhealthy_requests);
            let recovery_requests = Arc::clone(&recovery_requests);
            let total_successes = Arc::clone(&total_successes);
            let total_failures = Arc::clone(&total_failures);
            let chaos_started = Arc::clone(&chaos_started);
            let recovery_done = Arc::clone(&recovery_done);
            let histogram = Arc::clone(&shared_histogram);

            tokio::spawn(async move {
                let query_name = format!("test_query_{}", worker_id % 100);

                while running.load(Ordering::Relaxed) {
                    let (status, latency) = make_request(router.clone(), &query_name).await;

                    // Track which phase this request was in
                    if !chaos_started.load(Ordering::Relaxed) {
                        healthy_requests.fetch_add(1, Ordering::Relaxed);
                    } else if !recovery_done.load(Ordering::Relaxed) {
                        unhealthy_requests.fetch_add(1, Ordering::Relaxed);
                    } else {
                        recovery_requests.fetch_add(1, Ordering::Relaxed);
                    }

                    if status == StatusCode::OK {
                        total_successes.fetch_add(1, Ordering::Relaxed);
                        if let Ok(mut hist) = histogram.lock() {
                            let _ = hist.record(latency.as_micros() as u64);
                        }
                    } else {
                        total_failures.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        })
        .collect();

    // Wait for test duration
    tokio::time::sleep(test_duration).await;
    running.store(false, Ordering::SeqCst);

    // Abort all worker tasks to cancel any stuck awaits on buffered requests
    for handle in &handles {
        handle.abort();
    }

    // Wait briefly for aborts to complete
    for handle in handles {
        let _ = tokio::time::timeout(Duration::from_millis(100), handle).await;
    }

    let elapsed = test_start.elapsed();

    // Get histogram from shared state
    let histogram = shared_histogram.lock().unwrap();
    let backend_requests = request_count.load(Ordering::SeqCst);
    let buffered_max = buffer.len();

    // Report results
    println!("\nResults:");
    println!("  Duration:              {}", format_duration(elapsed));
    println!(
        "  Total requests:        {}",
        total_successes.load(Ordering::Relaxed) + total_failures.load(Ordering::Relaxed)
    );
    println!(
        "  Succeeded:             {}",
        total_successes.load(Ordering::Relaxed)
    );
    println!(
        "  Failed:                {}",
        total_failures.load(Ordering::Relaxed)
    );
    println!("  Backend calls:         {}", backend_requests);
    println!("  Buffer remaining:      {}", buffered_max);
    println!("\nPhase breakdown:");
    println!(
        "  Healthy phase:         {} requests",
        healthy_requests.load(Ordering::Relaxed)
    );
    println!(
        "  Unhealthy phase:       {} requests",
        unhealthy_requests.load(Ordering::Relaxed)
    );
    println!(
        "  Recovery phase:        {} requests",
        recovery_requests.load(Ordering::Relaxed)
    );
    println!("\nLatency Distribution (successful requests):");
    if histogram.len() > 0 {
        println!(
            "  p50:                   {}",
            format_duration(Duration::from_micros(histogram.value_at_quantile(0.50)))
        );
        println!(
            "  p95:                   {}",
            format_duration(Duration::from_micros(histogram.value_at_quantile(0.95)))
        );
        println!(
            "  p99:                   {}",
            format_duration(Duration::from_micros(histogram.value_at_quantile(0.99)))
        );
        println!(
            "  max:                   {}",
            format_duration(Duration::from_micros(histogram.max()))
        );
    } else {
        println!("  No successful requests recorded");
    }
}

// ============================================================================
// Main
// ============================================================================

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    println!("\n");
    println!("########################################################");
    println!("#          HELIX GATEWAY BENCHMARK SUITE               #");
    println!("########################################################");

    // Run stress tests at different concurrency levels
    rt.block_on(bench_healthy_gateway_stress(100, 100));
    rt.block_on(bench_healthy_gateway_stress(1000, 100));
    rt.block_on(bench_healthy_gateway_stress(10000, 100));

    // Run duration-based throughput test
    rt.block_on(bench_healthy_gateway_duration(1000, 30));

    // Run chaos/resilience test
    rt.block_on(bench_chaos_resilience(500, 60));

    println!("\n");
    println!("########################################################");
    println!("#               BENCHMARK COMPLETE                     #");
    println!("########################################################");
    println!("\n");
}
