use std::fmt;
use std::time::Duration;

use libresonic_core::{AuthenticationMethod, Error};

/// In-memory authentication with deliberately redacted Debug and no serialization.
#[derive(Clone)]
pub struct Credentials(pub(crate) Auth);

#[derive(Clone)]
pub(crate) enum Auth {
    Token { username: String, password: String },
    ApiKey(String),
}

impl Credentials {
    pub(crate) fn username(&self) -> Option<&str> {
        match &self.0 {
            Auth::Token { username, .. } => Some(username),
            Auth::ApiKey(_) => None,
        }
    }
    /// Username/password authentication with a fresh random salt for each request.
    pub fn token(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self(Auth::Token {
            username: username.into(),
            password: password.into(),
        })
    }

    /// A server-issued API key; no username is sent with it.
    pub fn api_key(key: impl Into<String>) -> Self {
        Self(Auth::ApiKey(key.into()))
    }

    pub(crate) fn method(&self) -> AuthenticationMethod {
        match self.0 {
            Auth::ApiKey(_) => AuthenticationMethod::ApiKey,
            Auth::Token { .. } => AuthenticationMethod::Token,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), Error> {
        let invalid = match &self.0 {
            Auth::ApiKey(api_key) => api_key.is_empty(),
            Auth::Token { username, .. } => username.trim().is_empty(),
        };
        if invalid {
            return Err(Error::InvalidConfiguration(
                "empty or unsupported credentials",
            ));
        }
        Ok(())
    }

    pub(crate) fn parameters(&self) -> Vec<(&'static str, String)> {
        use md5::{Digest, Md5};
        match &self.0 {
            Auth::ApiKey(key) => vec![("apiKey", key.clone())],
            Auth::Token { username, password } => {
                let salt: String = rand::random::<[u8; 16]>()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                let mut digest = Md5::new();
                digest.update(password.as_bytes());
                digest.update(salt.as_bytes());
                let token = format!("{:x}", digest.finalize());
                vec![("u", username.clone()), ("s", salt), ("t", token)]
            }
        }
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credentials([redacted])")
    }
}

/// Request policy; defaults require HTTPS and bound every collected response.
#[derive(Clone, Debug)]
pub struct ClientOptions {
    pub(crate) allow_http: bool,
    pub(crate) connect_timeout: Duration,
    pub(crate) request_timeout: Duration,
    pub(crate) json_limit: usize,
    pub(crate) artist_index_limit: usize,
    pub(crate) artwork_limit: usize,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            allow_http: false,
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(20),
            json_limit: 4 * 1024 * 1024,
            // getArtists has no pagination and returns the entire library index.
            artist_index_limit: 32 * 1024 * 1024,
            artwork_limit: 16 * 1024 * 1024,
        }
    }
}

impl ClientOptions {
    /// Explicitly acknowledge plaintext HTTP for a self-hosted/local server.
    #[must_use]
    pub fn allow_http(mut self, allow: bool) -> Self {
        self.allow_http = allow;
        self
    }

    /// Set nonzero connect and whole-request deadlines, including body reads.
    #[must_use]
    pub fn timeouts(mut self, connect: Duration, request: Duration) -> Self {
        self.connect_timeout = connect;
        self.request_timeout = request;
        self
    }

    /// Set nonzero byte caps applied while reading JSON (including the full
    /// artist index) and artwork bodies. This overrides the larger index default.
    #[must_use]
    pub fn response_limits(mut self, json_bytes: usize, artwork_bytes: usize) -> Self {
        self.json_limit = json_bytes;
        self.artist_index_limit = json_bytes;
        self.artwork_limit = artwork_bytes;
        self
    }

    /// Override the full, unpaginated getArtists response budget independently
    /// of ordinary JSON. Defaults to 32 MiB; ordinary JSON defaults to 4 MiB.
    #[must_use]
    pub fn artist_index_limit(mut self, bytes: usize) -> Self {
        self.artist_index_limit = bytes;
        self
    }

    pub(crate) fn validate(&self) -> Result<(), Error> {
        if self.connect_timeout.is_zero() || self.request_timeout.is_zero() {
            return Err(Error::InvalidConfiguration(
                "request deadlines must be nonzero",
            ));
        }
        if self.json_limit == 0 || self.artist_index_limit == 0 || self.artwork_limit == 0 {
            return Err(Error::InvalidConfiguration(
                "response limits must be nonzero",
            ));
        }
        Ok(())
    }
}
