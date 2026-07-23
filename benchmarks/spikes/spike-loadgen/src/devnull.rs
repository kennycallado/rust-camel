// Spike 1C devnull: minimal raw-tokio HTTP/1.1 server that reads a
// request line + headers + Content-Length body, writes a 200 response,
// and stays in keep-alive mode. Used as the loopback baseline target
// for the loadgen smoke test (spec §4.4 calibration + §4.5 loadgen
// validation). No hyper, no framework — the only "work" is the
// scheduler + tokio reactor between the loadgen and a constant
// `200 OK "ok"`.

use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> io::Result<()> {
    let port: u16 = std::env::var("SPIKE_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(18080);
    let addr: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
    let listener = TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    println!("SPIKE_DEVNULL_READY {}", bound.port());
    use std::io::Write;
    let _ = std::io::stdout().flush();

    loop {
        let (mut socket, _peer) = listener.accept().await?;
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let resp = b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 2\r\nconnection: keep-alive\r\n\r\nok";
            loop {
                let mut header_end: Option<usize> = None;
                let mut body_remaining: usize = 0;
                let mut total: usize = 0;
                // Read until we see \r\n\r\n + body_remaining bytes.
                loop {
                    let n = match socket.read(&mut buf[total..]).await {
                        Ok(0) => return,
                        Ok(n) => n,
                        Err(_) => return,
                    };
                    total += n;
                    if header_end.is_none() {
                        if let Some(pos) = buf[..total].windows(4).position(|w| w == b"\r\n\r\n") {
                            let he = pos + 4;
                            header_end = Some(he);
                            // Find Content-Length and account for body bytes
                            // already received in this read: when the headers
                            // and a partial body arrive in the same read, the
                            // running `body_remaining` counter must be seeded
                            // at `content_length - body_already` so the next
                            // loop iteration's `saturating_sub(n)` does not
                            // over-decrement and stall the request.
                            let header_str = std::str::from_utf8(&buf[..he]).unwrap_or("");
                            let cl = parse_content_length(header_str).unwrap_or(0);
                            let body_already = total.saturating_sub(he);
                            body_remaining = cl.saturating_sub(body_already);
                        }
                    } else {
                        body_remaining = body_remaining.saturating_sub(n);
                    }
                    if let Some(he) = header_end {
                        if total >= he + body_remaining {
                            break;
                        }
                    }
                    if total >= buf.len() {
                        // header too big, drop
                        return;
                    }
                }
                if socket.write_all(resp).await.is_err() {
                    return;
                }
            }
        });
    }
}

fn parse_content_length(headers: &str) -> Option<usize> {
    for line in headers.split("\r\n") {
        let mut parts = line.splitn(2, ':');
        let name = parts.next()?.trim();
        let value = parts.next()?.trim();
        if name.eq_ignore_ascii_case("content-length") {
            return value.parse().ok();
        }
    }
    None
}
