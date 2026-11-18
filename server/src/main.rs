use harmony_server::{AppState, app, auth, config::Config};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = Config::from_env()?;
    let bind = config.bind;
    let state = AppState::new(config).await?;

    if let Some(code) = auth::bootstrap_invite(&state.db).await? {
        // Printed straight to the console, not through the logger, so it does
        // not end up in log files.
        println!("\nNo accounts exist yet. Create the admin account by opening:");
        println!("    <your Harmony address>/#/invite/{code}");
        println!("This invite works once and expires in 7 days.\n");
    }

    if state.config.livekit.is_none() {
        tracing::info!("voice calls are off: HARMONY_LIVEKIT_URL is not set");
    }
    tokio::spawn(harmony_server::calls::sweep_forever(state.clone()));

    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!("Harmony listening on http://{bind}");
    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
