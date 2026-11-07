// 브라우저 속 유튜브 플레이어를 앱에서 조작한다.
// 127.0.0.1에 작은 HTTP 서버를 띄워 플레이어 페이지(player.html)를 주고, 페이지는 /cmd를 0.2초마다 읽어
// "이 영상의 start~end 구간을 재생하고 end에서 멈춰라" 명령을 수행한다. 의존성 없이 std만 쓴다.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PAGE: &str = include_str!("player.html");

#[derive(Default)]
struct Cmd {
    seq: u64,
    id: String,
    start: f64,
    end: f64,
    rate: f64,
}

pub struct Player {
    port: u16,
    cmd: Arc<Mutex<Cmd>>,
}

impl Player {
    pub fn start() -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("플레이어 서버 시작 실패: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let cmd = Arc::new(Mutex::new(Cmd { rate: 1.0, ..Cmd::default() }));
        let shared = cmd.clone();
        std::thread::spawn(move || {
            // 연결마다 스레드 — 브라우저가 미리 열어 두고 요청을 안 보내는 소켓이 다른 요청을 막지 않게
            for stream in listener.incoming().flatten() {
                let shared = shared.clone();
                std::thread::spawn(move || serve(stream, &shared));
            }
        });
        Ok(Self { port, cmd })
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// 구간 재생. 같은 구간을 다시 보내도 seq가 바뀌므로 처음부터 다시 재생된다.
    pub fn play(&self, id: &str, start: f64, end: f64, rate: f64) {
        let mut c = self.cmd.lock().unwrap();
        c.seq += 1;
        c.id = id.to_string();
        // 문장 앞뒤가 잘리지 않게 약간 여유를 둔다
        c.start = (start - 0.25).max(0.0);
        c.end = end + 0.35;
        c.rate = rate;
    }
}

fn serve(mut s: TcpStream, cmd: &Mutex<Cmd>) -> std::io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(1)))?;
    // 헤더 끝(빈 줄)까지 읽는다. 덜 읽고 닫으면 상대가 연결 리셋을 받는다.
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        let n = s.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let req = String::from_utf8_lossy(&buf);
    let path = req.split_whitespace().nth(1).unwrap_or("/");
    let (ctype, body) = if path.starts_with("/cmd") {
        let c = cmd.lock().unwrap();
        let json = serde_json::json!({"seq": c.seq, "id": c.id, "start": c.start, "end": c.end, "rate": c.rate});
        ("application/json", json.to_string())
    } else {
        ("text/html; charset=utf-8", PAGE.to_string())
    };
    write!(
        s,
        "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(port: u16, path: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn serves_page_and_commands() {
        let p = Player::start().unwrap();
        assert!(get(p.port, "/").contains("YT.Player"));
        p.play("abc", 10.0, 12.0, 0.75);
        let r = get(p.port, "/cmd");
        let body: serde_json::Value = serde_json::from_str(r.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["seq"], 1);
        assert_eq!(body["id"], "abc");
        assert_eq!(body["start"], 9.75);
        assert_eq!(body["rate"], 0.75);
    }
}
