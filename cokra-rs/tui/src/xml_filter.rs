const FILTERED_XML_TAGS: &[&str] = &[
  "tool_call",
  "tool_response",
  "function_call",
  "function_response",
];

#[derive(Debug, Default)]
pub(crate) struct XmlToolFilter {
  buffer: String,
  in_tag: bool,
  tag_name: Option<&'static str>,
}

impl XmlToolFilter {
  pub(crate) fn new() -> Self {
    Self::default()
  }

  /// 处理输入 delta，返回应该显示的文本。
  ///
  /// 某些模型会在文本输出中以内联 XML 形式表示工具调用，例如
  /// `<tool_call>{"name":"spawn_agent",...}</tool_call>`，这些不应显示在聊天面板中。
  pub(crate) fn filter(&mut self, delta: &str) -> String {
    self.buffer.push_str(delta);

    let mut output = String::new();
    loop {
      if self.in_tag {
        let Some(tag_name) = self.tag_name else {
          self.in_tag = false;
          continue;
        };
        let close_tag = format!("</{}>", tag_name);
        if let Some(end_pos) = self.buffer.find(&close_tag) {
          let remove_end = end_pos + close_tag.len();
          self.buffer.drain(..remove_end);
          self.in_tag = false;
          self.tag_name = None;
          continue;
        }

        // Tradeoff: if a model emits a malformed/unterminated tool tag, we would rather
        // drop the buffered tool payload than leak it into the transcript.
        if self.buffer.len() > 32 * 1024 {
          self.buffer.clear();
          self.in_tag = false;
          self.tag_name = None;
        }
        break;
      }

      let Some((start_pos, tag_name)) = find_next_open_tag(&self.buffer) else {
        // 没有匹配的标签：尽可能把文本向外输出，只在出现 '<' 时保留尾部以处理跨-delta 的标签起始。
        if let Some(last_lt) = self.buffer.rfind('<') {
          if last_lt > 0 {
            output.push_str(&self.buffer[..last_lt]);
            self.buffer.drain(..last_lt);
          }
        } else if !self.buffer.is_empty() {
          output.push_str(&self.buffer);
          self.buffer.clear();
        }
        break;
      };

      let open_tag_prefix = format!("<{}", tag_name);
      let rest = &self.buffer[start_pos + open_tag_prefix.len()..];
      let Some(tag_end_pos) = rest.find('>') else {
        // 标签开头存在但尚未完整：输出标签之前的文本，保留剩余等待更多 delta。
        if start_pos > 0 {
          output.push_str(&self.buffer[..start_pos]);
          self.buffer.drain(..start_pos);
        }
        break;
      };

      // 输出标签之前的文本
      if start_pos > 0 {
        output.push_str(&self.buffer[..start_pos]);
      }

      // 进入过滤模式并丢弃开标签本身。
      let open_end = start_pos + open_tag_prefix.len() + tag_end_pos + 1;
      self.buffer.drain(..open_end);
      self.in_tag = true;
      self.tag_name = Some(tag_name);
    }

    output
  }

  /// Flush 所有剩余缓冲内容（在 turn 结束时调用）。
  pub(crate) fn flush(&mut self) -> String {
    if self.in_tag {
      // 仍处于工具标签中：丢弃剩余缓冲，避免泄露。
      self.buffer.clear();
      self.in_tag = false;
      self.tag_name = None;
      return String::new();
    }

    self.in_tag = false;
    self.tag_name = None;
    let mut remaining = std::mem::take(&mut self.buffer);
    if let Some((pos, _)) = find_next_open_tag(&remaining) {
      remaining.truncate(pos);
    }
    remaining
  }
}

pub(crate) fn strip_inline_xml_tool_tags(text: &str) -> String {
  let mut result = text.to_string();
  for tag_name in FILTERED_XML_TAGS {
    loop {
      let open_prefix = format!("<{}", tag_name);
      let close_tag = format!("</{}>", tag_name);
      let Some(start) = result.find(&open_prefix) else {
        break;
      };
      let Some(open_end_rel) = result[start..].find('>') else {
        break;
      };
      let search_from = start + open_end_rel + 1;
      let Some(end_rel) = result[search_from..].find(&close_tag) else {
        break;
      };
      let remove_end = search_from + end_rel + close_tag.len();
      result.replace_range(start..remove_end, "");
    }
  }
  result
}

fn find_next_open_tag(buffer: &str) -> Option<(usize, &'static str)> {
  let mut best: Option<(usize, &'static str)> = None;
  for tag_name in FILTERED_XML_TAGS {
    let open = format!("<{}", tag_name);
    if let Some(pos) = buffer.find(&open) {
      match best {
        None => best = Some((pos, *tag_name)),
        Some((best_pos, _)) if pos < best_pos => best = Some((pos, *tag_name)),
        _ => {}
      }
    }
  }
  best
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn strip_removes_inline_tool_tags() {
    let input = "hello<tool_call>{\"name\":\"x\"}</tool_call>world";
    assert_eq!(strip_inline_xml_tool_tags(input), "helloworld");
  }

  #[test]
  fn streaming_filter_handles_cross_delta_tool_calls() {
    let mut f = XmlToolFilter::new();
    let out1 = f.filter("hi<tool_c");
    let out2 = f.filter("all>{}</tool_call>world");
    let out3 = f.flush();
    assert_eq!(format!("{out1}{out2}{out3}"), "hiworld");
  }

  #[test]
  fn streaming_filter_does_not_remove_unrelated_angle_brackets() {
    let mut f = XmlToolFilter::new();
    let out1 = f.filter("a < b > c");
    let out2 = f.flush();
    assert_eq!(format!("{out1}{out2}"), "a < b > c");
  }

  #[test]
  fn flush_drops_unterminated_tool_tag_payload() {
    let mut f = XmlToolFilter::new();
    let out1 = f.filter("<tool_call>payload");
    let out2 = f.flush();
    assert_eq!(format!("{out1}{out2}"), "");
  }
}
