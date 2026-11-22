use crate::models::download::{InputFilterOptions, PlaylistMode};

pub fn build_input_filter_args(input_filters: Option<&InputFilterOptions>) -> Vec<String> {
  let Some(input_filters) = input_filters else {
    return Vec::new();
  };

  let mut args = Vec::new();

  push_value_arg(&mut args, "-I", input_filters.playlist_items.as_deref());
  push_value_arg(
    &mut args,
    "--min-filesize",
    input_filters.min_filesize.as_deref(),
  );
  push_value_arg(
    &mut args,
    "--max-filesize",
    input_filters.max_filesize.as_deref(),
  );
  push_value_arg(&mut args, "--date", input_filters.date.as_deref());
  push_value_arg(
    &mut args,
    "--datebefore",
    input_filters.datebefore.as_deref(),
  );
  push_value_arg(&mut args, "--dateafter", input_filters.dateafter.as_deref());

  push_value_arg(
    &mut args,
    "--match-filters",
    input_filters.match_filters.as_deref(),
  );
  push_value_arg(
    &mut args,
    "--break-match-filters",
    input_filters.break_match_filters.as_deref(),
  );

  match input_filters.playlist_mode {
    Some(PlaylistMode::SingleVideo) => args.push("--no-playlist".into()),
    Some(PlaylistMode::Playlist) => args.push("--yes-playlist".into()),
    None => {}
  }

  args
}

/// True when any argument can remove videos; playlist-mode flags alone do not.
pub fn filters_active(input_filters: Option<&InputFilterOptions>) -> bool {
  build_input_filter_args(input_filters)
    .iter()
    .any(|arg| arg.starts_with('-') && !matches!(arg.as_str(), "--no-playlist" | "--yes-playlist"))
}

/// Names of the active filter groups (`items`, `size`, `date`, `advanced`), for the row's message.
pub fn active_filter_kinds(input_filters: Option<&InputFilterOptions>) -> Vec<&'static str> {
  let Some(f) = input_filters else {
    return Vec::new();
  };
  let set = |value: &Option<String>| value.as_deref().is_some_and(|v| !v.trim().is_empty());
  [
    ("items", set(&f.playlist_items)),
    ("size", set(&f.min_filesize) || set(&f.max_filesize)),
    (
      "date",
      set(&f.date) || set(&f.datebefore) || set(&f.dateafter),
    ),
    (
      "advanced",
      set(&f.match_filters) || set(&f.break_match_filters),
    ),
  ]
  .into_iter()
  .filter_map(|(kind, active)| active.then_some(kind))
  .collect()
}

fn push_value_arg(args: &mut Vec<String>, flag: &str, value: Option<&str>) {
  let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
    return;
  };
  args.push(flag.into());
  args.push(value.into());
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn only_removing_arguments_count_as_active_filters() {
    let with = |f: fn(&mut InputFilterOptions)| {
      let mut options = InputFilterOptions::default();
      f(&mut options);
      filters_active(Some(&options))
    };
    assert!(!filters_active(None));
    assert!(!filters_active(Some(&InputFilterOptions::default())));
    assert!(!with(|o| o.playlist_mode = Some(PlaylistMode::Playlist)));
    assert!(!with(|o| o.date = Some("  ".into())));
    assert!(with(|o| o.date = Some("20261120".into())));
    assert!(with(|o| o.playlist_items = Some("1-3".into())));
    assert!(with(|o| o.match_filters = Some("duration>60".into())));
  }

  #[test]
  fn active_filter_kinds_name_each_group_once() {
    let options = InputFilterOptions {
      date: Some("20261120".into()),
      datebefore: Some("20261201".into()),
      max_filesize: Some("50M".into()),
      min_filesize: Some(" ".into()),
      playlist_mode: Some(PlaylistMode::Playlist),
      ..Default::default()
    };
    assert_eq!(active_filter_kinds(Some(&options)), ["size", "date"]);
    assert!(active_filter_kinds(None).is_empty());
  }
}
