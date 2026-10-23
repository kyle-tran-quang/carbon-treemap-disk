import type { Crumb } from '../ipc';

export function joinPath(parent: string, name: string): string {
  return parent.endsWith('/') ? `${parent}${name}` : `${parent}/${name}`;
}

/** Absolute path of the last crumb. The first crumb is the scan root's full path. */
export function crumbPath(crumbs: readonly Crumb[]): string {
  return crumbs.reduce(
    (path, crumb, index) => (index === 0 ? crumb.name : joinPath(path, crumb.name)),
    '',
  );
}

/** Last path segment, or the whole path for `/`. */
export function baseName(path: string): string {
  const trimmed = path.length > 1 ? path.replace(/\/+$/, '') : path;
  const slash = trimmed.lastIndexOf('/');
  if (slash === -1 || trimmed === '/') return trimmed;
  return trimmed.slice(slash + 1) || trimmed;
}
