use crate::render::line_utils::line_to_static;
use crate::terminal_palette::light_blue;
use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_line;
use pulldown_cmark::CodeBlockKind;
use pulldown_cmark::CowStr;
use pulldown_cmark::Event;
use pulldown_cmark::HeadingLevel;
use pulldown_cmark::Options;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use pulldown_cmark::TagEnd;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

struct MarkdownStyles {
  h1: Style,
  h2: Style,
  h3: Style,
  h4: Style,
  h5: Style,
  h6: Style,
  code: Style,
  emphasis: Style,
  strong: Style,
  strikethrough: Style,
  ordered_list_marker: Style,
  unordered_list_marker: Style,
  link: Style,
  blockquote: Style,
  table_border: Style,
}

impl Default for MarkdownStyles {
  fn default() -> Self {
    Self {
      h1: Style::new().bold().underlined(),
      h2: Style::new().bold(),
      h3: Style::new().bold().italic(),
      h4: Style::new().italic(),
      h5: Style::new().italic(),
      h6: Style::new().italic(),
      code: Style::new().fg(light_blue()),
      emphasis: Style::new().italic(),
      strong: Style::new().bold(),
      strikethrough: Style::new().crossed_out(),
      ordered_list_marker: Style::new().fg(light_blue()),
      unordered_list_marker: Style::new(),
      link: Style::new().fg(light_blue()).underlined(),
      blockquote: Style::new().green(),
      table_border: Style::new().dim(),
    }
  }
}

#[derive(Clone, Debug)]
struct TableRenderState {
  rows: Vec<Vec<String>>,
  current_row: Vec<String>,
  current_cell: String,
  in_head: bool,
  header_rows: usize,
  alignments: Vec<pulldown_cmark::Alignment>,
}

#[derive(Clone, Debug)]
struct IndentContext {
  prefix: Vec<Span<'static>>,
  marker: Option<Vec<Span<'static>>>,
  is_list: bool,
}

impl IndentContext {
  fn new(prefix: Vec<Span<'static>>, marker: Option<Vec<Span<'static>>>, is_list: bool) -> Self {
    Self {
      prefix,
      marker,
      is_list,
    }
  }
}

pub fn render_markdown_text(input: &str) -> Text<'static> {
  render_markdown_text_with_width(input, None)
}

pub(crate) fn render_markdown_text_with_width(input: &str, width: Option<usize>) -> Text<'static> {
  let mut options = Options::empty();
  options.insert(Options::ENABLE_STRIKETHROUGH);
  options.insert(Options::ENABLE_TABLES);
  let parser = Parser::new_ext(input, options);
  let mut w = Writer::new(parser, width);
  w.run();
  w.text
}

struct Writer<'a, I>
where
  I: Iterator<Item = Event<'a>>,
{
  iter: I,
  text: Text<'static>,
  styles: MarkdownStyles,
  inline_styles: Vec<Style>,
  indent_stack: Vec<IndentContext>,
  list_indices: Vec<Option<u64>>,
  link: Option<String>,
  needs_newline: bool,
  pending_marker_line: bool,
  in_paragraph: bool,
  in_code_block: bool,
  wrap_width: Option<usize>,
  current_line_content: Option<Line<'static>>,
  current_initial_indent: Vec<Span<'static>>,
  current_subsequent_indent: Vec<Span<'static>>,
  current_line_style: Style,
  current_line_preformatted: bool,
  table_state: Option<TableRenderState>,
}

