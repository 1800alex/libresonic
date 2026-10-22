use std::time::Duration;

use libresonic_core::{AudioStream, Error, STREAM_CHUNK_BYTES, ServiceFuture};
use serde::Deserialize;
use serde_json::{Map, Value};
use url::Url;

use crate::{ClientOptions, Credentials};

#[derive(Clone)]
pub(crate) struct Transport {
    base: Url,
    auth: Credentials,
    http: reqwest::Client,
    audio_http: reqwest::Client,
    options: ClientOptions,
}

#[derive(Deserialize)]
struct Wrapper {
    #[serde(rename = "subsonic-response")]
    response: Envelope,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Envelope {
    status: String,
    error: Option<Failure>,
    pub version: Option<String>,
    #[serde(rename = "type")]
    pub server_type: Option<String>,
    pub server_version: Option<String>,
    #[serde(flatten)]
    pub data: Map<String, Value>,
}

#[derive(Deserialize)]
struct Failure {
    code: i32,
    message: Option<String>,
}

impl Transport {
    pub fn new(base: &str, auth: Credentials, options: ClientOptions) -> Result<Self, Error> {
        options.validate()?;
        auth.validate()?;
        let mut base =
            Url::parse(base).map_err(|_| Error::InvalidConfiguration("invalid server URL"))?;
        if !matches!(base.scheme(), "https" | "http") || base.host_str().is_none() {
            return Err(Error::InvalidConfiguration(
                "server URL requires HTTP(S) and a host",
            ));
        }
        if !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(Error::InvalidConfiguration(
                "server URL must not contain userinfo, query or fragment",
            ));
        }
        if base.scheme() == "http" && !options.allow_http {
            return Err(Error::InvalidConfiguration(
                "plaintext HTTP requires explicit opt-in",
            ));
        }
        base.set_path(&format!("{}/", base.path().trim_end_matches('/')));
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(options.connect_timeout)
            .timeout(options.request_timeout)
            .build()
            .map_err(|_| Error::InvalidConfiguration("unable to initialize HTTP transport"))?;
        // A stream has bounded per-read waits, not a whole-song deadline.
        let audio_http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(options.connect_timeout)
            .build()
            .map_err(|_| Error::InvalidConfiguration("unable to initialize audio transport"))?;
        Ok(Self {
            base,
            auth,
            http,
            audio_http,
            options,
        })
    }

