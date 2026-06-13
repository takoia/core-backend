// Named Discord webhooks, configured globally in Settings and reusable by
// name from the send_discord builder tool. Stored client-side (localStorage):
// the resolved URL is baked into the agent TOML at save time.
export type DiscordHook = { name: string; url: string };

const KEY = "takoia.discordHooks";

export function loadDiscordHooks(): DiscordHook[] {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((h) => h && h.name) : [];
  } catch {
    return [];
  }
}

export function saveDiscordHooks(hooks: DiscordHook[]): void {
  localStorage.setItem(KEY, JSON.stringify(hooks));
}

export function discordUrlByName(name: string): string {
  return loadDiscordHooks().find((h) => h.name === name)?.url ?? "";
}
