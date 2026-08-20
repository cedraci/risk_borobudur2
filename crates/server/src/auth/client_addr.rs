//! Where a request came from, for the audit log.
//!
//! Server mode is documented as running behind an HTTPS reverse proxy (the
//! session cookie is `Secure`), so the socket address the process sees is the
//! proxy's, not the client's — the forwarded header is the only thing that
//! carries the real origin. It is also trivially forgeable by anyone who can
//! reach the process directly, which is exactly why the deployment note says
//! never to expose the plain-HTTP port: the value below is evidence about a
//! request, not an authorization input, and nothing branches on it.

use axum::http::HeaderMap;
use std::net::SocketAddr;

const FORWARDED_FOR: &str = "x-forwarded-for";
const REAL_IP: &str = "x-real-ip";

/// The client address for one request: the left-most hop of
/// `X-Forwarded-For` (everything to its right is proxy chatter the client can
/// forge just as easily), then `X-Real-IP`, then the peer socket address when
/// the server was started with one — `None` if the deployment offers nothing.
pub fn from_request(headers: &HeaderMap, peer: Option<SocketAddr>) -> Option<String> {
    let header = |name: &str| {
        headers.get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    header(FORWARDED_FOR)
        .or_else(|| header(REAL_IP))
        .or_else(|| peer.map(|p| p.ip().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn takes_the_left_most_forwarded_hop() {
        let h = headers(&[(FORWARDED_FOR, "203.0.113.7, 10.0.0.1, 10.0.0.2")]);
        assert_eq!(from_request(&h, None).as_deref(), Some("203.0.113.7"));
    }

    #[test]
    fn falls_back_to_real_ip_then_to_the_peer() {
        let h = headers(&[(REAL_IP, "198.51.100.4")]);
        assert_eq!(from_request(&h, None).as_deref(), Some("198.51.100.4"));

        let peer: SocketAddr = "198.51.100.9:5000".parse().unwrap();
        assert_eq!(from_request(&HeaderMap::new(), Some(peer)).as_deref(), Some("198.51.100.9"));
    }

    #[test]
    fn an_empty_forwarded_header_is_not_an_address() {
        let h = headers(&[(FORWARDED_FOR, " ")]);
        assert_eq!(from_request(&h, None), None);
    }
}
