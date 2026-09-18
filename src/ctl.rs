//! Local control server & CLI client on the single-instance guard port (47319).
//! Enables zero-split-brain inspection, single-topic reset, and event mocking.
//! Dev-only: binds 127.0.0.1 and requires a per-boot token. The server mints
//! a random token at startup into `<state_dir>/ctl.token` (0600); the CLI
//! client reads it back, so only processes sharing the user's state dir can
//! issue mutating commands (reset/trigger). No token on disk, no auth.
use crate::{
    ctl_auth::{auth_line_ok, read_control_token, write_control_token},
    ctl_cmd::handle_cmd,
    state::AppState,
    types::Res,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

pub async fn run_control_server(s: AppState, listener: TcpListener) {
    // No entropy → no socket (fail-closed): serving unauthenticated
    // mutating commands is worse than no control plane at all.
    let Some(token) = crate::ctl_auth::new_control_token() else {
        eprintln!("[ctl] FATAL: no OS entropy for control token — control socket disabled");
        return;
    };
    write_control_token(&token);
    loop {
        let (socket, _) = match listener.accept().await {
            Ok(conn) => conn,
            // Transient accept errors must not kill the server: the dev
            // console would lose topics/reset until the next restart.
            Err(e) => {
                eprintln!("[ctl] accept failed (continuing): {e}");
                continue;
            }
        };
        let s2 = s.clone();
        let token2 = token.clone();
        tokio::spawn(async move {
            let (reader, mut writer) = socket.into_split();
            let mut lines = BufReader::new(reader).lines();
            // First line authenticates; the command follows. Wrong or
            // missing token: refuse before touching any state.
            let authed = match lines.next_line().await {
                Ok(Some(l)) => auth_line_ok(&l, &token2),
                _ => false,
            };
            if !authed {
                let _ = writer.write_all(b"ERR: unauthorized\n").await;
                let _ = writer.flush().await;
                return;
            }
            if let Ok(Some(line)) = lines.next_line().await {
                let resp = handle_cmd(&s2, line.trim()).await;
                let _ = writer.write_all(resp.as_bytes()).await;
                let _ = writer.flush().await;
            }
        });
    }
}

pub async fn run_ctl_client(port: u16, args: &[String]) -> Res<()> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream = match TcpStream::connect(&addr).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("⚠️  Cannot connect to herdr-telegram on {addr}: {e}");
            eprintln!("   Is the bot running? Start it with: herdr-telegram dev start");
            return Err(e.into());
        }
    };

    let Some(token) = read_control_token() else {
        eprintln!(
            "⚠️  Control token missing ({}): is the bot running from this state dir?",
            crate::ctl_auth::display_token_path()
        );
        return Err("control token missing".into());
    };
    let cmd_line = format!("auth {token}\n{}\n", args.join(" "));
    stream.write_all(cmd_line.as_bytes()).await?;
    stream.flush().await?;

    let (reader, _) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        println!("{line}");
    }
    Ok(())
}
