const CLOUD_KEY = 'carbon-treemap-include-cloud';

/** Cloud-sync folders are skipped unless the user turned this on. */
export function loadIncludeCloud(): boolean {
  try {
    return localStorage.getItem(CLOUD_KEY) === 'true';
  } catch {
    return false;
  }
}

export function saveIncludeCloud(value: boolean): void {
  try {
    localStorage.setItem(CLOUD_KEY, String(value));
  } catch {
    // Storage can be unavailable; the setting then lasts for this session only.
  }
}