impl<'a, I> Writer<'a, I>
where
  I: Iterator<Item = Event<'a>>,
{
  fn new(iter: I, wrap_width: Option<usize>) -> Self {
    Self {
      iter,
      text: Text::default(),
      styles: MarkdownStyles::default(),
      inline_styles: Vec::new(),
      indent_stack: Vec::new(),
      list_indices: Vec::new(),
      link: None,
      needs_newline: false,
      pending_marker_line: false,
      in_paragraph: false,
      in_code_block: false,
      wrap_width,
      current_line_content: None,
      current_initial_indent: Vec::new(),
      current_subsequent_indent: Vec::new(),
      current_line_style: Style::default(),
      current_line_preformatted: false,
      table_state: None,
    }
  }

  fn run(&mut self) {
    while let Some(ev) = self.iter.next() {
      self.handle_event(ev);
    }
    self.flush_current_line();
  }

  fn handle_event(&mut self, event: Event<'a>) {
    match event {
      Event::Start(tag) => self.start_tag(tag),
      Event::End(tag) => self.end_tag(tag),
      Event::Text(text) => self.text(text),
      Event::Code(code) => self.code(code),
      Event::SoftBreak => self.soft_break(),
      Event::HardBreak => self.hard_break(),
      Event::Rule => {
        self.flush_current_line();
        if !self.text.lines.is_empty() {
          self.push_blank_line();
        }
        self.push_line(Line::from("———"));
        self.needs_newline = true;
      }
      Event::Html(html) => self.html(html, false),
      Event::InlineHtml(html) => self.html(html, true),
      Event::FootnoteReference(_) => {}
      Event::TaskListMarker(_) => {}
      Event::InlineMath(text) => self.text(text),
      Event::DisplayMath(text) => {
        self.flush_current_line();
        self.text(text);
        self.flush_current_line();
        self.needs_newline = true;
      }
    }
  }

  fn start_tag(&mut self, tag: Tag<'a>) {
    match tag {
      Tag::Paragraph => self.start_paragraph(),
      Tag::Heading { level, .. } => self.start_heading(level),
      Tag::BlockQuote(_) => self.start_blockquote(),
      Tag::CodeBlock(kind) => {
        let indent = match kind {
          CodeBlockKind::Fenced(_) => None,
          CodeBlockKind::Indented => Some(Span::from(" ".repeat(4))),
        };
        let lang = match kind {
          CodeBlockKind::Fenced(lang) => Some(lang.to_string()),
          CodeBlockKind::Indented => None,
        };
        self.start_codeblock(lang, indent)
      }
      Tag::List(start) => self.start_list(start),
      Tag::Item => self.start_item(),
      Tag::Emphasis => self.push_inline_style(self.styles.emphasis),
      Tag::Strong => self.push_inline_style(self.styles.strong),
      Tag::Strikethrough => self.push_inline_style(self.styles.strikethrough),
      Tag::Link { dest_url, .. } => self.push_link(dest_url.to_string()),
      Tag::Table(alignments) => self.start_table(alignments),
      Tag::TableHead => self.start_table_head(),
      Tag::TableRow => self.start_table_row(),
      Tag::TableCell => self.start_table_cell(),
      Tag::HtmlBlock
      | Tag::FootnoteDefinition(_)
      | Tag::Image { .. }
      | Tag::MetadataBlock(_)
      | Tag::DefinitionList
      | Tag::DefinitionListTitle
      | Tag::DefinitionListDefinition
      | Tag::Superscript
      | Tag::Subscript => {}
    }
  }

  fn end_tag(&mut self, tag: TagEnd) {
    match tag {
      TagEnd::Paragraph => self.end_paragraph(),
      TagEnd::Heading(_) => self.end_heading(),
      TagEnd::BlockQuote(_) => self.end_blockquote(),
      TagEnd::CodeBlock => self.end_codeblock(),
      TagEnd::List(_) => self.end_list(),
      TagEnd::Item => {
        self.indent_stack.pop();
        self.pending_marker_line = false;
      }
      TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => self.pop_inline_style(),
      TagEnd::Link => self.pop_link(),
      TagEnd::Table => self.end_table(),
      TagEnd::TableHead => self.end_table_head(),
      TagEnd::TableRow => self.end_table_row(),
      TagEnd::TableCell => self.end_table_cell(),
      TagEnd::HtmlBlock
      | TagEnd::FootnoteDefinition
      | TagEnd::Image
      | TagEnd::MetadataBlock(_)
      | TagEnd::DefinitionList
      | TagEnd::DefinitionListTitle
      | TagEnd::DefinitionListDefinition
      | TagEnd::Superscript
      | TagEnd::Subscript => {}
    }
  }

  fn start_paragraph(&mut self) {
    if self.needs_newline {
      self.push_blank_line();
    }
    self.push_line(Line::default());
    self.needs_newline = false;
    self.in_paragraph = true;
  }

  fn end_paragraph(&mut self) {
    self.needs_newline = true;
    self.in_paragraph = false;
    self.pending_marker_line = false;
  }

  fn start_heading(&mut self, level: HeadingLevel) {
    if self.needs_newline {
      self.push_line(Line::default());
      self.needs_newline = false;
    }
    let heading_style = match level {
      HeadingLevel::H1 => self.styles.h1,
      HeadingLevel::H2 => self.styles.h2,
      HeadingLevel::H3 => self.styles.h3,
      HeadingLevel::H4 => self.styles.h4,
      HeadingLevel::H5 => self.styles.h5,
      HeadingLevel::H6 => self.styles.h6,
    };
    let content = format!("{} ", "#".repeat(level as usize));
    self.push_line(Line::from(vec![Span::styled(content, heading_style)]));
    self.push_inline_style(heading_style);
    self.needs_newline = false;
  }

  fn end_heading(&mut self) {
    self.needs_newline = true;
    self.pop_inline_style();
  }

  fn start_blockquote(&mut self) {
    if self.needs_newline {
      self.push_blank_line();
      self.needs_newline = false;
    }
    self
      .indent_stack
      .push(IndentContext::new(vec![Span::from("> ")], None, false));
  }

  fn end_blockquote(&mut self) {
    self.indent_stack.pop();
    self.needs_newline = true;
  }

  fn text(&mut self, text: CowStr<'a>) {
    if let Some(table_state) = self.table_state.as_mut() {
      table_state.current_cell.push_str(&text);
      return;
    }
    if self.pending_marker_line {
      self.push_line(Line::default());
    }
    self.pending_marker_line = false;
    if self.in_code_block && !self.needs_newline {
      let has_content = self
        .current_line_content
        .as_ref()
        .map(|line| !line.spans.is_empty())
        .unwrap_or_else(|| {
          self
            .text
            .lines
            .last()
            .map(|line| !line.spans.is_empty())
            .unwrap_or(false)
        });
      if has_content {
        self.push_line(Line::default());
      }
    }
    for (i, line) in text.lines().enumerate() {
      if self.needs_newline {
        self.push_line(Line::default());
        self.needs_newline = false;
      }
      if i > 0 {
        self.push_line(Line::default());
      }
      let content = line.to_string();
      let span = Span::styled(
        content,
        self.inline_styles.last().copied().unwrap_or_default(),
      );
      self.push_span(span);
    }
    self.needs_newline = false;
  }

  fn code(&mut self, code: CowStr<'a>) {
    if let Some(table_state) = self.table_state.as_mut() {
      table_state.current_cell.push_str(&code);
      return;
    }
    if self.pending_marker_line {
      self.push_line(Line::default());
      self.pending_marker_line = false;
    }
    let span = Span::from(code.into_string()).style(self.styles.code);
    self.push_span(span);
  }

  fn html(&mut self, html: CowStr<'a>, inline: bool) {
    self.pending_marker_line = false;
    for (i, line) in html.lines().enumerate() {
      if self.needs_newline {
        self.push_line(Line::default());
        self.needs_newline = false;
      }
      if i > 0 {
        self.push_line(Line::default());
      }
      let style = self.inline_styles.last().copied().unwrap_or_default();
      self.push_span(Span::styled(line.to_string(), style));
    }
    self.needs_newline = !inline;
  }

  fn hard_break(&mut self) {
    if let Some(table_state) = self.table_state.as_mut() {
      table_state.current_cell.push(' ');
      return;
    }
    self.push_line(Line::default());
  }

  fn soft_break(&mut self) {
    if let Some(table_state) = self.table_state.as_mut() {
      table_state.current_cell.push(' ');
      return;
    }
    self.push_line(Line::default());
  }

  fn start_table(&mut self, alignments: Vec<pulldown_cmark::Alignment>) {
    self.flush_current_line();
    if self.needs_newline {
      self.push_blank_line();
      self.needs_newline = false;
    }
    self.table_state = Some(TableRenderState {
      rows: Vec::new(),
      current_row: Vec::new(),
      current_cell: String::new(),
      in_head: false,
      header_rows: 0,
      alignments,
    });
  }

  fn start_table_head(&mut self) {
    if let Some(ref mut state) = self.table_state {
      state.in_head = true;
      state.current_row = Vec::new();
    }
  }

  fn end_table_head(&mut self) {
    if let Some(ref mut state) = self.table_state {
      if !state.current_row.is_empty() {
        let row = std::mem::take(&mut state.current_row);
        state.header_rows = state.header_rows.saturating_add(1);
        state.rows.push(row);
      }
      state.in_head = false;
    }
  }

  fn start_table_row(&mut self) {
    if let Some(ref mut state) = self.table_state {
      state.current_row = Vec::new();
    }
  }

  fn end_table_row(&mut self) {
    if let Some(ref mut state) = self.table_state {
      let row = std::mem::take(&mut state.current_row);
      if state.in_head {
        state.header_rows = state.header_rows.saturating_add(1);
      }
      state.rows.push(row);
    }
  }

  fn start_table_cell(&mut self) {
    if let Some(ref mut state) = self.table_state {
      state.current_cell = String::new();
    }
  }

  fn end_table_cell(&mut self) {
    if let Some(ref mut state) = self.table_state {
      let cell = std::mem::take(&mut state.current_cell);
      let cell = cell
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
      state.current_row.push(cell);
    }
  }

  fn end_table(&mut self) {
    let Some(state) = self.table_state.take() else {
      return;
    };
    if state.rows.is_empty() {
      return;
    }

    let col_count = state.rows.iter().map(|row| row.len()).max().unwrap_or(0);
    if col_count == 0 {
      return;
    }

    let mut col_widths = vec![0usize; col_count];
    for row in &state.rows {
      for (i, cell) in row.iter().enumerate() {
        if i < col_count {
          let width = UnicodeWidthStr::width(cell.as_str());
          col_widths[i] = col_widths[i].max(width);
        }
      }
    }
    for width in &mut col_widths {
      *width = (*width).max(3);
    }

    if let Some(max_width) = self.wrap_width {
      // Tables are rendered as fixed-width box drawing output. When tables are nested inside a
      // list or blockquote, the line prefix consumes part of the available width. Clamp using the
      // post-prefix width to avoid clipping in the UI.
      let prefix = self.prefix_spans(false);
      let prefix_width = Line::from(prefix).width();
      let available = max_width.saturating_sub(prefix_width).max(1);
      clamp_table_widths(&mut col_widths, available);
    }

    let border_style = self.styles.table_border;

    let top = render_table_border(&col_widths, '┌', '┬', '┐', '─');
    self.push_preformatted_line(Line::from(Span::styled(top, border_style)));

    let header_rows = if state.header_rows > 0 {
      state.header_rows.min(state.rows.len())
    } else if state.rows.len() > 1 {
      // Tradeoff: if the markdown omits an explicit table head, treat the first row as a header
      // so the output matches common GitHub-flavored markdown expectations.
      1
    } else {
      0
    };

    for (row_idx, row) in state.rows.iter().enumerate() {
      let mut spans = Vec::new();
      spans.push(Span::styled("│", border_style));
      for col_idx in 0..col_count {
        let width = col_widths[col_idx];
        let cell_text = row.get(col_idx).map(|s| s.as_str()).unwrap_or("");
        let alignment = state
          .alignments
          .get(col_idx)
          .copied()
          .unwrap_or(pulldown_cmark::Alignment::Left);
        let padded = pad_cell(cell_text, width, alignment);
        spans.push(Span::from(format!(" {} ", padded)));
        spans.push(Span::styled("│", border_style));
      }
      self.push_preformatted_line(Line::from(spans));

      if header_rows > 0 && row_idx + 1 == header_rows && row_idx + 1 < state.rows.len() {
        let sep = render_table_border(&col_widths, '├', '┼', '┤', '─');
        self.push_preformatted_line(Line::from(Span::styled(sep, border_style)));
      }
    }

    let bottom = render_table_border(&col_widths, '└', '┴', '┘', '─');
    self.push_preformatted_line(Line::from(Span::styled(bottom, border_style)));
    self.needs_newline = true;
  }

  fn start_list(&mut self, index: Option<u64>) {
    if self.list_indices.is_empty() && self.needs_newline {
      self.push_line(Line::default());
    }
    self.list_indices.push(index);
  }

  fn end_list(&mut self) {
    self.list_indices.pop();
    self.needs_newline = true;
  }

  fn start_item(&mut self) {
    self.pending_marker_line = true;
    let depth = self.list_indices.len();
    let is_ordered = self
      .list_indices
      .last()
      .map(Option::is_some)
      .unwrap_or(false);
    let width = depth * 4 - 3;
    let marker = if let Some(last_index) = self.list_indices.last_mut() {
      match last_index {
        None => Some(vec![Span::styled(
          " ".repeat(width - 1) + "- ",
          self.styles.unordered_list_marker,
        )]),
        Some(index) => {
          *index += 1;
          Some(vec![Span::styled(
            format!("{:width$}. ", *index - 1),
            self.styles.ordered_list_marker,
          )])
        }
      }
    } else {
      None
    };
    let indent_prefix = if depth == 0 {
      Vec::new()
    } else {
      let indent_len = if is_ordered { width + 2 } else { width + 1 };
      vec![Span::from(" ".repeat(indent_len))]
    };
    self
      .indent_stack
      .push(IndentContext::new(indent_prefix, marker, true));
    self.needs_newline = false;
  }

  fn start_codeblock(&mut self, _lang: Option<String>, indent: Option<Span<'static>>) {
    self.flush_current_line();
    if !self.text.lines.is_empty() {
      self.push_blank_line();
    }
    self.in_code_block = true;
    self.indent_stack.push(IndentContext::new(
      vec![indent.unwrap_or_default()],
      None,
      false,
    ));
    self.needs_newline = true;
  }

  fn end_codeblock(&mut self) {
    self.needs_newline = true;
    self.in_code_block = false;
    self.indent_stack.pop();
  }

  fn push_inline_style(&mut self, style: Style) {
    let current = self.inline_styles.last().copied().unwrap_or_default();
    let merged = current.patch(style);
    self.inline_styles.push(merged);
  }

  fn pop_inline_style(&mut self) {
    self.inline_styles.pop();
  }

  fn push_link(&mut self, dest_url: String) {
    self.link = Some(dest_url);
  }

  fn pop_link(&mut self) {
    if let Some(link) = self.link.take() {
      self.push_span(" (".into());
      self.push_span(Span::styled(link, self.styles.link));
      self.push_span(")".into());
    }
  }

  fn flush_current_line(&mut self) {
    if let Some(line) = self.current_line_content.take() {
      let style = self.current_line_style;
      // NB we don't wrap preformatted lines (code blocks, tables), in order to preserve the
      // expected layout and whitespace for copy/paste.
      if !self.current_line_preformatted
        && let Some(width) = self.wrap_width
      {
        let opts = RtOptions::new(width)
          .initial_indent(self.current_initial_indent.clone().into())
          .subsequent_indent(self.current_subsequent_indent.clone().into());
        for wrapped in word_wrap_line(&line, opts) {
          let owned = line_to_static(&wrapped).style(style);
          self.text.lines.push(owned);
        }
      } else {
        let mut spans = self.current_initial_indent.clone();
        let mut line = line;
        spans.append(&mut line.spans);
        self.text.lines.push(Line::from_iter(spans).style(style));
      }
      self.current_initial_indent.clear();
      self.current_subsequent_indent.clear();
      self.current_line_preformatted = false;
    }
  }

  fn push_line(&mut self, line: Line<'static>) {
    self.push_line_with_preformatted(line, self.in_code_block);
  }

  fn push_preformatted_line(&mut self, line: Line<'static>) {
    self.push_line_with_preformatted(line, true);
  }

  fn push_line_with_preformatted(&mut self, line: Line<'static>, preformatted: bool) {
    self.flush_current_line();
    let blockquote_active = self
      .indent_stack
      .iter()
      .any(|ctx| ctx.prefix.iter().any(|s| s.content.contains('>')));
    let style = if blockquote_active {
      self.styles.blockquote
    } else {
      line.style
    };
    let was_pending = self.pending_marker_line;

    self.current_initial_indent = self.prefix_spans(was_pending);
    self.current_subsequent_indent = self.prefix_spans(false);
    self.current_line_style = style;
    self.current_line_content = Some(line);
    self.current_line_preformatted = preformatted;

    self.pending_marker_line = false;
  }

  fn push_span(&mut self, span: Span<'static>) {
    if let Some(line) = self.current_line_content.as_mut() {
      line.push_span(span);
    } else {
      self.push_line(Line::from(vec![span]));
    }
  }

  fn push_blank_line(&mut self) {
    self.flush_current_line();
    if self.indent_stack.iter().all(|ctx| ctx.is_list) {
      self.text.lines.push(Line::default());
    } else {
      self.push_line(Line::default());
      self.flush_current_line();
    }
  }

  fn prefix_spans(&self, pending_marker_line: bool) -> Vec<Span<'static>> {
    let mut prefix: Vec<Span<'static>> = Vec::new();
    let last_marker_index = if pending_marker_line {
      self
        .indent_stack
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, ctx)| if ctx.marker.is_some() { Some(i) } else { None })
    } else {
      None
    };
    let last_list_index = self.indent_stack.iter().rposition(|ctx| ctx.is_list);

    for (i, ctx) in self.indent_stack.iter().enumerate() {
      if pending_marker_line {
        if Some(i) == last_marker_index
          && let Some(marker) = &ctx.marker
        {
          prefix.extend(marker.iter().cloned());
          continue;
        }
        if ctx.is_list && last_marker_index.is_some_and(|idx| idx > i) {
          continue;
        }
      } else if ctx.is_list && Some(i) != last_list_index {
        continue;
      }
      prefix.extend(ctx.prefix.iter().cloned());
    }

    prefix
  }
}

