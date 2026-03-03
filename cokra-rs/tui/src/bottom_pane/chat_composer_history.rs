use std::path::PathBuf;

use cokra_protocol::user_input::TextElement;

use crate::bottom_pane::MentionBinding;

#[derive(Debug, Clone)]
pub(crate) struct HistoryEntry {
  pub(crate) text: String,
  pub(crate) text_elements: Vec<TextElement>,
  pub(crate) local_image_paths: Vec<PathBuf>,
  pub(crate) remote_image_urls: Vec<String>,
  pub(crate) mention_bindings: Vec<MentionBinding>,
  pub(crate) pending_pastes: Vec<(String, String)>,
}

impl HistoryEntry {
  pub(crate) fn new(text: String) -> Self {
    let decoded = decode_history_mentions(&text);
    Self {
      text: decoded.text,
      text_elements: Vec::new(),
      local_image_paths: Vec::new(),
      remote_image_urls: Vec::new(),
      mention_bindings: decoded
        .mentions
        .into_iter()
        .map(|mention| MentionBinding {
          mention: mention.mention,
          path: mention.path,
        })
        .collect(),
      pending_pastes: Vec::new(),
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DecodedMention {
  mention: String,
  path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DecodedText {
  text: String,
  mentions: Vec<DecodedMention>,
}

// cokra currently stores history text as-is. Mention decoding is intentionally
// a no-op placeholder for later parity with codex mention codec.
fn decode_history_mentions(text: &str) -> DecodedText {
  DecodedText {
    text: text.to_string(),
    mentions: Vec::new(),
  }
}

#[derive(Debug, Default)]
pub(crate) struct ChatComposerHistory {
  local_history: Vec<HistoryEntry>,
  history_cursor: Option<isize>,
  saved_draft: Option<HistoryEntry>,
}

impl ChatComposerHistory {
  pub(crate) fn new() -> Self {
    Self::default()
  }

  pub(crate) fn push(&mut self, text: String) {
    self.push_entry(HistoryEntry::new(text));
  }

  pub(crate) fn push_entry(&mut self, entry: HistoryEntry) {
    if self
      .local_history
      .last()
      .is_some_and(|prev| prev.text == entry.text)
    {
      self.history_cursor = None;
      return;
    }
    self.local_history.push(entry);
    self.history_cursor = None;
  }

  pub(crate) fn should_handle_navigation(&self, text: &str, cursor: usize) -> bool {
    if self.local_history.is_empty() {
      return false;
    }
    cursor == 0 || cursor == text.len()
  }

  pub(crate) fn navigate_prev_entry(&mut self, current: HistoryEntry) -> Option<HistoryEntry> {
    let len = self.local_history.len() as isize;
    if len == 0 {
      return None;
    }

    let next_idx = match self.history_cursor {
      None => {
        self.saved_draft = Some(current);
        len - 1
      }
      Some(idx) => (idx - 1).max(0),
    };

    self.history_cursor = Some(next_idx);
    self.local_history.get(next_idx as usize).cloned()
  }

  pub(crate) fn navigate_next_entry(&mut self) -> Option<HistoryEntry> {
    let len = self.local_history.len() as isize;
    match self.history_cursor {
      None => None,
      Some(idx) => {
        let next_idx = idx + 1;
        if next_idx >= len {
          self.history_cursor = None;
          None
        } else {
          self.history_cursor = Some(next_idx);
          self.local_history.get(next_idx as usize).cloned()
        }
      }
    }
  }

  pub(crate) fn navigate_prev(&mut self, current_text: &str) -> Option<String> {
    self
      .navigate_prev_entry(HistoryEntry::new(current_text.to_string()))
      .map(|entry| entry.text)
  }

  pub(crate) fn navigate_next(&mut self) -> Option<String> {
    self.navigate_next_entry().map(|entry| entry.text)
  }

  pub(crate) fn saved_draft_entry(&self) -> Option<&HistoryEntry> {
    self.saved_draft.as_ref()
  }

  pub(crate) fn saved_draft(&self) -> &str {
    self
      .saved_draft
      .as_ref()
      .map(|entry| entry.text.as_str())
      .unwrap_or("")
  }

  pub(crate) fn reset_navigation(&mut self) {
    self.history_cursor = None;
    self.saved_draft = None;
  }

  pub(crate) fn clear(&mut self) {
    self.local_history.clear();
    self.reset_navigation();
  }
}

#[cfg(test)]
mod tests {
  use pretty_assertions::assert_eq;

  use super::ChatComposerHistory;
  use super::HistoryEntry;

  #[test]
  fn push_deduplicates_adjacent() {
    let mut history = ChatComposerHistory::new();
    history.push("hello".into());
    history.push("hello".into());
    assert_eq!(history.local_history.len(), 1);
  }

  #[test]
  fn navigate_prev_saves_draft_entry() {
    let mut history = ChatComposerHistory::new();
    history.push("first".into());
    history.push("second".into());
    let result = history.navigate_prev_entry(HistoryEntry::new("draft".into()));
    assert_eq!(
      result.as_ref().map(|entry| entry.text.as_str()),
      Some("second")
    );
    assert_eq!(
      history.saved_draft_entry().map(|entry| entry.text.as_str()),
      Some("draft")
    );
  }

  #[test]
  fn navigate_next_returns_to_draft() {
    let mut history = ChatComposerHistory::new();
    history.push("msg".into());
    history.navigate_prev("draft");
    let result = history.navigate_next();
    assert_eq!(result, None);
    assert_eq!(history.saved_draft(), "draft");
  }
}
