<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Agent, type Connector, type UsageTotal } from "./lib/api";
  import RunView from "./lib/RunView.svelte";
  import AgentsView from "./lib/AgentsView.svelte";
  import SettingsView from "./lib/SettingsView.svelte";
  import UsageView from "./lib/UsageView.svelte";
  import McpView from "./lib/McpView.svelte";
  import SkillsView from "./lib/SkillsView.svelte";
  import CanvasView from "./lib/CanvasView.svelte";
  import BuilderView from "./lib/BuilderView.svelte";
  import LogsView from "./lib/LogsView.svelte";
  import LoginView from "./lib/LoginView.svelte";
  import { t, locale, setLocale } from "./lib/i18n";
  import "./lib/theme"; // applies the persisted theme on load
  import logo from "./lib/assets/takoia.png";
  import Icon from "./lib/Icon.svelte";

  type View = "run" | "agents" | "builder" | "canvas" | "mcp" | "skills" | "logs" | "settings" | "usage";
  let view: View = "run";
  let healthy = false;
  let token: string | null = localStorage.getItem("auth_token");

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

  async function boot() {
    try {
      await api.health();
      healthy = true;
    } catch {
      healthy = false;
    }
    await Promise.all([loadAgents(), loadConnectors(), loadUsage()]);
  }

  function onAuthed(tk: string) {
    token = tk;
    boot();
  }

  function logout() {
    localStorage.removeItem("auth_token");
    token = null;
  }

  onMount(() => {
    if (token) boot();
  });

  $: if (view === "usage") void loadUsage();
</script>

{#if !token}
  <LoginView {onAuthed} />
{:else}
  <header>
    <div class="container nav">
      <div class="brand">
        <img class="logo" src={logo} alt="TakoIA" />
        <h1>TakoIA</h1>
      </div>
      <nav>
        <button class:active={view === "run"} on:click={() => (view = "run")}><Icon name="run" />{$t("nav.run")}</button>
        <button class:active={view === "agents"} on:click={() => (view = "agents")}><Icon name="agents" />{$t("nav.agents")}</button>
        <button class:active={view === "builder"} on:click={() => (view = "builder")}><Icon name="builder" />{$t("nav.builder")}</button>
        <button class:active={view === "canvas"} on:click={() => (view = "canvas")}><Icon name="canvas" />Canvas</button>
        <button class:active={view === "mcp"} on:click={() => (view = "mcp")}><Icon name="mcp" />{$t("nav.mcp")}</button>
        <button class:active={view === "skills"} on:click={() => (view = "skills")}><Icon name="skills" />{$t("nav.skills")}</button>
        <button class:active={view === "logs"} on:click={() => (view = "logs")}><Icon name="logs" />{$t("nav.logs")}</button>
        <button class:active={view === "settings"} on:click={() => (view = "settings")}><Icon name="settings" />{$t("nav.settings")}</button>
        <button class:active={view === "usage"} on:click={() => (view = "usage")}><Icon name="usage" />{$t("nav.usage")}</button>
        <span class="sep"></span>
        <button class="lang" class:on={$locale === "fr"} on:click={() => setLocale("fr")}>FR</button>
        <button class="lang" class:on={$locale === "en"} on:click={() => setLocale("en")}>EN</button>
        <button class="logout" on:click={logout} title={$t("login.logout")}>⎋</button>
        <span class="dot" class:ok={healthy} title={healthy ? $t("nav.online") : $t("nav.offline")}></span>
      </nav>
    </div>
  </header>

  {#if view === "canvas"}
    <CanvasView />
  {:else}
    <main class="container">
      {#if view === "run"}
        <RunView {agents} />
      {:else if view === "agents"}
        <AgentsView {agents} onChanged={loadAgents} />
      {:else if view === "builder"}
        <BuilderView />
      {:else if view === "mcp"}
        <McpView />
      {:else if view === "skills"}
        <SkillsView />
      {:else if view === "logs"}
        <LogsView />
      {:else if view === "settings"}
        <SettingsView {connectors} onChanged={loadConnectors} />
      {:else if view === "usage"}
        <UsageView totals={usageTotals} estimatedTotal={usageTotal} />
      {/if}
    </main>
  {/if}
{/if}

<style>
  header { border-bottom: 1px solid var(--border); background: color-mix(in srgb, var(--panel) 80%, transparent); position: sticky; top: 0; z-index: 10; backdrop-filter: blur(8px); height: 76px; }
  .nav { display: flex; align-items: center; justify-content: space-between; padding: 0.7rem 1.5rem; }
  .brand { display: flex; align-items: center; gap: 0.5rem; }
  .brand .logo { width: 60px; height: 60px; border-radius: 50%; }
  .brand h1 { margin: 0; font-size: 1.2rem; }
  .brand .tag { color: var(--muted); font-size: 0.8rem; }
  nav { display: flex; align-items: center; gap: 0.35rem; }
  nav button { display: inline-flex; align-items: center; gap: 0.35rem; background: transparent; border: 1px solid transparent; color: var(--muted); padding: 0.4rem 0.7rem; border-radius: 8px; cursor: pointer; font: inherit; }
  nav button:hover { color: var(--text); }
  nav button.active { background: color-mix(in srgb, var(--accent) 18%, transparent); border-color: var(--border); color: var(--text); }
  .sep { width: 1px; height: 20px; background: var(--border); margin: 0 0.3rem; }
  .lang { font-size: 0.72rem; padding: 0.25rem 0.5rem !important; }
  .lang.on { color: var(--accent); }
  .logout { font-size: 0.9rem; }
  .dot { width: 10px; height: 10px; border-radius: 50%; background: var(--err); margin-left: 0.4rem; }
  .dot.ok { background: var(--ok); }
  main { padding-top: 1.5rem; padding-bottom: 3rem; display: flex; flex-direction: column; gap: 1.25rem; }
  main > :global(.card) { margin-top: 0; }
</style>
