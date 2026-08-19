export function apiPath(path: string): string {
  return path.startsWith("/") ? path : `/${path}`;
}
