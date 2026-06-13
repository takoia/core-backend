<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Agent, type Connector, type UsageTotal } from "./lib/api";
  import RunView from "./lib/RunView.svelte";
  import AgentsView from "./lib/AgentsView.svelte";
  import SettingsView from "./lib/SettingsView.svelte";
  import UsageView from "./lib/UsageView.svelte";

  type View = "run" | "agents" | "settings" | "usage";
  let view: View = "run";
  let healthy = false;

  let agents: Agent[] = [];
  let connectors: Connector[] = [];
  let usageTotals: UsageTotal[] = [];
  let usageTotal = 0;

  async function loadAgents() {
    agents = await api.listAgents();
  }
  async function loadConnectors() {
    connectors = await api.listConnectors();
  }
  async function loadUsage() {
    const u = await api.usage();
    usageTotals = u.totals;
    usageTotal = u.estimated_total_usd;
  }

  onMount(async () => {
    try {
      await api.health();
      healthy = true;
    } catch {
      healthy = false;
    }
    await Promise.all([loadAgents(), loadConnectors(), loadUsage()]);
  });

  // Refresh usage when switching to it.
  $: if (view === "usage") void loadUsage();
</script>

<header>
  <div class="container nav">
    <div class="brand">
      <h1>TakoIA</h1>
      <span class="tag">marketplace of autonomous expert agents</span>
    </div>
    <nav>
      <button class:active={view === "run"} on:click={() => (view = "run")}>Run</button>
      <button class:active={view === "agents"} on:click={() => (view = "agents")}>Agents</button>
      <button class:active={view === "settings"} on:click={() => (view = "settings")}>Settings</button>
      <button class:active={view === "usage"} on:click={() => (view = "usage")}>Usage</button>
      <span class="dot" class:ok={healthy} title={healthy ? "backend online" : "backend offline"}></span>
    </nav>
  </div>
</header>

<main class="container">
  {#if view === "run"}
    <RunView {agents} />
  {:else if view === "agents"}
    <AgentsView {agents} onChanged={loadAgents} />
  {:else if view === "settings"}
    <SettingsView {connectors} onChanged={loadConnectors} />
  {:else if view === "usage"}
    <UsageView totals={usageTotals} estimatedTotal={usageTotal} />
  {/if}
</main>

<style>
  header { border-bottom: 1px solid var(--border); background: #0d111a; position: sticky; top: 0; z-index: 10; }
  .nav { display: flex; align-items: center; justify-content: space-between; padding: 0.9rem 1.5rem; }
  nav { display: flex; align-items: center; gap: 0.4rem; }
  nav button {
    background: transparent; border: 1px solid transparent; color: var(--muted);
    padding: 0.4rem 0.8rem; border-radius: 8px; cursor: pointer; font: inherit;
  }
  nav button:hover { color: var(--text); }
  nav button.active { background: #1a2133; border-color: var(--border); color: var(--text); }
  .dot { width: 10px; height: 10px; border-radius: 50%; background: var(--err); margin-left: 0.5rem; }
  .dot.ok { background: var(--ok); }
  main { padding-top: 1.5rem; padding-bottom: 3rem; display: flex; flex-direction: column; gap: 1.25rem; }
  main > :global(.card) { margin-top: 0; }
</style>
