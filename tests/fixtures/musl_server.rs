use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Instant;

const PORT: u16 = 29876;

struct ServerStats {
    start_time: Instant,
    requests_served: u64,
    bytes_received: u64,
    bytes_sent: u64,
}

fn handle_connection(mut stream: TcpStream, stats: &mut ServerStats) {
    let mut buffer = [0u8; 4096];
    let bytes_read = match stream.read(&mut buffer) {
        Ok(n) if n > 0 => n,
        _ => return,
    };

    stats.bytes_received += bytes_read as u64;
    stats.requests_served += 1;

    let req_str = String::from_utf8_lossy(&buffer[..bytes_read]);
    let mut lines = req_str.lines();
    let request_line = match lines.next() {
        Some(line) => line,
        None => return,
    };

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let path_and_query = parts.next().unwrap_or("/");

    let mut path_parts = path_and_query.splitn(2, '?');
    let path = path_parts.next().unwrap_or("/");
    let query = path_parts.next().unwrap_or("");

    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }

    let (status, content_type, body) = match path {
        "/" => {
            let html = format!(
                r#"<!DOCTYPE html>
<html>
<head><title>Musl ISR Server</title></head>
<body>
<h1>Musl Linux HTTP Server</h1>
<p>Running on static Musl libc under ISR sandbox with PAC and shadow tags.</p>
<ul>
  <li>Requests served: {}</li>
  <li>Uptime: {}s</li>
  <li>Bytes in: {}</li>
  <li>Bytes out: {}</li>
</ul>
<h3>Endpoints</h3>
<ul>
  <li><a href="/bench">/bench</a> - Allocator benchmark</li>
  <li><a href="/stats">/stats</a> - JSON Telemetry</li>
  <li><a href="/echo">/echo</a> - Header reflection</li>
</ul>
</body>
</html>"#,
                stats.requests_served,
                stats.start_time.elapsed().as_secs(),
                stats.bytes_received,
                stats.bytes_sent,
            );
            ("200 OK", "text/html; charset=utf-8", html.into_bytes())
        }
        "/bench" => {
            // Exercise the allocator with scratch vectors, session ring, and JSON generation.
            let mut scratch_data = Vec::with_capacity(128);
            for i in 0..128 {
                scratch_data.push(((i * 31 + 17) & 0xff) as u8);
            }
            let checksum: u32 = scratch_data.iter().map(|&x| x as u32).sum();

            let mut session_ring = Vec::new();
            for i in 0..16 {
                session_ring.push(format!("session-token-{}-{}", i, stats.requests_served));
            }

            let json_body = format!(
                r#"{{"status":"ok","engine":"isr-musl","mitigations":["pac","shadow_tags","cfi"],"requests":{},"chk":{},"sessions":{}}}"#,
                stats.requests_served,
                checksum,
                session_ring.len(),
            );
            ("200 OK", "application/json", json_body.into_bytes())
        }
        "/stats" => {
            let json_body = format!(
                r#"{{"requests":{},"uptime_ms":{},"bytes_in":{},"bytes_out":{}}}"#,
                stats.requests_served,
                stats.start_time.elapsed().as_millis(),
                stats.bytes_received,
                stats.bytes_sent,
            );
            ("200 OK", "application/json", json_body.into_bytes())
        }
        "/echo" => {
            let mut echo_headers = Vec::new();
            for (k, v) in &headers {
                echo_headers.push(format!(r#""{}":"{}""#, k, v.replace('"', "\\\"")));
            }
            let json_body = format!(
                r#"{{"method":"{}","path":"{}","query":"{}","headers":{{{}}}}}"#,
                method,
                path,
                query,
                echo_headers.join(","),
            );
            ("200 OK", "application/json", json_body.into_bytes())
        }
        _ => {
            let json_body = format!(
                r#"{{"error":"not_found","path":"{}"}}"#,
                path.replace('"', "\\\"")
            );
            ("404 Not Found", "application/json", json_body.into_bytes())
        }
    };

    let response = format!(
        "HTTP/1.1 {}\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         Server: isr-musl/1.0\r\n\
         \r\n",
        status,
        content_type,
        body.len(),
    );

    let mut full_response = response.into_bytes();
    full_response.extend_from_slice(&body);
    stats.bytes_sent += full_response.len() as u64;

    let _ = stream.write_all(&full_response);
    let _ = stream.flush();
}

fn main() {
    let bind_addr = format!("127.0.0.1:{}", PORT);
    println!("Starting Musl HTTP Server on {}", bind_addr);

    let listener = match TcpListener::bind(&bind_addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind to {}: {}", bind_addr, e);
            std::process::exit(1);
        }
    };

    println!("Listening on http://{}", listener.local_addr().unwrap());

    let mut stats = ServerStats {
        start_time: Instant::now(),
        requests_served: 0,
        bytes_received: 0,
        bytes_sent: 0,
    };

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => handle_connection(stream, &mut stats),
            Err(e) => {
                eprintln!("Accept error: {}", e);
                break;
            }
        }
    }
}
