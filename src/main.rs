use irc_mcp::{config::Config, mcp::TOOLS_JSON, relay::Relay, service::Service};
use std::{path::Path, sync::Arc};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--print-tools"] {
        println!("{TOOLS_JSON}");
        return Ok(());
    }
    if args.len() != 2 || args[0] != "--config" {
        return Err("Usage: irc-mcp --config /path/config.toml | --print-tools".into());
    }
    let cfg = Arc::new(Config::load(Path::new(&args[1]))?);
    let relay = Relay::new(cfg.clone(), cfg.read_password()?)?;
    relay.version().await?;
    let app = Arc::new(Service::new(cfg.clone(), relay.clone())?);
    let stop = CancellationToken::new();
    let router = irc_mcp::http::build(app.clone(), stop.clone())
        .await
        .map_err(|_| irc_mcp::http::startup_failure())?;
    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    let observe_stop = stop.clone();
    let hub = app.hub.clone();
    let mut observer = tokio::spawn(async move {
        relay.observe(hub, observe_stop).await;
    });
    let signal_stop = stop.clone();
    let signal = tokio::spawn(async move {
        #[cfg(unix)]
        {
            if let Ok(mut term) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
            } else {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
        signal_stop.cancel();
    });
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(stop.clone().cancelled_owned())
        .await;
    stop.cancel();
    signal.abort();
    if tokio::time::timeout(std::time::Duration::from_secs(5), &mut observer)
        .await
        .is_err()
    {
        observer.abort();
    }
    result?;
    Ok(())
}