fn render_table_border(
  col_widths: &[usize],
  left: char,
  mid: char,
  right: char,
  fill: char,
) -> String {
  let mut s = String::new();
  s.push(left);
  for (i, width) in col_widths.iter().enumerate() {
    for _ in 0..(*width + 2) {
      s.push(fill);
    }
    if i + 1 < col_widths.len() {
      s.push(mid);
    }
  }
  s.push(right);
  s
}

fn clamp_table_widths(col_widths: &mut [usize], max_width: usize) {
  let col_count = col_widths.len();
  if col_count == 0 {
    return;
  }

  let min_width = 3usize;
  let fixed_overhead = 1 + 3 * col_count; // 1 leading │ and per-col " x │"
  let mut total = fixed_overhead + col_widths.iter().sum::<usize>();
  if total <= max_width {
    return;
  }

  let min_total = fixed_overhead + min_width * col_count;
  if min_total > max_width {
    return;
  }

  while total > max_width {
    let mut widest_idx = None;
    let mut widest = min_width;
    for (idx, width) in col_widths.iter().copied().enumerate() {
      if width > widest {
        widest = width;
        widest_idx = Some(idx);
      }
    }
    let Some(idx) = widest_idx else {
      break;
    };
    if col_widths[idx] <= min_width {
      break;
    }
    col_widths[idx] -= 1;
    total -= 1;
  }
}

