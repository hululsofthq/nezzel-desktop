use super::{gallery_files::Staging, ytdlp_args};
use crate::{
  models::{
    download::{AudioFormat, FormatOptions, VideoPostprocessPreset},
    TrackType,
  },
  state::config_models::OutputSettings,
};
use serde_json::json;
use std::future::Future;

// Substitute the tier found in the extractor's format list into the normal download arguments.
const HEIGHT: u32 = u32::MAX;
const FPS: u32 = u32::MAX - 1;

fn audio_target_bitrate(
  format: AudioFormat,
  quality: Option<&str>,
  postprocess_args: Option<&str>,
) -> Option<f64> {
  // Only lossy re-encode profiles get --audio-quality. yt-dlp interprets 0–10 as encoder quality.
  let quality = quality?.trim_end_matches(['k', 'K']).parse::<f64>().ok()?;
  let args = postprocess_args.and_then(shlex::split).unwrap_or_default();
  let last_option = |names: &[&str]| {
    args
      .windows(2)
      .rfind(|pair| names.contains(&pair[0].as_str()))
      .map(|pair| pair[1].as_str())
  };
  let custom_quality = last_option(&["-q:a", "-q:a:0", "-qscale:a", "-aq"]);
  if custom_quality.is_some_and(|value| value != "-1")
    || last_option(&["-c:a", "-c:a:0", "-codec:a", "-acodec"]).is_some()
  {
    return None;
  }
  // MP3 at --audio-quality 0–9 is LAME VBR -V0…-V9 (the default is 0). Average bitrates from
  // the LAME presets; an explicit -b:a is handled below.
  if matches!(format, AudioFormat::Mp3)
    && (0.0..=10.0).contains(&quality)
    && custom_quality.is_none()
    && last_option(&["-b:a", "-b:a:0", "-ab"]).is_none()
  {
    const LAME_VBR_KBPS: [f64; 11] = [
      245.0, 225.0, 190.0, 175.0, 165.0, 130.0, 115.0, 100.0, 85.0, 65.0, 65.0,
    ];
    return Some(LAME_VBR_KBPS[quality.round() as usize] * 1_000.0);
  }
  if quality <= 10.0
    && !matches!(format, AudioFormat::Opus)
    && !(matches!(format, AudioFormat::Mp3 | AudioFormat::Ogg) && custom_quality == Some("-1"))
  {
    // An explicit bitrate alone does not turn off the encoder's VBR quality setting.
    return None;
  }
  let bitrate = match last_option(&["-b:a", "-b:a:0", "-ab"]) {
    Some(value) => {
      let (number, multiplier) = if let Some(number) = value.strip_suffix('k') {
        (number, 1_000.0)
      } else if let Some(number) = value.strip_suffix('M') {
        (number, 1_000_000.0)
      } else {
        (value, 1.0)
      };
      number.parse::<f64>().ok()? * multiplier
    }
    None if quality > 10.0 => quality * 1_000.0,
    None => return None,
  };
  (bitrate.is_finite() && bitrate > 0.0).then_some(bitrate)
}

pub(crate) struct PresetProbe(Staging);

impl PresetProbe {
  pub(crate) fn new(url: &str, output: &OutputSettings, keep_original_codec: bool) -> Option<Self> {
    let audio = FormatOptions {
      track_type: TrackType::Audio,
      abr: None,
      height: None,
      fps: None,
      audio_encoding: None,
      video_encoding: None,
      audio_track: None,
      video_track: None,
    };
    let video = FormatOptions {
      track_type: TrackType::Both,
      height: Some(HEIGHT),
      ..audio.clone()
    };
    let video_args = ytdlp_args::build_output_args(&video, output, None).ok()?;
    let audio_args = ytdlp_args::build_output_args(&audio, output, None).ok()?;
    let option = |args: &[String], name: &str| {
      args
        .windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
    };
    let audio_postprocess = option(&audio_args, "--postprocessor-args")
      .and_then(|value| value.strip_prefix("ffmpeg:").map(str::to_owned));
    let audio_bitrate = audio_target_bitrate(
      output.audio.format,
      option(&audio_args, "--audio-quality").as_deref(),
      audio_postprocess.as_deref(),
    );
    let config = json!({
      "video": ytdlp_args::build_format_args_for(url, &video, output),
      "video_fps": ytdlp_args::build_format_args_for(url, &FormatOptions { fps: Some(FPS), ..video.clone() }, output),
      "video_unbounded": ytdlp_args::build_format_args_for(url, &FormatOptions { height: None, ..video }, output),
      "audio": ytdlp_args::build_format_args_for(url, &audio, output),
      "height_token": HEIGHT.to_string(),
      "fps_token": FPS.to_string(),
      "small_max_height": crate::capture::preview::SMALL_MAX_HEIGHT,
      "video_reencodes": video_args.iter().any(|arg| arg == "--recode-video")
        || (output.video.postprocess_preset == VideoPostprocessPreset::Custom
          && !output.video.postprocess_args.trim().is_empty()),
      "remux": option(&video_args, "--remux-video"),
      "audio_target": option(&audio_args, "--audio-format").unwrap_or_else(|| "best".into()),
      "audio_target_bitrate": audio_bitrate,
      "check_quicktime": cfg!(target_os = "macos") && !keep_original_codec,
      // The sheet drops a cached estimate if the output profile has changed since extraction.
      "output": [
        output.video.container, output.video.policy, output.video.postprocess_preset,
        output.video.custom_postprocess_mode, !output.video.postprocess_args.trim().is_empty(),
        keep_original_codec, output.audio.format, output.audio.policy, output.audio.postprocess_preset,
        audio_postprocess.map_or_else(|| json!(false), |args| json!(args)),
      ],
    });
    let staging = Staging::new().ok()?;
    staging.path.to_str()?;
    let path = staging.path.join("nezzel/yt_dlp_plugins/postprocessor");
    std::fs::create_dir_all(&path).ok()?;
    std::fs::write(path.join("presets.json"), config.to_string()).ok()?;
    std::fs::write(
      path.join("nezzel_sizes.py"),
      include_str!("preset_sizes.py"),
    )
    .ok()?;
    Some(Self(staging))
  }

