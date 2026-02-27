// Utils - Env
// Environment variable utilities

/// Get environment variable
pub fn get_var(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// Set environment variable
pub fn set_var(key: &str, value: &str) {
    std::env::set_var(key, value);
}
