import type { UpdateChannel } from '../../generated/contract';

export interface UpdaterCheckPayload {
  available: boolean;
  /** The newer version, when one exists. */
  version?: string;
  currentVersion: string;
  /** The manifest `notes` string, unparsed. */
  notes?: string;
  /** A newer version exists but this build cannot update itself in-app: show "Open download page". */
  manual: boolean;
  /** Set when updates come from a store (Microsoft Store, Snap), whether or not a newer version is known. */
  channel?: UpdateChannel;
}

export interface UpdaterDownloadProgressPayload {
  received: number;
  total: number;
}
