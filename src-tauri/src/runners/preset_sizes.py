"""Select options-sheet streams from formats that yt-dlp has already extracted."""
import json
from contextlib import suppress
from pathlib import Path

from yt_dlp.postprocessor.common import PostProcessor


class NezzelPresetSizesPP(PostProcessor):
    def select(self, info, args, height=0, fps=0):
        config = self.config
        selector = args[args.index('-f') + 1].replace(config['height_token'], str(height)).replace(config['fps_token'], str(fps))
        sort = args[args.index('-S') + 1].replace(config['height_token'], str(height)).replace(config['fps_token'], str(fps))
        # Apply the download's sort to a copy. The probe's own format selection still uses its original list.
        selection = {**info, 'formats': [dict(fmt) for fmt in info['formats']]}
        downloader = self._downloader
        params = downloader.params
        previous = {key: params[key] for key in ('format_sort', 'check_formats') if key in params}
        try:
            params['format_sort'] = sort.split(',')
            params['check_formats'] = False
            downloader.sort_formats(selection)
            selected = downloader._select_formats(selection['formats'], downloader.build_format_selector(selector))
        finally:
            params.pop('format_sort', None)
            params.pop('check_formats', None)
            params.update(previous)
        return selected[0] if len(selected) == 1 else None

    def video_ids(self, info, tier):
        height, fps = tier
        config = self.config
        if config['video_reencodes']:
            return None
        key = 'video_fps' if fps else 'video' if height else 'video_unbounded'
        selected = self.select(info, config[key], height, fps)
        if not selected:
            return None
        container = config['remux'] or selected.get('ext')
        codec = (selected.get('vcodec') or '').lower()
        if config['check_quicktime'] and container == 'mp4' and not codec.startswith(
            ('avc1', 'avc3', 'h264', 'hvc1', 'hev1', 'hevc', 'h265')
        ):
            return None
        streams = selected.get('requested_formats') or [selected]
        return [stream['format_id'] for stream in streams]

    def audio_selection(self, info):
        selected = self.select(info, self.config['audio'])
        if not selected:
            return None, None
        codec = (selected.get('acodec') or '').lower().split('.')[0]
        if codec in ('', 'none'):
            return None, None
        if codec == 'mp4a':
            codec = 'aac'
        target = self.config['audio_target']
        if target == 'ogg':
            target = 'vorbis'
        copies_audio = target in ('best', codec) or (target == 'm4a' and codec == 'aac')
        if copies_audio:
            return [selected['format_id']], None
        bitrate = self.config['audio_target_bitrate']
        # A re-encode with no known bitrate (lossless, custom quality) has no honest estimate.
        return ([selected['format_id']], bitrate) if bitrate else (None, None)

    def run(self, info):
        # Selection is optional metadata. Probe errors must not fail the info read.
        with suppress(Exception):
            info.pop('_nezzel_preset_formats', None)
            info.pop('_nezzel_preset_output', None)
            info.pop('_nezzel_preset_audio_bitrate', None)
            self.config = json.loads(Path(__file__).with_name('presets.json').read_text(encoding='utf-8'))
            self.collect(info)
        return [], info

    def collect(self, info):
        formats = info.get('formats')
        if not formats or info.get('is_live'):
            return

        # Match the video tiers exposed by the parsed format list and the options sheet.
        tiers = {
            (fmt['height'], int((fmt.get('fps') or 0) + 0.5))
            for fmt in formats
            if (fmt.get('height') or 0) > 0 and not fmt.get('abr') and fmt.get('ext') != 'mhtml'
        }
        audio_ids, audio_bitrate = self.audio_selection(info)
        sizes = {'audio': audio_ids}
        if audio_bitrate is not None:
            info['_nezzel_preset_audio_bitrate'] = audio_bitrate
        if tiers:
            best = max(tiers)
            small = max((tier for tier in tiers if tier[0] <= self.config['small_max_height']), default=min(tiers))
            sizes['best'] = self.video_ids(info, best)
            if small[0] != best[0]:
                sizes['small'] = self.video_ids(info, small)
        elif any(fmt.get('vcodec') not in (None, 'none') for fmt in formats):
            sizes['best'] = self.video_ids(info, (0, 0))
            sizes['small'] = self.video_ids(info, (self.config['small_max_height'], 0))
        info.update({
            '_nezzel_preset_formats': {key: ids for key, ids in sizes.items() if ids},
            '_nezzel_preset_output': self.config['output'],
        })