  pub(crate) fn args(&self) -> [&str; 4] {
    [
      "--plugin-dirs",
      self.0.path.to_str().unwrap_or_default(),
      "--use-postprocessor",
      "NezzelPresetSizes:when=after_filter",
    ]
  }

  pub(crate) fn startup_failed(stderr: &str) -> bool {
    stderr.contains("NezzelPresetSizesPP") || stderr.contains("nezzel_sizes")
  }

  pub(crate) async fn run_with_fallback<T, F: Future<Output = T>>(
    probe: Option<&Self>,
    mut run: impl FnMut(Option<&Self>) -> F,
    probe_failed: impl FnOnce(&T) -> bool,
  ) -> T {
    let result = run(probe).await;
    if probe.is_some() && probe_failed(&result) {
      tracing::warn!(kind = "exit", "preset sizes probe failed");
      return run(None).await;
    }
    result
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::models::download::{AudioFormat, AudioPostprocessPreset, TranscodePolicy};
  use serde_json::Value;

  #[test]
  fn stages_the_configured_bitrate_for_lossy_audio_reencoding() {
    for (format, policy, custom_args, expected) in [
      (
        AudioFormat::Mp3,
        TranscodePolicy::AllowReencode,
        "",
        Some(245_000.0),
      ),
      (
        AudioFormat::Mp3,
        TranscodePolicy::AllowReencode,
        "-q:a 2",
        None,
      ),
      (
        AudioFormat::Mp3,
        TranscodePolicy::AllowReencode,
        "-b:a 192k",
        None,
      ),
      (
        AudioFormat::Mp3,
        TranscodePolicy::AllowReencode,
        "-b:a 192k -q:a -1",
        Some(192_000.0),
      ),
      (
        AudioFormat::Opus,
        TranscodePolicy::AllowReencode,
        "-b:a 192000 -vbr off",
        Some(192_000.0),
      ),
      (
        AudioFormat::Flac,
        TranscodePolicy::AllowReencode,
        "-b:a 192k",
        None,
      ),
      (AudioFormat::Mp3, TranscodePolicy::Never, "-b:a 192k", None),
    ] {
      let mut output = OutputSettings::default();
      output.audio.format = format;
      output.audio.policy = policy;
      output.audio.postprocess_preset = AudioPostprocessPreset::Custom;
      output.audio.postprocess_args = custom_args.into();
      let probe = PresetProbe::new("https://example.com/v", &output, false).unwrap();
      let config = probe
        .0
        .path
        .join("nezzel/yt_dlp_plugins/postprocessor/presets.json");
      let config: Value = serde_json::from_slice(&std::fs::read(config).unwrap()).unwrap();
      assert_eq!(
        config["audio_target_bitrate"].as_f64(),
        expected,
        "{format:?} {policy:?} {custom_args}"
      );
    }
  }

  #[tokio::test]
  async fn retries_a_failed_probe_once_without_its_arguments() {
    let probe = PresetProbe::new("https://example.com/v", &OutputSettings::default(), false)
      .expect("stage preset probe");
    let success = (0, "");
    let ordinary_failure = (
      1,
      "ERROR: [generic] Unable to download webpage: HTTP Error 404",
    );
    let missing_processor = (1, "KeyError: 'NezzelPresetSizesPP'");
    let broken_import = (
      1,
      "Error while importing module 'yt_dlp_plugins.postprocessor.nezzel_sizes'",
    );
    for (attached, first, fallback, expected, expected_calls) in [
      (
        true,
        ordinary_failure,
        success,
        ordinary_failure,
        &[true][..],
      ),
      (
        true,
        missing_processor,
        success,
        success,
        &[true, false][..],
      ),
      (
        true,
        broken_import,
        missing_processor,
        missing_processor,
        &[true, false][..],
      ),
      (true, success, ordinary_failure, success, &[true][..]),
      (
        false,
        missing_processor,
        success,
        missing_processor,
        &[false][..],
      ),
    ] {
      let mut calls = Vec::new();
      let result = PresetProbe::run_with_fallback(
        attached.then_some(&probe),
        |probe| {
          let result = if calls.is_empty() { first } else { fallback };
          calls.push(probe.is_some());
          std::future::ready(result)
        },
        |(code, stderr)| *code != 0 && PresetProbe::startup_failed(stderr),
      )
      .await;
      assert_eq!(result, expected);
      assert_eq!(calls, expected_calls);
    }
  }
}
