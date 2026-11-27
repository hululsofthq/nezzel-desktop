import { useI18n } from 'vue-i18n';
import { useCatchStore } from '../../stores/catch';
import type { CatchCandidate, CatchFormat, CatchTab } from '../../tauri/types/catch';

/** Words for candidates and formats: "Video 1080p" / "Audio only", format name muted beside them. */
export function useCatchText() {
  const { t } = useI18n();
  const store = useCatchStore();
  const candidateTitle = (c: CatchCandidate) => c.title?.trim()
    || (c.main ? store.state?.title?.trim() : '')
    || t('catch.picker.untitled', { site: store.state?.host ?? '' });
  const formatLabel = (f: CatchFormat) => f.label === 'audio'
    ? t('catch.picker.audio')
    : f.height ? t('catch.picker.video', { height: f.height }) : t('catch.picker.videoAny');
  /** The bar's quality button: just the height, or the audio label. */
  const shortLabel = (f: CatchFormat | null) => !f
    ? t('catch.picker.best')
    : f.label === 'audio' ? t('catch.picker.audio') : f.height ? `${f.height}p` : t('catch.picker.videoAny');
  /** A tab's name: its page title, else its site, else "New tab" (the home page). */
  const tabTitle = (tab: Pick<CatchTab, 'title' | 'host' | 'url'>) => tab.title?.trim() || tab.host || (tab.url ? tab.url : t('catch.shell.newTab'));
  return { candidateTitle, formatLabel, shortLabel, tabTitle };
}
