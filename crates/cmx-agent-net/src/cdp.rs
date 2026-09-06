//! 极简 CDP-over-WebSocket 客户端（手写 WS 帧，不引 tokio-tungstenite，离线安全）。
//!
//! 只够用：握手 → 发 JSON 命令（`{id,method,params,sessionId?}`）→ 按 id 收结果（忽略事件）。
//! 客户端帧按 RFC6455 掩码；服务端帧解掩码。仅处理 text/continuation/ping/close。

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const MAX_FRAME: usize = 64 * 1024 * 1024; // 64MB 上限，防超大 payload OOM

pub struct Cdp {
    stream: TcpStream,
    next_id: i64,
}

impl Cdp {
    /// 连接 `ws://host:port/path` 的 CDP 端点。
    pub async fn connect(ws_url: &str) -> Result<Self, String> {
        let rest = ws_url.strip_prefix("ws://").ok_or("CDP 需 ws:// URL")?;
        let (hostport, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let stream = TcpStream::connect(hostport)
            .await
            .map_err(|e| format!("连 CDP 端口失败：{e}"))?;
        let mut c = Self { stream, next_id: 0 };
        c.handshake(hostport, path).await?;
        Ok(c)
    }

    async fn handshake(&mut self, host: &str, path: &str) -> Result<(), String> {
        // Sec-WebSocket-Key 用固定合法 base64（不校验 accept，Chrome 必升级）。
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        self.stream.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        let mut b = [0u8; 1];
        loop {
            let n = self.stream.read(&mut b).await.map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("CDP 握手：连接关闭".into());
            }
            buf.push(b[0]);
            if buf.ends_with(b"\r\n\r\n") {
                break;
            }
            if buf.len() > 8192 {
                return Err("CDP 握手响应过长".into());
            }
        }
        let head = String::from_utf8_lossy(&buf);
        if !head.starts_with("HTTP/1.1 101") {
            return Err(format!("CDP 握手失败：{}", head.lines().next().unwrap_or("")));
        }
        Ok(())
    }

    fn text_frame(payload: &[u8]) -> Vec<u8> {
        let mut f = Vec::with_capacity(payload.len() + 14);
        f.push(0x81); // FIN + text opcode
        let mask = {
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0x2a55137f);
            n.to_le_bytes()
        };
        let len = payload.len();
        if len < 126 {
            f.push(0x80 | len as u8);
        } else if len < 65536 {
            f.push(0x80 | 126);
            f.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            f.push(0x80 | 127);
            f.extend_from_slice(&(len as u64).to_be_bytes());
        }
        f.extend_from_slice(&mask);
        for (i, byte) in payload.iter().enumerate() {
            f.push(byte ^ mask[i & 3]);
        }
        f
    }

    async fn read_frame(&mut self) -> Result<(u8, Vec<u8>), String> {
        let mut h = [0u8; 2];
        self.stream.read_exact(&mut h).await.map_err(|e| e.to_string())?;
        let op = h[0] & 0x0f;
        let masked = h[1] & 0x80 != 0;
        let mut len = (h[1] & 0x7f) as usize;
        if len == 126 {
            let mut e = [0u8; 2];
            self.stream.read_exact(&mut e).await.map_err(|x| x.to_string())?;
            len = u16::from_be_bytes(e) as usize;
        } else if len == 127 {
            let mut e = [0u8; 8];
            self.stream.read_exact(&mut e).await.map_err(|x| x.to_string())?;
            len = u64::from_be_bytes(e) as usize;
        }
        if len > MAX_FRAME {
            return Err("CDP 帧过大".into());
        }
        let mut mask = [0u8; 4];
        if masked {
            self.stream.read_exact(&mut mask).await.map_err(|x| x.to_string())?;
        }
        let mut payload = vec![0u8; len];
        self.stream.read_exact(&mut payload).await.map_err(|x| x.to_string())?;
        if masked {
            for i in 0..len {
                payload[i] ^= mask[i & 3];
            }
        }
        Ok((op, payload))
    }

    async fn read_msg(&mut self) -> Result<Value, String> {
        let mut acc: Vec<u8> = Vec::new();
        loop {
            let (op, payload) = self.read_frame().await?;
            match op {
                0x1 | 0x0 => {
                    acc.extend_from_slice(&payload);
                    // 简化：CDP 消息基本单帧；若分片，读到能解析为止
                    if let Ok(v) = serde_json::from_slice::<Value>(&acc) {
                        return Ok(v);
                    }
                }
                0x9 => {
                    // ping → 空 pong（掩码全 0）
                    self.stream.write_all(&[0x8a, 0x80, 0, 0, 0, 0]).await.map_err(|e| e.to_string())?;
                }
                0x8 => return Err("CDP 连接被关闭".into()),
                _ => {}
            }
        }
    }

    async fn send(&mut self, v: &Value) -> Result<(), String> {
        let s = serde_json::to_vec(v).map_err(|e| e.to_string())?;
        self.stream.write_all(&Self::text_frame(&s)).await.map_err(|e| e.to_string())
    }

    /// 发一条 CDP 命令，等匹配 id 的结果（忽略事件与其它 id）。
    pub async fn call(&mut self, method: &str, params: Value, session: Option<&str>) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        self.send(&msg).await?;
        for _ in 0..2000 {
            let v = self.read_msg().await?;
            if v.get("id").and_then(|x| x.as_i64()) == Some(id) {
                if let Some(err) = v.get("error") {
                    return Err(format!("CDP {method}: {err}"));
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
        }
        Err(format!("CDP {method}: 无响应"))
    }
}
