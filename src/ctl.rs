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
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
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
            let mut reader = BufReader::new(reader);
            // Bounded reads: no timeout + no length cap lets any local uid
            // hold tasks forever or OOM on a GB without `\n`. Commands are
            // one short line — 10s + 4KB then drop (fail-closed). The cap
            // is enforced DURING the read (`take`), never after buffering
            // a full line (a post-hoc `len` check OOMs first).
            async fn read_capped_line(
                r: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
            ) -> Option<String> {
                let mut buf = Vec::new();
                let n = match tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    r.take(4097).read_until(b'\n', &mut buf),
                )
                .await
                {
                    Ok(Ok(n)) => n,
                    _ => return None,
                };
                // `take(4097)` caps allocation during the read: anything
                // over 4KB (or empty/EOF) refuses without ever buffering
                // an unbounded line.
                if n == 0 || buf.len() > 4096 {
                    return None;
                }
                String::from_utf8(buf).ok().map(|s| {
                    s.strip_suffix('\n')
                        .unwrap_or(&s)
                        .strip_suffix('\r')
                        .unwrap_or(s.strip_suffix('\n').unwrap_or(&s))
                        .to_string()
                })
            }
            let authed = match read_capped_line(&mut reader).await {
                Some(l) => auth_line_ok(&l, &token2),
                None => false,
            };
            if !authed {
                let _ = writer.write_all(b"ERR: unauthorized\n").await;
                let _ = writer.flush().await;
                return;
            }
            let Some(cmd) = read_capped_line(&mut reader).await else {
                return;
            };
            let resp = handle_cmd(&s2, cmd.trim()).await;
            let _ = writer.write_all(resp.as_bytes()).await;
            let _ = writer.flush().await;
        });
    }
}

pub async fn run_ctl_client(port: u16, args: &[String]) -> Res<()> {
    let addr = format!("127.0.0.1:{port}");
    // Bounded connect: a wedged listener must fail the CLI visibly,
    // never hang it forever (fail-closed for a dev tool too).
    let mut stream = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        TcpStream::connect(&addr),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            eprintln!("⚠️  Cannot connect to herdr-telegram on {addr}: {e}");
            eprintln!("   Is the bot running? Start it with: herdr-telegram dev start");
            return Err(e.into());
        }
        Err(_) => {
            eprintln!("⚠️  Connect to herdr-telegram on {addr} timed out — is the bot wedged?");
            return Err("control connect timed out".into());
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
