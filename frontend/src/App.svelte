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
  import MemoryView from "./lib/MemoryView.svelte";
  import VideoView from "./lib/VideoView.svelte";
  import LoginView from "./lib/LoginView.svelte";
  import Toasts from "./lib/Toasts.svelte";
  import Bell from "./lib/Bell.svelte";
  import { t, locale, setLocale } from "./lib/i18n";
  import "./lib/theme"; // applies the persisted theme on load
  import logo from "./lib/assets/takoia.png";
  import Icon from "./lib/Icon.svelte";

  type View = "dashboard" | "video" | "mcp" | "skills" | "memory" | "logs" | "settings" | "usage";
  let view: View = "dashboard";
  let navCollapsed = false;
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
        <button class="burger" on:click={() => (navCollapsed = !navCollapsed)} title="menu">☰</button>
        <button class="logobtn" on:click={() => (view = "dashboard")} title="home">
          <img class="logo" src={logo} alt="TakoIA" />
        </button>
      </div>
      <nav class:collapsed={navCollapsed}>
        <button class:active={view === "dashboard"} on:click={() => (view = "dashboard")}><Icon name="builder" />{$t("nav.dashboard")}</button>
        <button class:active={view === "video"} on:click={() => (view = "video")}><Icon name="video" />{$t("nav.video")}</button>
        <button class:active={view === "mcp"} on:click={() => (view = "mcp")}><Icon name="mcp" />{$t("nav.mcp")}</button>
        <button class:active={view === "skills"} on:click={() => (view = "skills")}><Icon name="skills" />{$t("nav.skills")}</button>
        <button class:active={view === "memory"} on:click={() => (view = "memory")}><Icon name="builder" />{$t("nav.memory")}</button>
        <button class:active={view === "logs"} on:click={() => (view = "logs")}><Icon name="logs" />{$t("nav.logs")}</button>
        <button class:active={view === "settings"} on:click={() => (view = "settings")}><Icon name="settings" />{$t("nav.settings")}</button>
        <button class:active={view === "usage"} on:click={() => (view = "usage")}><Icon name="usage" />{$t("nav.usage")}</button>
        <span class="sep"></span>
        <Bell />
        <button class="lang" class:on={$locale === "fr"} on:click={() => setLocale("fr")}>FR</button>
        <button class="lang" class:on={$locale === "en"} on:click={() => setLocale("en")}>EN</button>
        <button class="logout" on:click={logout} title={$t("login.logout")}>⎋</button>
        <span class="dot" class:ok={healthy} title={healthy ? $t("nav.online") : $t("nav.offline")}></span>
      </nav>
    </div>
  </header>

    {#if view === "dashboard"}
      <BuilderView />
    {:else}
    <main class="container">
      {#if view === "mcp"}
        <McpView />
      {:else if view === "skills"}
        <SkillsView />
      {:else if view === "video"}
        <VideoView />
      {:else if view === "memory"}
        <MemoryView />
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

<Toasts />

<style>
  header { border-bottom: 1px solid var(--border); background: color-mix(in srgb, var(--panel) 80%, transparent); position: sticky; top: 0; z-index: 10; backdrop-filter: blur(8px); height: 90px; }
  .nav { display: flex; align-items: center; justify-content: space-between; gap: 1rem; max-width: none; padding: 0.5rem 1.25rem; }
  .brand { display: flex; align-items: center; gap: 0.5rem; flex: 0 0 auto; }
  .burger { background: transparent; border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.25rem 0.5rem; cursor: pointer; font-size: 1rem; }
  nav.collapsed { display: none; }
  .logobtn { background: none; border: none; padding: 0; cursor: pointer; display: flex; }
  .brand .logo { width: 84px; height: 84px; border-radius: 50%; }
  .brand .tag { color: var(--muted); font-size: 0.8rem; }
  nav { display: flex; align-items: center; gap: 0.2rem; overflow-x: auto; min-width: 0; scrollbar-width: none; }
  nav::-webkit-scrollbar { display: none; }
  nav button { display: inline-flex; align-items: center; gap: 0.3rem; flex: 0 0 auto; white-space: nowrap; background: transparent; border: 1px solid transparent; color: var(--muted); padding: 0.35rem 0.55rem; border-radius: 8px; cursor: pointer; font: inherit; font-size: 0.88rem; }
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
