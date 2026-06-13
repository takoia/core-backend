<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "./api";
  import { t } from "./i18n";
  import Icon from "./Icon.svelte";
  import MemoryGraph from "./MemoryGraph.svelte";

  let stats: Record<string, string> = {};
  let topics: { topic: string; count: number }[] = [];
  let agentMemories: { key: string; content: string; created_at?: string }[] = [];
  let selectedTopic: string | null = null;
  let expandedMem: number | null = null;
  let busy = false;

  function agentId(topic: string): string | null {
    return topic.startsWith("takoia/agent/") ? topic.slice("takoia/agent/".length) : null;
  }

  async function load() {
    const o = await api.memoryOverview();
    stats = o.stats;
    topics = o.topics;
  }

  async function view(topic: string) {
    selectedTopic = topic;
    const id = agentId(topic);
    agentMemories = id ? await api.memories(id) : [];
  }

  async function purge(topic: string) {
    if (!confirm(`${$t("memory.confirmPurge")}\n${topic}`)) return;
    busy = true;
    try {
      await api.memoryPurge(topic);
      if (selectedTopic === topic) {
        selectedTopic = null;
        agentMemories = [];
      }
      await load();
    } finally {
      busy = false;
    }
  }

  onMount(load);
</script>

<div class="card">
  <h2>{$t("memory.title")} <span class="muted small">— {$t("memory.subtitle")}</span></h2>
  <div class="stats">
    <div class="stat"><span class="n">{stats.memories ?? "0"}</span><span class="l">{$t("memory.memories")}</span></div>
    <div class="stat"><span class="n">{stats.topics ?? "0"}</span><span class="l">{$t("memory.topics")}</span></div>
    <div class="stat"><span class="n">{stats.avg_weight ?? "—"}</span><span class="l">{$t("memory.avgWeight")}</span></div>
    <div class="stat"><span class="n small">{stats.newest ?? "—"}</span><span class="l">{$t("memory.newest")}</span></div>
  </div>
</div>

<MemoryGraph />

<div class="card">
  <h2>{$t("memory.topicsTitle")}</h2>
  {#if topics.length === 0}
    <p class="muted small">{$t("memory.empty")}</p>
  {:else}
    <table>
      <thead><tr><th>{$t("memory.topic")}</th><th>{$t("memory.count")}</th><th></th></tr></thead>
      <tbody>
        {#each topics as tp}
          <tr>
            <td class="topic">{agentId(tp.topic) ?? tp.topic}</td>
            <td><span class="badge">{tp.count}</span></td>
            <td class="actions">
              <button on:click={() => view(tp.topic)}><Icon name="agents" size={14} /> {$t("memory.viewBtn")}</button>
              <button class="danger" on:click={() => purge(tp.topic)} disabled={busy}>{$t("memory.purge")}</button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

{#if selectedTopic}
  <div class="card">
    <h2>{$t("memory.entriesTitle")} <span class="muted small">{agentId(selectedTopic) ?? selectedTopic}</span></h2>
    {#if agentMemories.length === 0}
      <p class="muted small">{$t("memory.noEntries")}</p>
    {:else}
      {#each agentMemories as m, i}
        <div class="mem" class:open={expandedMem === i} on:click={() => (expandedMem = expandedMem === i ? null : i)} role="button" tabindex="0">
          <span class="muted small">{expandedMem === i ? "▾" : "▸"} {m.key}{m.created_at ? " · " + m.created_at.slice(0, 19).replace("T", " ") : ""}</span>
          {#if expandedMem === i}
            <pre class="memfull">{m.content}</pre>
          {:else}
            <div class="memshort">{m.content.slice(0, 160)}{m.content.length > 160 ? "…" : ""}</div>
          {/if}
        </div>
      {/each}
    {/if}
  </div>
{/if}

<style>
  .stats { display: flex; gap: 1.5rem; flex-wrap: wrap; margin-top: 0.5rem; }
  .stat { display: flex; flex-direction: column; }
  .stat .n { font-size: 1.8rem; font-weight: 600; }
  .stat .n.small { font-size: 0.95rem; }
  .stat .l { color: var(--muted); font-size: 0.78rem; }
  table { width: 100%; border-collapse: collapse; font-size: 0.85rem; }
  th { text-align: left; color: var(--muted); font-weight: 500; padding: 0.4rem 0.5rem; border-bottom: 1px solid var(--border); }
  td { padding: 0.5rem; border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent); }
  .topic { font-family: ui-monospace, monospace; font-size: 0.8rem; }
  .badge { background: var(--accent); color: #04231a; border-radius: 20px; padding: 0.1rem 0.5rem; font-size: 0.75rem; }
  .actions { display: flex; gap: 0.4rem; }
  button { display: inline-flex; align-items: center; gap: 0.3rem; border: 1px solid var(--border); background: #1a2133; color: var(--text); border-radius: 7px; padding: 0.3rem 0.6rem; cursor: pointer; font: inherit; font-size: 0.78rem; }
  button.danger { background: var(--err); border-color: var(--err); color: #2a0707; }
  .mem { padding: 0.5rem 0.4rem; border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent); cursor: pointer; border-radius: 8px; }
  .mem:hover { background: color-mix(in srgb, var(--accent) 8%, transparent); }
  .mem.open { background: color-mix(in srgb, var(--accent) 6%, transparent); }
  .memshort { font-size: 0.85rem; }
  .memfull { margin: 0.4rem 0 0; white-space: pre-wrap; word-break: break-word; font-family: ui-monospace, monospace; font-size: 0.78rem; max-height: 340px; overflow-y: auto; background: var(--bg); border: 1px solid var(--border); border-radius: 8px; padding: 0.6rem; }
  .small { font-size: 0.78rem; }
</style>
