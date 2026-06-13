<script lang="ts">
  import { onMount } from "svelte";
  import { api, type McpServer } from "./api";
  import Logo from "./Logo.svelte";

  let servers: McpServer[] = [];
  let installed: { name: string; connected: boolean }[] = [];
  let query = "";
  let category = "All";
  let busy: string | null = null;
  let toast = "";

  $: categories = ["All", ...Array.from(new Set(servers.map((s) => s.category)))];
  $: filtered = servers.filter(
    (s) =>
      (category === "All" || s.category === category) &&
      (query === "" ||
        s.name.toLowerCase().includes(query.toLowerCase()) ||
        s.description.toLowerCase().includes(query.toLowerCase())),
  );

  function isConnected(s: McpServer): boolean {
    return installed.some(
      (i) => i.connected && (i.name === s.id || i.name.toLowerCase().includes(s.id)),
    );
  }

  async function load() {
    servers = await api.mcpCatalog();
    try {
      installed = await api.mcpInstalled();
    } catch {
      installed = [];
    }
  }

  async function connect(s: McpServer) {
    busy = s.id;
    toast = "";
    try {
      const r = await api.mcpConnect(s.id);
      toast = `${s.name}: ${r.message}`;
      installed = await api.mcpInstalled();
    } catch (e) {
      toast = `${s.name}: ${e instanceof Error ? e.message : e}`;
    } finally {
      busy = null;
    }
  }

  onMount(load);
</script>

<div class="card">
  <div class="head">
    <h2>MCP servers <span class="muted small">— {servers.length} connectors</span></h2>
    <div class="filters">
      <input placeholder="Search…" bind:value={query} />
      <select bind:value={category}>
        {#each categories as c}<option>{c}</option>{/each}
      </select>
    </div>
  </div>
  {#if toast}<p class="toast">{toast}</p>{/if}

  <div class="grid">
    {#each filtered as s}
      <div class="mcp">
        <div class="top">
          <Logo slug={s.logo} label={s.name} />
          <div class="meta">
            <strong>{s.name}</strong>
            <span class="cat">{s.category} · {s.transport}</span>
          </div>
          {#if isConnected(s)}<span class="badge ok">connected</span>{/if}
        </div>
        <p class="desc">{s.description}</p>
        <div class="actions">
          <button on:click={() => connect(s)} disabled={busy === s.id || isConnected(s)}>
            {busy === s.id ? "…" : isConnected(s) ? "✓ Connected" : "Connect"}
          </button>
          <a href={s.docs} target="_blank" rel="noreferrer" class="docs">GitHub ↗</a>
        </div>
      </div>
    {/each}
  </div>
</div>

<style>
  .head { display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 0.6rem; }
  .filters { display: flex; gap: 0.5rem; }
  input, select { background: #0b0e14; border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.45rem 0.6rem; font: inherit; }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(230px, 1fr)); gap: 0.8rem; margin-top: 1rem; }
  .mcp { border: 1px solid var(--border); border-radius: 10px; padding: 0.8rem; background: #0f1422; display: flex; flex-direction: column; }
  .top { display: flex; align-items: center; gap: 0.6rem; }
  .meta { display: flex; flex-direction: column; line-height: 1.2; flex: 1; }
  .cat { color: var(--muted); font-size: 0.72rem; }
  .desc { color: var(--muted); font-size: 0.8rem; margin: 0.5rem 0; flex: 1; }
  .actions { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; }
  button { border: 1px solid var(--accent); background: var(--accent); color: #fff; border-radius: 7px; padding: 0.35rem 0.7rem; cursor: pointer; font: inherit; font-size: 0.8rem; }
  button:disabled { background: #1a2133; border-color: var(--border); color: var(--muted); cursor: default; }
  .docs { color: var(--muted); font-size: 0.76rem; text-decoration: none; }
  .badge.ok { background: var(--ok); color: #04231a; font-size: 0.68rem; padding: 0.1rem 0.45rem; border-radius: 20px; }
  .toast { background: #11203a; border: 1px solid var(--border); padding: 0.5rem 0.7rem; border-radius: 8px; font-size: 0.82rem; }
  .small { font-size: 0.78rem; }
</style>
