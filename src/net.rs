//! HTTP for the desktop app and the web version, behind one async interface.
//!
//! The desktop uses reqwest's blocking client (each background task has its own thread, so
//! blocking is fine there); the web uses the browser's fetch through reqwest's wasm client.

use web_time::Duration;

pub use reqwest::StatusCode;
use serde::de::DeserializeOwned;

#[cfg(not(target_arch = "wasm32"))]
use reqwest::blocking as backend;
#[cfg(target_arch = "wasm32")]
use reqwest as backend;

#[derive(Clone)]
pub struct Http(backend::Client);

impl Http {
    /// A client that gives up on requests after `timeout` (None: no limit, for big downloads).
    pub fn new(timeout: Option<Duration>) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let builder = {
            let b = backend::Client::builder()
                .user_agent(concat!("rainbow/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(30));
            match timeout {
                Some(t) => b.timeout(t),
                None => b.timeout(None),
            }
        };
        // Browsers set their own User-Agent, and requests time out one by one (see `Req::timeout`).
        #[cfg(target_arch = "wasm32")]
        let builder = {
            let _ = timeout;
            backend::Client::builder()
        };
        Self(builder.build().expect("failed to create HTTP client"))
    }

    pub fn get(&self, url: &str) -> Req {
        Req(self.0.get(url))
    }

    pub fn post(&self, url: &str) -> Req {
        Req(self.0.post(url))
    }
}

pub struct Req(backend::RequestBuilder);

impl Req {
    pub fn header(self, name: &str, value: &str) -> Self {
        Req(self.0.header(name, value))
    }

    pub fn json(self, body: &serde_json::Value) -> Self {
        Req(self.0.json(body))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn body(self, body: String) -> Self {
        Req(self.0.body(body))
    }

    pub fn timeout(self, d: Duration) -> Self {
        Req(self.0.timeout(d))
    }

    pub async fn send(self) -> reqwest::Result<Resp> {
        #[cfg(not(target_arch = "wasm32"))]
        return self.0.send().map(Resp);
        #[cfg(target_arch = "wasm32")]
        return self.0.send().await.map(|r| Resp { status: r.status(), headers: r.headers().clone(), length: r.content_length(), body: Body::Whole(r) });
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub struct Resp(backend::Response);

#[cfg(target_arch = "wasm32")]
pub struct Resp {
    status: StatusCode,
    headers: reqwest::header::HeaderMap,
    length: Option<u64>,
    body: Body,
}

#[cfg(target_arch = "wasm32")]
enum Body {
    Whole(reqwest::Response),
    Stream(std::pin::Pin<Box<dyn futures_core::Stream<Item = reqwest::Result<bytes::Bytes>>>>),
    Done,
}

impl Resp {
    pub fn status(&self) -> StatusCode {
        #[cfg(not(target_arch = "wasm32"))]
        return self.0.status();
        #[cfg(target_arch = "wasm32")]
        return self.status;
    }

    fn headers(&self) -> &reqwest::header::HeaderMap {
        #[cfg(not(target_arch = "wasm32"))]
        return self.0.headers();
        #[cfg(target_arch = "wasm32")]
        return &self.headers;
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers().get(name)?.to_str().ok()
    }

    pub fn content_length(&self) -> Option<u64> {
        #[cfg(not(target_arch = "wasm32"))]
        return self.0.content_length();
        #[cfg(target_arch = "wasm32")]
        return self.length;
    }

    pub fn error_for_status(self) -> reqwest::Result<Self> {
        #[cfg(not(target_arch = "wasm32"))]
        return self.0.error_for_status().map(Resp);
        #[cfg(target_arch = "wasm32")]
        return match self.body {
            Body::Whole(r) => r.error_for_status().map(|r| Resp { body: Body::Whole(r), ..self }),
            _ => Ok(self),
        };
    }

    /// The next piece of the body, or None at the end.
    pub async fn chunk(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use std::io::Read;
            let mut buf = vec![0u8; 64 * 1024];
            let n = self.0.read(&mut buf)?;
            buf.truncate(n);
            Ok((n > 0).then_some(buf))
        }
        #[cfg(target_arch = "wasm32")]
        {
            if let Body::Whole(_) = self.body {
                let Body::Whole(r) = std::mem::replace(&mut self.body, Body::Done) else { unreachable!() };
                self.body = Body::Stream(Box::pin(r.bytes_stream()));
            }
            let Body::Stream(stream) = &mut self.body else { return Ok(None) };
            match std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await {
                Some(piece) => Ok(Some(piece?.to_vec())),
                None => {
                    self.body = Body::Done;
                    Ok(None)
                }
            }
        }
    }

    pub async fn bytes(self) -> reqwest::Result<Vec<u8>> {
        #[cfg(not(target_arch = "wasm32"))]
        return self.0.bytes().map(|b| b.to_vec());
        #[cfg(target_arch = "wasm32")]
        return match self.body {
            Body::Whole(r) => r.bytes().await.map(|b| b.to_vec()),
            _ => Ok(Vec::new()),
        };
    }

    pub async fn json<T: DeserializeOwned>(self) -> reqwest::Result<T> {
        #[cfg(not(target_arch = "wasm32"))]
        return self.0.json();
        #[cfg(target_arch = "wasm32")]
        return match self.body {
            Body::Whole(r) => r.json().await,
            _ => unreachable!("json() after reading the body"),
        };
    }
}
