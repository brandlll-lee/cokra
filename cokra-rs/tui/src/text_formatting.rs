pub(crate) fn capitalize_first(input: &str) -> String {
  let mut chars = input.chars();
  match chars.next() {
    Some(first) => {
      let mut capitalized = first.to_uppercase().collect::<String>();
      capitalized.push_str(chars.as_str());
      capitalized
    }
    None => String::new(),
  }
}
