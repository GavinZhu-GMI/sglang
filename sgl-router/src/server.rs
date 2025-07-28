use crate::config::RouterConfig;
use crate::logging::{self, LoggingConfig};
// Logging is handled via tracing middleware
use crate::metrics::{self, PrometheusConfig};
// Request ID is handled by middleware
use crate::openai_api_types::{ChatCompletionRequest, CompletionRequest, GenerateRequest};
use crate::routers::{RouterFactory, RouterTrait};
use crate::service_discovery::{start_service_discovery, ServiceDiscoveryConfig};
use axum::{
    body::Body,
    extract::{Query, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use reqwest::Client;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::spawn;
// Tower middleware is handled via Axum
// Compression is handled in the router setup
// CORS is handled in the router setup
// Request body limit is handled in the router setup
// Timeout is handled in the router setup
use tracing::{error, info, warn, Level};

#[derive(Clone)]
pub struct AppState {
    router: Arc<dyn RouterTrait>,
    client: Client,
    _concurrency_limiter: Arc<tokio::sync::Semaphore>,
}

impl AppState {
    pub fn new(
        router_config: RouterConfig,
        client: Client,
        max_concurrent_requests: usize,
    ) -> Result<Self, String> {
        let router = RouterFactory::create_router(&router_config)?;
        let router = Arc::from(router);
        let concurrency_limiter = Arc::new(tokio::sync::Semaphore::new(max_concurrent_requests));
        Ok(Self {
            router,
            client,
            _concurrency_limiter: concurrency_limiter,
        })
    }
}

// Fallback handler for unmatched routes
async fn sink_handler() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

// Health check endpoints
async fn liveness(State(state): State<Arc<AppState>>) -> Response {
    state.router.liveness()
}

async fn readiness(State(state): State<Arc<AppState>>) -> Response {
    state.router.readiness()
}

async fn health(State(state): State<Arc<AppState>>) -> Response {
    let req = Request::builder().body(Body::empty()).unwrap();
    state.router.health(&state.client, req).await
}

async fn health_generate(State(state): State<Arc<AppState>>) -> Response {
    let req = Request::builder().body(Body::empty()).unwrap();
    state.router.health_generate(&state.client, req).await
}

async fn get_server_info(State(state): State<Arc<AppState>>) -> Response {
    let req = Request::builder().body(Body::empty()).unwrap();
    state.router.get_server_info(&state.client, req).await
}

async fn v1_models(State(state): State<Arc<AppState>>) -> Response {
    let req = Request::builder().body(Body::empty()).unwrap();
    state.router.get_models(&state.client, req).await
}

async fn get_model_info(State(state): State<Arc<AppState>>) -> Response {
    let req = Request::builder().body(Body::empty()).unwrap();
    state.router.get_model_info(&state.client, req).await
}

// Generation endpoints
async fn generate(
    State(state): State<Arc<AppState>>,
    Json(body): Json<GenerateRequest>,
) -> Response {
    let req = Request::builder()
        .method("POST")
        .uri("/generate")
        .body(Body::empty())
        .unwrap();

    let json_body = serde_json::to_value(body).unwrap();
    state
        .router
        .route_generate(&state.client, req, json_body)
        .await
}

async fn v1_chat_completions(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ChatCompletionRequest>,
) -> Response {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .body(Body::empty())
        .unwrap();

    let json_body = serde_json::to_value(body).unwrap();
    state.router.route_chat(&state.client, req, json_body).await
}

async fn v1_completions(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CompletionRequest>,
) -> Response {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/completions")
        .body(Body::empty())
        .unwrap();

    let json_body = serde_json::to_value(body).unwrap();
    state
        .router
        .route_completion(&state.client, req, json_body)
        .await
}

// Worker management endpoints
async fn add_worker(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let worker_url = match params.get("url") {
        Some(url) => url.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "Worker URL required. Provide 'url' query parameter",
            )
                .into_response();
        }
    };

    match state.router.add_worker(&worker_url).await {
        Ok(message) => (StatusCode::OK, message).into_response(),
        Err(error) => (StatusCode::BAD_REQUEST, error).into_response(),
    }
}

async fn list_workers(State(state): State<Arc<AppState>>) -> Response {
    let worker_list = state.router.get_worker_urls();
    Json(serde_json::json!({ "urls": worker_list })).into_response()
}

async fn remove_worker(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let worker_url = match params.get("url") {
        Some(url) => url.to_string(),
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    state.router.remove_worker(&worker_url);
    (
        StatusCode::OK,
        format!("Successfully removed worker: {}", worker_url),
    )
        .into_response()
}

async fn flush_cache(State(state): State<Arc<AppState>>) -> Response {
    state.router.flush_cache(&state.client).await
}

async fn get_loads(State(state): State<Arc<AppState>>) -> Response {
    state.router.get_worker_loads(&state.client).await
}

pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub router_config: RouterConfig,
    pub max_payload_size: usize,
    pub log_dir: Option<String>,
    pub log_level: Option<String>,
    pub service_discovery_config: Option<ServiceDiscoveryConfig>,
    pub prometheus_config: Option<PrometheusConfig>,
    pub request_timeout_secs: u64,
    pub request_id_headers: Option<Vec<String>>,
}

