#[cfg(unix)]
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SuspendContext;

#[cfg(unix)]
impl SuspendContext {
  pub(crate) fn new() -> Self {
    Self
  }

  pub(crate) fn suspend(&self) {
    // TODO: wire real SIGTSTP/SIGCONT handling once app event loop is stabilized.
  }
}
