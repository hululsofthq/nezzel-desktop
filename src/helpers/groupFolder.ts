export interface GroupFolderInput {
  /** Files this link yields in total (playlist entries picked, photos/videos picked from a post). */
  files: number;
  /** Title of the playlist, album or post. */
  title?: string;
  uploader?: string;
  /** Source id (playlist or post), used when there is no title. */
  id?: string;
  url?: string;
}

/**
 * Template values that tell the engine to save this download in a folder named after its group.
 * Nothing for a single file; the engine applies the name only while the folder layout is the default.
 */
export function groupFolderValues(input: GroupFolderInput): Record<string, string> {
  if (input.files < 2) return {};
  return { nezzel_group_files: String(input.files), nezzel_group_title: groupFolderTitle(input) };
}

export function groupFolderTitle({ title, uploader, id, url }: GroupFolderInput): string {
  const named = title?.trim();
  if (named) return named;
  const byId = [uploader?.trim(), id?.trim()].filter(Boolean).join(' - ');
  if (byId) return byId;
  try {
    return new URL(url ?? '').hostname.replace(/^www\./, '');
  } catch {
    return '';
  }
}
