mod format_args;
mod input_filter_args;
mod location_args;
mod output_args;

pub use format_args::build_format_args_for;
pub use input_filter_args::{active_filter_kinds, build_input_filter_args, filters_active};
pub use location_args::{
  build_location_args, default_download_dir, downloads_dir, filename_metadata_args, group_dir_name,
  output_base_dir, path_preferences_for, temporary_location_args, widen_group,
};
pub use output_args::build_output_args;

#[cfg(test)]
mod tests;
