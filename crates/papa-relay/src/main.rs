//! `papa-relay`: serves the API, or runs an admin command.
//! Settings come from environment variables; see `.env.example`.

use std::process::ExitCode;

use papa_relay::rate_limit::RateLimiter;
use papa_relay::{AppState, Config, Shared, Store, admin, now_ms};
use tracing_subscriber::EnvFilter;

const USAGE: &str = "\
Usage:
  papa-relay [serve]                        Serve the API
  papa-relay admin invite create [NOTE] [--days N]
                                            Mint an invite code (valid 14 days by default)
  papa-relay admin invite list              List invites and who used them

Settings are read from environment variables; see .env.example.";

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

#[tokio::main]
async fn main() -> ExitCode {
    // Log level from RUST_LOG, e.g. `RUST_LOG=papa_relay=debug,tower_http=debug`.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("papa_relay=info,tower_http=info")),
        )
        // stdout is for command output, such as a new invite code.
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        [] | ["serve"] => serve().await,
        ["admin", "invite", "create", rest @ ..] => create_invite(rest).await,
        ["admin", "invite", "list"] => list_invites().await,
        ["help" | "--help" | "-h"] => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(format!("unknown command\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("papa-relay: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> Result<(), String> {
    let config = Config::from_env().map_err(|e| e.to_string())?;
    let store = open_store(&config).await?;
    let provisioner = config.provisioner();
    let mode = provisioner.mode();
    let state = AppState::new(Shared {
        store,
        provisioner,
        rate_limiter: RateLimiter::new(config.auth_per_minute),
        client_ip_header: config.client_ip_header.clone(),
    });
    let listener = tokio::net::TcpListener::bind(config.addr)
        .await
        .map_err(|e| format!("can't listen on {}: {e}", config.addr))?;
    #[cfg(unix)]
    {
        let socket = admin::socket_path(&config.db_path);
        admin::listen(&socket, state.clone())
            .map_err(|e| format!("can't listen on {}: {e}", socket.display()))?;
    }
    tracing::info!(addr = %config.addr, ?mode, db = %config.db_path.display(), "papa-relay listening");
    papa_relay::serve(listener, state, async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutting down");
    })
    .await
    .map_err(|e| e.to_string())
}

async fn create_invite(args: &[&str]) -> Result<(), String> {
    let mut note = String::new();
    let mut days: i64 = 14;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match *arg {
            "--days" => {
                days = args
                    .next()
                    .and_then(|d| d.parse().ok())
                    .ok_or("--days needs a number")?;
            }
            other if note.is_empty() => note = other.to_owned(),
            other => return Err(format!("unexpected argument {other:?}\n\n{USAGE}")),
        }
    }
    match admin_request(admin::Request::CreateInvite { note, days }).await? {
        admin::Reply::Invite { code, .. } => {
            // Shown once: only its hash is stored.
            println!("{code}");
            eprintln!("Valid for {days} days, for one account. It can't be shown again.");
            Ok(())
        }
        other => Err(unexpected(other)),
    }
}

async fn list_invites() -> Result<(), String> {
    let invites = match admin_request(admin::Request::ListInvites).await? {
        admin::Reply::Invites { invites } => invites,
        other => return Err(unexpected(other)),
    };
    if invites.is_empty() {
        println!("No invites yet.");
    }
    let now = now_ms();
    for invite in invites {
        let status = match (&invite.used_by_email, invite.expires_at > now) {
            (Some(email), _) => format!("used by {email}"),
            (None, true) => {
                // Rounded up: an invite made today has all 14 days left.
                let days = (invite.expires_at - now + DAY_MS - 1) / DAY_MS;
                format!("open, {days} days left")
            }
            (None, false) => "expired".to_owned(),
        };
        let note = if invite.note.is_empty() {
            "(no note)"
        } else {
            &invite.note
        };
        println!("{note:<30} {status}");
    }
    Ok(())
}

/// Asks the running relay through its admin socket or, if none is
/// running, opens the database directly.
async fn admin_request(request: admin::Request) -> Result<admin::Reply, String> {
    let config = Config::from_env().map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        let socket = admin::socket_path(&config.db_path);
        let sent = admin::send(&socket, &request)
            .await
            .map_err(|e| format!("admin socket {}: {e}", socket.display()))?;
        if let Some(reply) = sent {
            return Ok(reply);
        }
    }
    let store = open_store(&config).await?;
    Ok(admin::handle(&store, request).await)
}

fn unexpected(reply: admin::Reply) -> String {
    match reply {
        admin::Reply::Error { message } => message,
        other => format!("unexpected reply: {other:?}"),
    }
}

async fn open_store(config: &Config) -> Result<Store, String> {
    Store::open(&config.db_path)
        .await
        .map_err(|e| format!("can't open {}: {e}", config.db_path.display()))
}
