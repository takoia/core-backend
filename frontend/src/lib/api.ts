// Thin API client. All calls are same-origin: in dev Vite proxies /api to the
// Rust backend, in production the backend serves this bundle directly.

export interface Health {
  status: string;
  service: string;
  version: string;
}

async function getJson<T>(path: string): Promise<T> {
  const res = await fetch(path, { headers: { Accept: "application/json" } });
  if (!res.ok) {
    throw new Error(`${res.status} ${res.statusText}`);
  }
  return (await res.json()) as T;
}

export const api = {
  health: () => getJson<Health>("/api/health"),
};