    async fn response(
        &self,
        endpoint: &'static str,
        params: &[(&str, String)],
        audio: bool,
        offset: Option<u64>,
    ) -> Result<reqwest::Response, Error> {
        // Only constant endpoint names enter join; item IDs are query-encoded below.
        let url = self
            .base
            .join(&format!("rest/{endpoint}.view"))
            .map_err(|_| Error::InvalidConfiguration("invalid server path"))?;
        let client = if audio { &self.audio_http } else { &self.http };
        let mut request = client
            .get(url)
            .query(&[("c", "sonicfast"), ("v", "1.16.1"), ("f", "json")])
            .query(&self.auth.parameters())
            .query(params);
        if let Some(offset) = offset {
            request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        self.send(request).await
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response, Error> {
        let response = tokio::time::timeout(self.options.request_timeout, request.send())
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(transport_error)?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        Err(match status.as_u16() {
            300..=399 => Error::RedirectBlocked,
            401 => Error::Authentication,
            403 => Error::PermissionDenied,
            404 => Error::NotFound,
            429 => Error::RateLimited {
                retry_after: response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(Duration::from_secs),
            },
            501 => Error::Unsupported,
            status => Error::Http { status },
        })
    }

    pub async fn json(
        &self,
        endpoint: &'static str,
        params: &[(&str, String)],
    ) -> Result<Envelope, Error> {
        let response = self.response(endpoint, params, false, None).await?;
        let limit = if endpoint == "getArtists" {
            self.options.artist_index_limit
        } else {
            self.options.json_limit
        };
        let bytes = collect(response, limit, endpoint).await?;
        envelope(&bytes)
    }

    pub async fn json_write(
        &self,
        endpoint: &'static str,
        params: &[(&str, String)],
        form_post: bool,
    ) -> Result<Envelope, Error> {
        let url = self
            .base
            .join(&format!("rest/{endpoint}.view"))
            .map_err(|_| Error::InvalidConfiguration("invalid server path"))?;
        let mut fields = vec![
            ("c", "sonicfast".to_owned()),
            ("v", "1.16.1".into()),
            ("f", "json".into()),
        ];
        fields.extend(self.auth.parameters());
        fields.extend_from_slice(params);
        let encoded = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&fields)
            .finish();
        if encoded.len() > 1024 * 1024 {
            return Err(Error::InvalidConfiguration("write request exceeds 1 MiB"));
        }
        let request = if form_post {
            self.http
                .post(url)
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(encoded)
        } else {
            if url.as_str().len() + encoded.len() + 1 > 8192 {
                return Err(Error::InvalidConfiguration(
                    "write request exceeds GET limits; this server needs formPost support",
                ));
            }
            self.http.get(url).query(&fields)
        };
        let response = self.send(request).await?;
        envelope(&collect(response, self.options.json_limit, endpoint).await?)
    }

    pub async fn artwork(&self, params: &[(&str, String)]) -> Result<Vec<u8>, Error> {
        let response = self.response("getCoverArt", params, false, None).await?;
        let mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let is_json = mime.contains("json");
        let limit = if is_json {
            self.options.json_limit
        } else {
            self.options.artwork_limit
        };
        let bytes = collect(response, limit, "getCoverArt").await?;
        if is_json {
            envelope(&bytes)?;
            return Err(Error::InvalidResponse);
        }
        if bytes.is_empty() || !mime.starts_with("image/") {
            return Err(Error::InvalidResponse);
        }
        Ok(bytes)
    }

    pub async fn download(&self, id: &str) -> Result<Box<dyn AudioStream>, Error> {
        let response = self
            .response("download", &[("id", id.to_owned())], true, None)
            .await?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::InvalidResponse);
        }
        let mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if mime.contains("json") {
            let body = tokio::time::timeout(
                self.options.request_timeout,
                collect(response, self.options.json_limit, "download error body"),
            )
            .await
            .map_err(|_| Error::Timeout)??;
            envelope(&body)?;
            return Err(Error::InvalidResponse);
        }
        if mime.contains("xml")
            || !(mime.starts_with("audio/")
                || mime.starts_with("application/octet-stream")
                || mime.starts_with("application/mp4")
                || mime.starts_with("video/mp4"))
        {
            return Err(Error::InvalidResponse);
        }
        let length = response.content_length();
        Ok(Box::new(ResponseStream {
            response: Some(response),
            pending: bytes::Bytes::new(),
            timeout: self.options.request_timeout,
            length,
            position: 0,
            range: None,
        }))
    }
    pub async fn audio(&self, id: &str) -> Result<Box<dyn AudioStream>, Error> {
        let response = self
            .response(
                "stream",
                &[("id", id.to_owned()), ("format", "raw".into())],
                true,
                None,
            )
            .await?;
        let mime = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if mime.contains("json") {
            let body = tokio::time::timeout(
                self.options.request_timeout,
                collect(response, self.options.json_limit, "stream error body"),
            )
            .await
            .map_err(|_| Error::Timeout)??;
            envelope(&body)?;
            return Err(Error::InvalidResponse);
        }
        if !mime.starts_with("audio/")
            && !mime.starts_with("application/octet-stream")
            && !mime.starts_with("application/mp4")
            && !mime.starts_with("video/mp4")
        {
            return Err(Error::InvalidResponse);
        }
        let length = response.content_length();
        let seekable = length.is_some()
            && response.status() == reqwest::StatusCode::OK
            && response
                .headers()
                .get(reqwest::header::ACCEPT_RANGES)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.eq_ignore_ascii_case("bytes"));
        Ok(Box::new(ResponseStream {
            response: Some(response),
            pending: bytes::Bytes::new(),
            timeout: self.options.request_timeout,
            length,
            position: 0,
            range: seekable.then(|| (self.clone(), id.to_owned())),
        }))
    }
}

struct ResponseStream {
    response: Option<reqwest::Response>,
    pending: bytes::Bytes,
    timeout: Duration,
    length: Option<u64>,
    position: u64,
    range: Option<(Transport, String)>,
}