fn pad_cell(text: &str, width: usize, alignment: pulldown_cmark::Alignment) -> String {
  let mut content = text.replace('\t', " ");
  if UnicodeWidthStr::width(content.as_str()) > width {
    content = truncate_to_width(content.as_str(), width);
  }
  let content_width = UnicodeWidthStr::width(content.as_str());
  let padding = width.saturating_sub(content_width);
  let (left_pad, right_pad) = match alignment {
    pulldown_cmark::Alignment::Left => (0, padding),
    pulldown_cmark::Alignment::Center => (padding / 2, padding - padding / 2),
    pulldown_cmark::Alignment::Right => (padding, 0),
    pulldown_cmark::Alignment::None => (0, padding),
  };
  format!(
    "{}{}{}",
    " ".repeat(left_pad),
    content,
    " ".repeat(right_pad)
  )
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
  if UnicodeWidthStr::width(text) <= max_width {
    return text.to_string();
  }
  let mut out = String::new();
  let mut width = 0usize;
  for ch in text.chars() {
    let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
    if width + ch_width > max_width {
      break;
    }
    out.push(ch);
    width += ch_width;
  }
  out
}

#[cfg(test)]
mod markdown_render_tests {
  include!("markdown_render_tests.rs");
}

#[cfg(test)]
mod tests {
  use super::*;
  use pretty_assertions::assert_eq;
  use ratatui::text::Text;

