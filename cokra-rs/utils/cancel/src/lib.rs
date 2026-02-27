// Utils - Cancel
// Cancellation utilities

use tokio_util::sync::CancellationToken;

/// Create a new cancellation token
pub fn cancel_token() -> CancellationToken {
    CancellationToken::new()
}
