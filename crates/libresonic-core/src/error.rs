use std::fmt;
use std::time::Duration;

/// Safe service failures. No variant stores a URL, secret or raw server body.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Invalid connection settings; the reason is a fixed library message.
    InvalidConfiguration(&'static str),
    /// An empty identifier or inappropriate item kind.
    InvalidIdentifier,
    /// The supplied item belongs to a different server profile.
    WrongServer,
    /// Credentials or the selected authentication mechanism were rejected.
    Authentication,
    /// The authenticated user lacks permission.
    PermissionDenied,
    /// A playlist changed since the selected rows were read; refresh before retrying.
    Conflict,
    /// The requested item or endpoint was not found.
    NotFound,
    /// The operation is explicitly unsupported.
    Unsupported,
    /// The server asked the client to wait; no automatic retry was made.
    RateLimited {
        /// A delay in seconds supplied by the server, when available.
        retry_after: Option<Duration>,
    },
    /// An HTTP failure with a safe numeric status.
    Http {
        /// HTTP status code.
        status: u16,
    },
    /// A protocol failure with an unrecognized numeric code.
    Protocol {
        /// Server error code; its free-form message is deliberately discarded.
        code: i32,
    },
    /// Invalid JSON, response shape or mandatory identity.
    InvalidResponse,
    /// A body exceeded its configured byte budget.
    ResponseTooLarge,
    /// A collected HTTP body exceeded its budget. Resource names are fixed
    /// protocol endpoint labels, never request URLs, item IDs or server text.
    ResponseLimitExceeded {
        /// Static resource/endpoint label.
        resource: &'static str,
        /// Configured maximum body size in bytes.
        limit_bytes: usize,
    },
    /// An authenticated redirect was refused before following it.
    RedirectBlocked,
    /// A connect or whole-request deadline elapsed.
    Timeout,
    /// A transport failure without the underlying credential-bearing error.
    Network,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(reason) => write!(f, "Invalid configuration: {reason}"),
            Self::InvalidIdentifier => f.write_str("Invalid media identifier"),
            Self::WrongServer => f.write_str("Media belongs to a different server"),
            Self::Authentication => {
                f.write_str("Authentication failed; check the selected credentials")
            }
            Self::PermissionDenied => f.write_str("The server denied permission"),
            Self::Conflict => {
                f.write_str("The playlist changed on the server; refresh and try again")
            }
            Self::NotFound => f.write_str("The requested item or endpoint was not found"),
            Self::Unsupported => f.write_str("The server does not support this operation"),
            Self::RateLimited { .. } => f.write_str("The server rate limit was reached"),
            Self::Http { status } => write!(f, "The server returned HTTP {status}"),
            Self::Protocol { code } => write!(f, "The server returned protocol error {code}"),
            Self::InvalidResponse => f.write_str("The server returned an invalid response"),
            Self::ResponseTooLarge => f.write_str("The response exceeded its byte limit"),
            Self::ResponseLimitExceeded {
                resource,
                limit_bytes,
            } => {
                write!(
                    f,
                    "The {resource} response exceeded its {limit_bytes}-byte limit"
                )
            }
            Self::RedirectBlocked => f.write_str("An authenticated redirect was blocked"),
            Self::Timeout => f.write_str("The server request timed out"),
            Self::Network => f.write_str("Unable to reach the server"),
        }
    }
}

// Deliberately no raw transport/server error in source().
impl std::error::Error for Error {}
