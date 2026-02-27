// Utils - Network Proxy
// Network proxy utilities

use std::env;

/// Get proxy URL from environment
pub fn get_proxy() -> Option<String> {
    env::var("HTTP_PROXY").ok()
        .or_else(|| env::var("http_proxy").ok())
}
