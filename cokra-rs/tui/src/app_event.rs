use cokra_protocol::Op;

#[derive(Debug)]
pub(crate) enum AppEvent {
  CodexOp(Op),
}
