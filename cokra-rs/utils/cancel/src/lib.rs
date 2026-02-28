// Cancel utility
pub struct CancellationToken;

impl CancellationToken {
  pub fn new() -> Self {
    Self
  }
  pub fn cancel(&self) {}
  pub fn is_cancelled(&self) -> bool {
    false
  }
}
