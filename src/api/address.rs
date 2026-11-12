//! The server address rules: only an origin, and only a safe one.
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerUrlError {
    /// Not `http://` or `https://`, or not a bare address (path, query, user name, spaces...).
    Malformed,
    /// `http://` to a host that is not this machine: it would send everything in the clear.
    InsecureRemote,
}

impl fmt::Display for ServerUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Malformed => "the server must be a bare address like https://host[:port]",
            Self::InsecureRemote => "plain http:// is only allowed for localhost; use https://",
        })
    }
}

impl std::error::Error for ServerUrlError {}

/// An origin: scheme, host and port, normalised so two spellings of one origin compare equal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    https: bool,
    host: String,
    port: u16,
}

impl Origin {
    /// Reads the origin of `url`, ignoring any path, query or fragment after it.
    fn of(url: &str) -> Option<Self> {
        let (https, rest) = match url.split_once("://")? {
            (s, rest) if s.eq_ignore_ascii_case("https") => (true, rest),
            (s, rest) if s.eq_ignore_ascii_case("http") => (false, rest),
            _ => return None,
        };
        let authority = rest.split(['/', '?', '#']).next()?;
        // No user name or password, backslash, space or non-ASCII: `http://localhost@evil.com`
        // and its cousins are how a loopback check gets fooled.
        if authority.is_empty()
            || !authority
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b))
        {
            return None;
        }
        let (host, port) = match authority.strip_prefix('[') {
            Some(v6) => {
                let (host, tail) = v6.split_once(']')?;
                if host.is_empty() || !host.bytes().all(|b| b.is_ascii_hexdigit() || b == b':') {
                    return None;
                }
                (format!("[{}]", host.to_ascii_lowercase()), tail)
            }
            None => {
                let end = authority.find(':').unwrap_or(authority.len());
                let host = &authority[..end];
                let labels_ok = host.split('.').all(|l| {
                    !l.is_empty()
                        && !l.starts_with('-')
                        && !l.ends_with('-')
                        && !l.contains(['[', ']'])
                });
                if !labels_ok {
                    return None;
                }
                (host.to_ascii_lowercase(), &authority[end..])
            }
        };
        let port = match tail_port(port) {
            Some(Some(p)) => p,
            Some(None) => {
                if https {
                    443
                } else {
                    80
                }
            }
            None => return None,
        };
        Some(Self { https, host, port })
    }

    fn is_loopback(&self) -> bool {
        matches!(self.host.as_str(), "localhost" | "127.0.0.1" | "[::1]")
    }
}

/// `":8080"` is `Some(Some(8080))`, `""` is `Some(None)`, anything else is `None`.
fn tail_port(tail: &str) -> Option<Option<u16>> {
    if tail.is_empty() {
        return Some(None);
    }
    let digits = tail.strip_prefix(':')?;
    if digits.is_empty() || digits.len() > 5 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u16>().ok().filter(|p| *p > 0).map(Some)
}

/// Where the API lives. It can only be built from an address the CLI is willing to talk to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerUrl(Origin);

impl ServerUrl {
    /// `https://host[:port]`, or `http://` for `localhost`, `127.0.0.1` and `[::1]` only.
    /// One trailing `/` is allowed; any other path is not (the signed path is the API's own).
    pub fn parse(text: &str) -> Result<Self, ServerUrlError> {
        let bare = text.strip_suffix('/').unwrap_or(text);
        let origin = Origin::of(bare).ok_or(ServerUrlError::Malformed)?;
        let after_scheme = &bare[bare.find("://").map_or(0, |i| i + 3)..];
        if after_scheme.contains(['/', '?', '#']) {
            return Err(ServerUrlError::Malformed);
        }
        if !origin.https && !origin.is_loopback() {
            return Err(ServerUrlError::InsecureRemote);
        }
        Ok(Self(origin))
    }

    pub fn is_loopback(&self) -> bool {
        self.0.is_loopback()
    }

    /// The address without a trailing slash, ready to put a path after.
    pub fn base(&self) -> String {
        let o = &self.0;
        let default = if o.https { 443 } else { 80 };
        let scheme = if o.https { "https" } else { "http" };
        if o.port == default {
            format!("{scheme}://{}", o.host)
        } else {
            format!("{scheme}://{}:{}", o.host, o.port)
        }
    }

    /// Whether `url` (a verification link, say) points at this same origin.
    pub fn same_origin(&self, url: &str) -> bool {
        Origin::of(url).is_some_and(|o| o == self.0)
    }
}