impl AudioStream for ResponseStream {
    fn byte_len(&self) -> Option<u64> {
        self.length
    }
    fn is_seekable(&self) -> bool {
        self.range.is_some()
    }
    fn seek_to(&mut self, offset: u64) -> ServiceFuture<'_, ()> {
        Box::pin(async move {
            let (transport, id) = self.range.as_ref().ok_or(Error::Unsupported)?;
            let total = self.length.ok_or(Error::Unsupported)?;
            if offset > total {
                return Err(Error::InvalidResponse);
            }
            if offset == self.position {
                return Ok(());
            }
            if offset == total {
                self.response = None;
            } else {
                let response = transport
                    .response(
                        "stream",
                        &[("id", id.clone()), ("format", "raw".into())],
                        true,
                        Some(offset),
                    )
                    .await?;
                // Never reinterpret a server ignoring Range as data from the
                // requested offset. Require the same complete representation.
                let expected = format!("bytes {offset}-{}/{total}", total - 1);
                if response.status() != reqwest::StatusCode::PARTIAL_CONTENT
                    || response
                        .headers()
                        .get(reqwest::header::CONTENT_RANGE)
                        .and_then(|v| v.to_str().ok())
                        != Some(expected.as_str())
                    || response.content_length() != Some(total - offset)
                {
                    return Err(Error::InvalidResponse);
                }
                self.response = Some(response);
            }
            self.pending = bytes::Bytes::new();
            self.position = offset;
            Ok(())
        })
    }
    fn next(&mut self) -> ServiceFuture<'_, Option<Vec<u8>>> {
        Box::pin(async move {
            if self.pending.is_empty() {
                let Some(response) = self.response.as_mut() else {
                    return Ok(None);
                };
                let next = async {
                    loop {
                        match response.chunk().await.map_err(transport_error)? {
                            Some(chunk) if chunk.is_empty() => continue,
                            other => return Ok(other),
                        }
                    }
                };
                let Some(bytes) = tokio::time::timeout(self.timeout, next)
                    .await
                    .map_err(|_| Error::Timeout)??
                else {
                    return Ok(None);
                };
                // Hyper's adaptive HTTP/1 reader can legitimately return more
                // than 256 KiB under playback backpressure (its current default
                // maximum is 417,792 bytes). Keep a separate 512 KiB transport
                // budget; the public decoder chunks remain at most 16 KiB.
                if bytes.len() > 512 * 1024 {
                    return Err(Error::ResponseLimitExceeded {
                        resource: "audio transport chunk",
                        limit_bytes: 512 * 1024,
                    });
                }
                self.pending = bytes;
            }
            let length = self.pending.len().min(STREAM_CHUNK_BYTES);
            self.position += length as u64;
            Ok(Some(self.pending.split_to(length).to_vec()))
        })
    }
}

fn transport_error(error: reqwest::Error) -> Error {
    if error.is_timeout() {
        Error::Timeout
    } else {
        Error::Network
    }
}

async fn collect(
    mut response: reqwest::Response,
    limit: usize,
    resource: &'static str,
) -> Result<Vec<u8>, Error> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(Error::ResponseLimitExceeded {
            resource,
            limit_bytes: limit,
        });
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(Error::ResponseLimitExceeded {
                resource,
                limit_bytes: limit,
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn envelope(bytes: &[u8]) -> Result<Envelope, Error> {
    let response = serde_json::from_slice::<Wrapper>(bytes)
        .map_err(|_| Error::InvalidResponse)?
        .response;
    match (response.status.as_str(), response.error.as_ref()) {
        ("ok", None) => Ok(response),
        ("failed", Some(error)) => Err(match error.code {
            40..=44 => Error::Authentication,
            50 => Error::PermissionDenied,
            70 => Error::NotFound,
            0 if error.message.as_deref().is_some_and(explicitly_unsupported) => Error::Unsupported,
            code => Error::Protocol { code },
        }),
        _ => Err(Error::InvalidResponse),
    }
}

fn explicitly_unsupported(message: &str) -> bool {
    let message = message.trim().to_ascii_lowercase();
    matches!(
        message.as_str(),
        "not implemented"
            | "unknown method"
            | "unknown endpoint"
            | "unknown rest endpoint"
            | "unsupported operation"
    ) || message.ends_with(" is not implemented")
}