  fn lines_to_strings(text: &Text<'_>) -> Vec<String> {
    text
      .lines
      .iter()
      .map(|l| {
        l.spans
          .iter()
          .map(|s| s.content.clone())
          .collect::<String>()
      })
      .collect()
  }

  #[test]
  fn wraps_plain_text_when_width_provided() {
    let markdown = "This is a simple sentence that should wrap.";
    let rendered = render_markdown_text_with_width(markdown, Some(16));
    let lines = lines_to_strings(&rendered);
    assert_eq!(
      lines,
      vec![
        "This is a simple".to_string(),
        "sentence that".to_string(),
        "should wrap.".to_string(),
      ]
    );
  }

  #[test]
  fn wraps_list_items_preserving_indent() {
    let markdown = "- first second third fourth";
    let rendered = render_markdown_text_with_width(markdown, Some(14));
    let lines = lines_to_strings(&rendered);
    assert_eq!(
      lines,
      vec!["- first second".to_string(), "  third fourth".to_string(),]
    );
  }

  #[test]
  fn wraps_nested_lists() {
    let markdown =
      "- outer item with several words to wrap\n  - inner item that also needs wrapping";
    let rendered = render_markdown_text_with_width(markdown, Some(20));
    let lines = lines_to_strings(&rendered);
    assert_eq!(
      lines,
      vec![
        "- outer item with".to_string(),
        "  several words to".to_string(),
        "  wrap".to_string(),
        "    - inner item".to_string(),
        "      that also".to_string(),
        "      needs wrapping".to_string(),
      ]
    );
  }

