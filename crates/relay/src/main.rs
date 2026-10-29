use clap::Parser;
use shardrail_relay::RelayRuntime;
use std::net::SocketAddr;

#[derive(Debug, Parser)]
#[command(
    name = "shardrail-relay",
    about = "Self-hosted E2EE WebSocket Relay for Shardrail (D67/D69/D70)"
)]
struct Args {
    /// Address to bind the WebSocket relay to (e.g. 127.0.0.1:8787 or 0.0.0.0:8787).
    #[arg(long, default_value = "127.0.0.1:8787")]
    listen: SocketAddr,

    /// Optional secret auth token required for daemons to register.
    #[arg(long)]
    auth_token: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let auth_token = args
        .auth_token
        .or_else(|| std::env::var("SHARDRAIL_RELAY_AUTH_TOKEN").ok());

    println!("Starting Shardrail Relay on ws://{}", args.listen);
    if auth_token.is_some() {
        println!("Daemon registration token authentication: ENABLED");
    } else {
        println!("Daemon registration token authentication: DISABLED (open pairing mode)");
    }

    let runtime = RelayRuntime::with_auth_token(auth_token);
    runtime.serve(args.listen).await?;
    Ok(())
}
