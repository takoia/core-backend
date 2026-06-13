<script lang="ts">
  import { api, type Agent } from "./api";
  import { t } from "./i18n";

  export let agents: Agent[] = [];
  export let onChanged: () => void = () => {};

  let tab: "private" | "public" = "private";

  let importMsg = "";
  let memoriesFor: string | null = null;
  let memories: { key: string; content: string }[] = [];

  $: visibleAgents = agents.filter((a) => a.visibility === tab);

  async function publish(a: Agent, visibility: string) {
    await api.publishAgent(a.id, visibility, a.price_per_run_usd);
    onChanged();
  }

  async function showMemories(a: Agent) {
    memoriesFor = a.id;
    memories = await api.memories(a.id);
  }

  async function exportToml(a: Agent) {
    const toml = await api.exportToml(a.id);
    await navigator.clipboard?.writeText(toml).catch(() => {});
    importMsg = `exported ${a.id} TOML to clipboard`;
  }

  // Deterministic hue derived from the agent id.
  function hueFromId(id: string): number {
    let hash = 0;
    for (let i = 0; i < id.length; i++) {
      hash = (hash * 31 + id.charCodeAt(i)) >>> 0;
    }
    return hash % 360;
  }

  // Pick an emoji from the expertise keyword, else a monogram from the name.
  function avatarGlyph(a: Agent): string {
    const exp = (a.expertise_domain || "").toLowerCase();
    if (exp.includes("trading")) return "📈";
    if (exp.includes("invoice")) return "🧾";
    if (exp.includes("watch") || exp.includes("veille")) return "🛰️";
    if (exp.includes("email")) return "✉️";
    if (exp) return "🐙";
    return monogram(a.name);
  }

  function monogram(name: string): string {
    const parts = (name || "").trim().split(/\s+/).filter(Boolean);
    if (parts.length === 0) return "🐙";
    if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
    return (parts[0][0] + parts[1][0]).toUpperCase();
  }
</script>

<div class="card">
  <h2>{$t("agents.title")}</h2>

  <div class="tabs">
    <button class="tab" class:active={tab === "private"} on:click={() => (tab = "private")}>
      {$t("agents.tabPrivate")}
    </button>
    <button class="tab" class:active={tab === "public"} on:click={() => (tab = "public")}>
      {$t("agents.tabPublic")}
    </button>
  </div>

  {#if visibleAgents.length === 0}
    <p class="muted small">{$t("agents.empty")}</p>
  {:else}
    <div class="grid">
      {#each visibleAgents as a (a.id)}
        <div class="agent">
          <div class="head">
            <div
              class="avatar"
              style="--h: {hueFromId(a.id)}; background: hsl({hueFromId(a.id)}, 45%, 32%);"
            >
              {avatarGlyph(a)}
            </div>
            <div class="meta">
              <strong>{a.name}</strong>
              <span class="muted small">{a.author || "—"}</span>
            </div>
          </div>

          <div class="tags">
            {#if a.expertise_domain}<span class="badge">{a.expertise_domain}</span>{/if}
            <span class="badge autonomy"
              >{a.autonomy_level === "full_auto"
                ? $t("agents.autoShort")
                : $t("agents.approvalShort")}</span
            >
            <span class="muted small">{a.runs_count} {$t("agents.col.runs").toLowerCase()}</span>
            {#if a.price_per_run_usd > 0}
              <span class="badge price">${a.price_per_run_usd}</span>
            {/if}
          </div>

          <div class="actions">
            {#if a.visibility === "public"}
              <button on:click={() => publish(a, "private")}>{$t("agents.makePrivate")}</button>
            {:else}
              <button on:click={() => publish(a, "public")}>{$t("agents.publish")}</button>
            {/if}
            <button on:click={() => showMemories(a)}>{$t("agents.memory")}</button>
            <button on:click={() => exportToml(a)}>{$t("agents.exportToml")}</button>
          </div>
        </div>
      {/each}
    </div>
  {/if}
</div>

{#if memoriesFor}
  <div class="card">
    <h2>{$t("agents.memoryTitle")}</h2>
    {#if memories.length === 0}
      <p class="muted">{$t("agents.noMemory")}</p>
    {:else}
      {#each memories as m}
        <div class="mem"><span class="muted small">{m.key}</span><div>{m.content.slice(0, 240)}</div></div>
      {/each}
    {/if}
  </div>
{/if}

<style>
  .tabs { display: flex; gap: 0.4rem; margin: 0.6rem 0 1rem; }
  .tab {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--muted);
    border-radius: 8px;
    padding: 0.35rem 0.9rem;
    cursor: pointer;
    font: inherit;
    font-size: 0.82rem;
  }
  .tab.active { background: var(--accent); border-color: var(--accent); color: #fff; }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 0.8rem;
  }
  .agent {
    border: 1px solid var(--border);
    background: var(--panel);
    border-radius: 10px;
    padding: 0.8rem;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  .head { display: flex; align-items: center; gap: 0.6rem; }
  .avatar {
    width: 44px;
    height: 44px;
    flex: 0 0 44px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 1.1rem;
    font-weight: 600;
    color: #fff;
    text-transform: uppercase;
  }
  .meta { display: flex; flex-direction: column; min-width: 0; }
  .meta strong { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }

  .tags { display: flex; gap: 0.4rem; align-items: center; flex-wrap: wrap; }
  .badge {
    font-size: 0.7rem;
    padding: 0.1rem 0.45rem;
    border-radius: 20px;
    background: var(--border);
    color: var(--text);
  }
  .badge.autonomy { background: var(--accent); color: #fff; }
  .badge.price { background: var(--ok); color: #04231a; }

  .actions { display: flex; gap: 0.35rem; flex-wrap: wrap; }
  button {
    border: 1px solid var(--border);
    background: #1a2133;
    color: var(--text);
    border-radius: 7px;
    padding: 0.3rem 0.6rem;
    cursor: pointer;
    font: inherit;
    font-size: 0.78rem;
  }
  button.primary { background: var(--accent); border-color: var(--accent); color: #fff; }

  textarea {
    width: 100%;
    background: #0b0e14;
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 8px;
    padding: 0.6rem;
    font-family: ui-monospace, monospace;
    font-size: 0.8rem;
  }
  .row { display: flex; gap: 0.8rem; align-items: center; margin-top: 0.6rem; }
  .small { font-size: 0.78rem; }
  .mem { padding: 0.4rem 0; border-bottom: 1px solid #161c2a; }
</style>