  #[test]
  fn wraps_ordered_lists() {
    let markdown = "1. ordered item contains many words for wrapping";
    let rendered = render_markdown_text_with_width(markdown, Some(18));
    let lines = lines_to_strings(&rendered);
    assert_eq!(
      lines,
      vec![
        "1. ordered item".to_string(),
        "   contains many".to_string(),
        "   words for".to_string(),
        "   wrapping".to_string(),
      ]
    );
  }

  #[test]
  fn wraps_blockquotes() {
    let markdown = "> block quote with content that should wrap nicely";
    let rendered = render_markdown_text_with_width(markdown, Some(22));
    let lines = lines_to_strings(&rendered);
    assert_eq!(
      lines,
      vec![
        "> block quote with".to_string(),
        "> content that should".to_string(),
        "> wrap nicely".to_string(),
      ]
    );
  }

  #[test]
  fn wraps_blockquotes_inside_lists() {
    let markdown = "- list item\n  > block quote inside list that wraps";
    let rendered = render_markdown_text_with_width(markdown, Some(24));
    let lines = lines_to_strings(&rendered);
    assert_eq!(
      lines,
      vec![
        "- list item".to_string(),
        "  > block quote inside".to_string(),
        "  > list that wraps".to_string(),
      ]
    );
  }

  #[test]
  fn wraps_list_items_containing_blockquotes() {
    let markdown = "1. item with quote\n   > quoted text that should wrap";
    let rendered = render_markdown_text_with_width(markdown, Some(24));
    let lines = lines_to_strings(&rendered);
    assert_eq!(
      lines,
      vec![
        "1. item with quote".to_string(),
        "   > quoted text that".to_string(),
        "   > should wrap".to_string(),
      ]
    );
  }

  #[test]
  fn does_not_wrap_code_blocks() {
    let markdown = "````\nfn main() { println!(\"hi from a long line\"); }\n````";
    let rendered = render_markdown_text_with_width(markdown, Some(10));
    let lines = lines_to_strings(&rendered);
    assert_eq!(
      lines,
      vec!["fn main() { println!(\"hi from a long line\"); }".to_string(),]
    );
  }
}
