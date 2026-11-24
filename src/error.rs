use std::fmt;

/// What can go wrong, worded for the person reading the terminal (or Canopy's operation log).
#[derive(Debug)]
pub enum Error {
    /// Bad command-line use.
    Usage(String),
    /// A saved connection is missing or unreadable.
    Config(String),
    /// JDownloader or MyJDownloader could not be reached.
    Network(String),
    /// The server answered with an error (`AUTH_FAILED`, `TOKEN_INVALID`…).
    Api { status: u16, kind: String },
    /// The answer did not follow the protocol.
    Protocol(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Usage(message) | Error::Config(message) => f.write_str(message),
            Error::Network(message) => write!(f, "cannot reach JDownloader: {message}"),
            Error::Api { status, kind } => match kind.as_str() {
                "AUTH_FAILED" => f.write_str("MyJDownloader refused the e-mail or password"),
                "OFFLINE" => f.write_str("JDownloader is offline (not connected to MyJDownloader)"),
                "TOO_MANY_REQUESTS" => {
                    f.write_str("MyJDownloader asks to slow down; try again shortly")
                }
                _ => write!(f, "JDownloader answered {kind} (HTTP {status})"),
            },
            Error::Protocol(message) => write!(f, "unexpected answer: {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// Exit code: 2 for usage mistakes, 1 for everything else.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Usage(_) => 2,
            _ => 1,
        }
    }

    /// The session expired or was revoked: a reconnect may fix it.
    pub fn is_session_lost(&self) -> bool {
        matches!(self, Error::Api { kind, .. } if kind == "TOKEN_INVALID" || kind == "AUTH_FAILED_SESSION")
    }
}
