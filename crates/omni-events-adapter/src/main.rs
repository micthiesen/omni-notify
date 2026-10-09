//! `executor-events-adapter` binary.
//!
//! `executor-events-adapter` serves on `0.0.0.0:$PORT`;
//! `executor-events-adapter healthcheck` probes `/health` for the container
//! health check and exits 0 or 1.

use std::io::IsTerminal as _;
use std::process::ExitCode;
use std::time::Duration;

use omni_events_adapter::AdapterBuilder;
use omni_events_adapter::config::{EnvConfig, port_from};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(3);
/// Long-lived legacy event streams must not hold a container stop open.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .with_ansi(std::io::stdout().is_terminal())
        .init();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(%error, "could not start the runtime");
            return ExitCode::FAILURE;
        }
    };
    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => runtime.block_on(serve()),
        Some("healthcheck") => runtime.block_on(healthcheck()),
        Some(other) => {
            tracing::error!(command = other, "unknown command; use serve or healthcheck");
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> ExitCode {
    let config = match EnvConfig::from_env(|key| std::env::var(key).ok()) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "invalid configuration");
            return ExitCode::FAILURE;
        }
    };
    let adapter = match AdapterBuilder::new(config.options).build() {
        Ok(adapter) => adapter,
        Err(error) => {
            tracing::error!(%error, "could not build the adapter");
            return ExitCode::FAILURE;
        }
    };
    let listener = match TcpListener::bind(("0.0.0.0", config.port)).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(%error, port = config.port, "could not listen");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(port = config.port, "executor events adapter listening");
    let stop = CancellationToken::new();
    let signal = stop.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        signal.cancel();
    });
    let served = adapter.serve(listener, stop.clone().cancelled_owned());
    let result = tokio::select! {
        result = served => result,
        () = async {
            stop.cancelled().await;
            tokio::time::sleep(DRAIN_TIMEOUT).await;
        } => Ok(()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "server failed");
            ExitCode::FAILURE
        }
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = terminate.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn healthcheck() -> ExitCode {
    let Ok(port) = port_from(std::env::var("PORT").ok()) else {
        return ExitCode::FAILURE;
    };
    let Ok(client) = reqwest::Client::builder()
        .no_proxy()
        .timeout(HEALTHCHECK_TIMEOUT)
        .build()
    else {
        return ExitCode::FAILURE;
    };
    match client
        .get(format!("http://127.0.0.1:{port}/health"))
        .header(reqwest::header::USER_AGENT, omni_events_adapter::USER_AGENT)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}
