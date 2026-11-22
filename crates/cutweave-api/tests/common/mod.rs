// SPDX-License-Identifier: GPL-3.0-or-later
//! A tiny raw HTTP/1.1 client so tests can set any Host/Origin header.
#![allow(dead_code, clippy::format_push_string)]

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

pub struct Reply {
    pub status: u16,
    pub head: String,
    pub body: String,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<String> {
        self.head.lines().skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_owned())
        })
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

pub struct Req<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub host: Option<String>,
    pub headers: Vec<(&'a str, String)>,
    pub body: Option<&'a str>,
}

impl<'a> Req<'a> {
    pub fn get(path: &'a str) -> Req<'a> {
        Req {
            method: "GET",
            path,
            host: None,
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn method(mut self, m: &'a str) -> Self {
        self.method = m;
        self
    }

    pub fn host(mut self, h: &str) -> Self {
        self.host = Some(h.to_owned());
        self
    }

    pub fn header(mut self, k: &'a str, v: &str) -> Self {
        self.headers.push((k, v.to_owned()));
        self
    }

    pub fn bearer(self, token: &str) -> Self {
        self.header("Authorization", &format!("Bearer {token}"))
    }

    pub fn body(mut self, b: &'a str) -> Self {
        self.body = Some(b);
        self
    }

    pub async fn send(self, port: u16) -> Reply {
        let host = self
            .host
            .clone()
            .unwrap_or_else(|| format!("127.0.0.1:{port}"));
        let mut text = format!(
            "{} {} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n",
            self.method, self.path
        );
        for (k, v) in &self.headers {
            text.push_str(&format!("{k}: {v}\r\n"));
        }
        let body = self.body.unwrap_or("");
        text.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(text.as_bytes()).await.unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.unwrap();
        let raw = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        Reply {
            status,
            head: head.to_owned(),
            body: body.to_owned(),
        }
    }
}