pub async fn startup(config: ServerConfig) -> Result<(), Box<dyn std::error::Error>> {
    // Only initialize logging if not already done (for Python bindings support)
    static LOGGING_INITIALIZED: AtomicBool = AtomicBool::new(false);

    let _log_guard = if !LOGGING_INITIALIZED.swap(true, Ordering::SeqCst) {
        Some(logging::init_logging(LoggingConfig {
            level: config
                .log_level
                .as_deref()
                .and_then(|s| match s.to_uppercase().parse::<Level>() {
                    Ok(l) => Some(l),
                    Err(_) => {
                        warn!("Invalid log level string: '{}'. Defaulting to INFO.", s);
                        None
                    }
                })
                .unwrap_or(Level::INFO),
            json_format: false,
            log_dir: config.log_dir.clone(),
            colorize: true,
            log_file_name: "sgl-router".to_string(),
            log_targets: None,
        }))
    } else {
        None
    };

    // Initialize prometheus metrics exporter
    if let Some(prometheus_config) = config.prometheus_config {
        metrics::start_prometheus(prometheus_config);
    }

    info!(
        "Starting router on {}:{} | mode: {:?} | policy: {:?} | max_payload: {}MB",
        config.host,
        config.port,
        config.router_config.mode,
        config.router_config.policy,
        config.max_payload_size / (1024 * 1024)
    );

    let client = Client::builder()
        .pool_idle_timeout(Some(Duration::from_secs(50)))
        .timeout(Duration::from_secs(config.request_timeout_secs))
        .build()
        .expect("Failed to create HTTP client");

    let app_state = Arc::new(AppState::new(
        config.router_config.clone(),
        client.clone(),
        config.router_config.max_concurrent_requests,
    )?);
    let router_arc = Arc::clone(&app_state.router);

    // Start the service discovery if enabled
    if let Some(service_discovery_config) = config.service_discovery_config {
        if service_discovery_config.enabled {
            match start_service_discovery(service_discovery_config, router_arc).await {
                Ok(handle) => {
                    info!("Service discovery started");
                    // Spawn a task to handle the service discovery thread
                    spawn(async move {
                        if let Err(e) = handle.await {
                            error!("Service discovery task failed: {:?}", e);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to start service discovery: {}", e);
                    warn!("Continuing without service discovery");
                }
            }
        }
    }

    info!(
        "Router ready | workers: {:?}",
        app_state.router.get_worker_urls()
    );

    // Configure request ID headers
    let request_id_headers = config.request_id_headers.clone().unwrap_or_else(|| {
        vec![
            "x-request-id".to_string(),
            "x-correlation-id".to_string(),
            "x-trace-id".to_string(),
            "request-id".to_string(),
        ]
    });

    // Create routes
    let protected_routes = Router::new()
        .route("/generate", post(generate))
        .route("/v1/chat/completions", post(v1_chat_completions))
        .route("/v1/completions", post(v1_completions));

    let public_routes = Router::new()
        .route("/liveness", get(liveness))
        .route("/readiness", get(readiness))
        .route("/health", get(health))
        .route("/health_generate", get(health_generate))
        .route("/v1/models", get(v1_models))
        .route("/get_model_info", get(get_model_info))
        .route("/get_server_info", get(get_server_info));

    let admin_routes = Router::new()
        .route("/add_worker", post(add_worker))
        .route("/remove_worker", post(remove_worker))
        .route("/list_workers", get(list_workers))
        .route("/flush_cache", post(flush_cache))
        .route("/get_loads", get(get_loads));

    // Build app with all routes and middleware
    let app = Router::new()
        .merge(protected_routes)
        .merge(public_routes)
        .merge(admin_routes)
        // Request body size limiting
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            config.max_payload_size,
        ))
        // Custom logging layer that includes request IDs
        .layer(crate::middleware::create_logging_layer())
        // Request ID layer must come before the logging layer
        .layer(crate::middleware::RequestIdLayer::new(request_id_headers))
        // CORS (should be last)
        .layer(create_cors_layer(
            config.router_config.cors_allowed_origins.clone(),
        ))
        // Fallback
        .fallback(sink_handler)
        // State - apply last to get Router<Arc<AppState>>
        .with_state(app_state);

    // Create TCP listener - use the configured host
    let addr = format!("{}:{}", config.host, config.port);
    let listener = TcpListener::bind(&addr).await?;

    // Start server with graceful shutdown
    info!("Starting server on {}", addr);

    // Serve the application with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    Ok(())
}

// Graceful shutdown handler
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received Ctrl+C, starting graceful shutdown");
        },
        _ = terminate => {
            info!("Received terminate signal, starting graceful shutdown");
        },
    }
}

// CORS Layer Creation
fn create_cors_layer(allowed_origins: Vec<String>) -> tower_http::cors::CorsLayer {
    use tower_http::cors::Any;

    let cors = if allowed_origins.is_empty() {
        // Allow all origins if none specified
        tower_http::cors::CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
            .expose_headers(Any)
    } else {
        // Restrict to specific origins
        let origins: Vec<http::HeaderValue> = allowed_origins
            .into_iter()
            .filter_map(|origin| origin.parse().ok())
            .collect();

        tower_http::cors::CorsLayer::new()
            .allow_origin(origins)
            .allow_methods([http::Method::GET, http::Method::POST, http::Method::OPTIONS])
            .allow_headers([http::header::CONTENT_TYPE, http::header::AUTHORIZATION])
            .expose_headers([http::header::HeaderName::from_static("x-request-id")])
    };

    cors.max_age(Duration::from_secs(3600))
}
