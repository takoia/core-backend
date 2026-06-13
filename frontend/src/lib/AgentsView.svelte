<script lang="ts">
  import { api, type Agent } from "./api";

  export let agents: Agent[] = [];
  export let onChanged: () => void = () => {};

  let tomlText = `[agent]
id = "trading-expert"
name = "Trading Expert"
author = "You — trading desk"
version = "0.1.0"
description = "Expert trading agent that researches and reports market setups."
expertise = "trading"
autonomy = "confirm_before_action"
visibility = "public"
price_per_run_usd = 2.0
emit = ["report.ready"]

[steps.action]
allowed_tools = ["web_search"]
`;
  let importMsg = "";
  let memoriesFor: string | null = null;
  let memories: { key: string; content: string }[] = [];

  async function importToml() {
    importMsg = "";
    try {
      const r = await api.importToml(tomlText);
      importMsg = `imported: ${r.id}`;
      onChanged();
    } catch (e) {
      importMsg = e instanceof Error ? e.message : String(e);
    }
  }

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
</script>

<div class="card">
  <h2>Your expert agents</h2>
  <table>
    <thead><tr><th>Agent</th><th>Expertise</th><th>Author</th><th>Autonomy</th><th>Visibility</th><th>Runs</th><th></th></tr></thead>
    <tbody>
      {#each agents as a}
        <tr>
          <td><strong>{a.name}</strong><br /><span class="muted small">{a.id}</span></td>
          <td>{a.expertise_domain || "—"}</td>
          <td class="small">{a.author || "—"}</td>
          <td>{a.autonomy_level === "full_auto" ? "auto" : "approval"}</td>
          <td>
            <span class="badge {a.visibility}">{a.visibility}</span>
            {#if a.price_per_run_usd > 0}<span class="muted small"> ${a.price_per_run_usd}</span>{/if}
          </td>
          <td>{a.runs_count}</td>
          <td class="actions">
            {#if a.visibility === "public"}
              <button on:click={() => publish(a, "private")}>Make private</button>
            {:else}
              <button on:click={() => publish(a, "public")}>Publish</button>
            {/if}
            <button on:click={() => showMemories(a)}>Memory</button>
            <button on:click={() => exportToml(a)}>Export TOML</button>
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>

{#if memoriesFor}
  <div class="card">
    <h2>Accumulated expertise (memory)</h2>
    {#if memories.length === 0}
      <p class="muted">No memories yet — they grow each time the agent runs.</p>
    {:else}
      {#each memories as m}
        <div class="mem"><span class="muted small">{m.key}</span><div>{m.content.slice(0, 240)}</div></div>
      {/each}
    {/if}
  </div>
{/if}

<div class="card">
  <h2>Import a declarative agent (TOML)</h2>
  <p class="muted small">An agent is a file. Wire two agents by matching one's <code>emit</code> to another's <code>trigger.on</code>.</p>
  <textarea rows="14" bind:value={tomlText}></textarea>
  <div class="row">
    <button class="primary" on:click={importToml}>Import</button>
    {#if importMsg}<span class="muted small">{importMsg}</span>{/if}
  </div>
</div>

<style>
  table { width: 100%; border-collapse: collapse; font-size: 0.85rem; }
  th { text-align: left; color: var(--muted); font-weight: 500; padding: 0.4rem 0.5rem; border-bottom: 1px solid var(--border); }
  td { padding: 0.5rem; border-bottom: 1px solid #161c2a; vertical-align: top; }
  .actions { display: flex; gap: 0.35rem; flex-wrap: wrap; }
  button { border: 1px solid var(--border); background: #1a2133; color: var(--text); border-radius: 7px; padding: 0.3rem 0.6rem; cursor: pointer; font: inherit; font-size: 0.78rem; }
  button.primary { background: var(--accent); border-color: var(--accent); color: #fff; }
  textarea { width: 100%; background: #0b0e14; border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.6rem; font-family: ui-monospace, monospace; font-size: 0.8rem; }
  .row { display: flex; gap: 0.8rem; align-items: center; margin-top: 0.6rem; }
  .badge { font-size: 0.7rem; padding: 0.1rem 0.45rem; border-radius: 20px; background: var(--border); }
  .badge.public { background: var(--ok); color: #04231a; }
  .small { font-size: 0.78rem; }
  .mem { padding: 0.4rem 0; border-bottom: 1px solid #161c2a; }
</style>
