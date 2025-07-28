## Motivation

This PR implements the direct Axum migration from actix-web for the SGLang router component. The migration aims to:
- Simplify the codebase by using a more idiomatic Rust web framework
- Improve maintainability and reduce complexity
- Enable better async/await support and middleware patterns
- Leverage Axum's integration with the Tower ecosystem

## Modifications

### Framework Migration
- Replaced actix-web with Axum framework for all HTTP server functionality
- Converted all request handlers to use Axum's extractor pattern
- Migrated from actix-web middleware to Tower-based middleware stack

### Middleware Stack Implementation
- Implemented custom logging middleware with request ID tracking and latency metrics
- Added CORS support using tower-http
- Integrated request body size limiting
- Implemented graceful shutdown handling

### Python Bindings Updates
- Updated PyO3 bindings to support new parameters required by the Axum implementation
- Fixed Python wrapper class to include all necessary parameters:
  - `bootstrap_port_annotation`
  - `request_timeout_secs`
  - `max_concurrent_requests`
  - `cors_allowed_origins`

### Refactoring to Remove Duplicate Async Methods
- Removed duplicate `create_router_async`, `new_async` methods to eliminate maintenance burden
- Unified approach using synchronous methods that handle both sync and async contexts
- Updated tests to use `spawn_blocking` for router creation in async contexts
- Improved `wait_for_healthy_workers` to detect execution context and handle appropriately

### Key Changes by File

**`src/server.rs`**
- Converted from actix-web App to Axum Router
- Implemented proper state management with AppState
- Added custom middleware stack with request ID and logging support
- Fixed logging middleware to use custom implementation instead of default TraceLayer
- Migrated all route handlers to use Axum extractors

**`src/middleware.rs`**
- Created comprehensive logging middleware with:
  - Request ID generation and tracking
  - Latency measurement
  - Structured logging with proper span context
  - OpenAI-compatible request ID format

**`src/lib.rs`**
- Updated PyO3 bindings to match new server implementation
- Added new configuration parameters for middleware

**Python Integration**
- Updated `launch_router.py` to pass new required parameters
- Fixed `router.py` wrapper class to include all parameters expected by Rust

### PD Disaggregation Support
- Maintained full compatibility with PD (Prefill-Decode) disaggregated routing mode
- Preserved all PD-specific configuration options and service discovery

## Checklist

- [x] Format your code according to the [Code Formatting with Pre-Commit](https://docs.sglang.ai/references/contribution_guide.html#code-formatting-with-pre-commit).
- [x] Add unit tests as outlined in the [Running Unit Tests](https://docs.sglang.ai/references/contribution_guide.html#running-unit-tests-adding-to-ci).
- [ ] Update documentation / docstrings / example tutorials as needed, according to [Writing Documentation](https://docs.sglang.ai/references/contribution_guide.html#writing-documentation-running-docs-ci).
- [ ] Provide throughput / latency benchmark results and accuracy evaluation results as needed, according to [Benchmark and Profiling](https://docs.sglang.ai/references/benchmark_and_profiling.html) and [Accuracy Results](https://docs.sglang.ai/references/accuracy_evaluation.html).
- [ ] For reviewers: If you haven't made any contributions to this PR and are only assisting with merging the main branch, please remove yourself as a co-author when merging the PR.
- [ ] Please feel free to join our Slack channel at https://slack.sglang.ai to discuss your PR.
