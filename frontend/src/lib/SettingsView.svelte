<script lang="ts">
  import { api, type Connector } from "./api";
  import { t } from "./i18n";
  import { THEMES, themeId, setTheme } from "./theme";
  import IntegrationsView from "./IntegrationsView.svelte";

  export let connectors: Connector[] = [];
  export let onChanged: () => void = () => {};

  type Tab = "appearance" | "providers" | "integrations" | "expert" | "account";
  let tab: Tab = "appearance";

  // Provider form
  let name = "";
  let base_url = "";
  let model = "";
  let secret = "";
  let is_default = false;
  let msg = "";

  async function save() {
    msg = "";
    try {
      await api.upsertConnector({ kind: "llm", name, base_url, model, secret, is_default });
      msg = $t("settings.saved");
      name = base_url = model = secret = "";
      is_default = false;
      onChanged();
    } catch (e) {
      msg = e instanceof Error ? e.message : String(e);
    }
  }

  $: llmConnectors = connectors.filter((c) => c.kind === "llm");

  // Expert TOML import
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

[trigger]
on = "market.open"

[steps.action]
allowed_tools = ["web_search"]
`;
  let importMsg = "";
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

  const user = localStorage.getItem("auth_user") ?? "admin";
  function logout() {
    localStorage.removeItem("auth_token");
    window.location.reload();
  }
</script>

<div class="tabs">
  <button class:active={tab === "appearance"} on:click={() => (tab = "appearance")}>{$t("settings.tab.appearance")}</button>
  <button class:active={tab === "providers"} on:click={() => (tab = "providers")}>{$t("settings.tab.providers")}</button>
  <button class:active={tab === "integrations"} on:click={() => (tab = "integrations")}>{$t("settings.tab.integrations")}</button>
  <button class:active={tab === "expert"} on:click={() => (tab = "expert")}>{$t("settings.tab.expert")}</button>
  <button class:active={tab === "account"} on:click={() => (tab = "account")}>{$t("settings.tab.account")}</button>
</div>

{#if tab === "appearance"}
  <div class="card">
    <h2>{$t("settings.theme")}</h2>
    <div class="themes">
      {#each THEMES as th}
        <button
          class="swatch"
          class:active={$themeId === th.id}
          on:click={() => setTheme(th.id)}
          style="--s-bg:{th.vars['--bg']}; --s-panel:{th.vars['--panel']}; --s-accent:{th.vars['--accent']}"
        >
          <span class="dot1"></span><span class="dot2"></span>
          {th.name}
        </button>
      {/each}
    </div>
  </div>

{:else if tab === "providers"}
  <div class="card">
    <h2>{$t("settings.providers")}</h2>
    <p class="muted small">{$t("settings.providersHint")}</p>
    <table>
      <thead><tr>
        <th>{$t("settings.col.name")}</th><th>{$t("settings.col.baseUrl")}</th>
        <th>{$t("settings.col.model")}</th><th>{$t("settings.col.secret")}</th>
        <th>{$t("settings.col.default")}</th>
      </tr></thead>
      <tbody>
        {#each llmConnectors as c}
          <tr>
            <td><strong>{c.name}</strong></td>
            <td class="small">{c.base_url || "—"}</td>
            <td class="small">{c.model || "—"}</td>
            <td>{c.has_secret ? c.secret_hint : "—"}</td>
            <td>{c.is_default ? "★" : ""}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  <div class="card">
    <h2>{$t("settings.addUpdate")}</h2>
    <div class="form">
      <label>{$t("settings.name")} <input bind:value={name} placeholder="claude_max | ollama | gemini | codex" /></label>
      <label>{$t("settings.baseUrl")} <input bind:value={base_url} placeholder="claude-cli | https://…/v1" /></label>
      <label>{$t("settings.model")} <input bind:value={model} placeholder={$t("settings.modelOptional")} /></label>
      <label>{$t("settings.secret")} <input type="password" bind:value={secret} placeholder={$t("settings.secretKept")} /></label>
      <label class="check"><input type="checkbox" bind:checked={is_default} /> {$t("settings.defaultProvider")}</label>
    </div>
    <div class="row">
      <button class="primary" on:click={save} disabled={!name}>{$t("settings.save")}</button>
      {#if msg}<span class="muted small">{msg}</span>{/if}
    </div>
  </div>

{:else if tab === "integrations"}
  <IntegrationsView />

{:else if tab === "expert"}
  <div class="card">
    <h2>{$t("agents.importTitle")}</h2>
    <p class="muted small">{$t("agents.importHint")}</p>
    <textarea rows="16" bind:value={tomlText}></textarea>
    <div class="row">
      <button class="primary" on:click={importToml}>{$t("agents.import")}</button>
      {#if importMsg}<span class="muted small">{importMsg}</span>{/if}
    </div>
  </div>

{:else if tab === "account"}
  <div class="card">
    <h2>{$t("settings.tab.account")}</h2>
    <p class="muted">{$t("settings.account.loggedAs")} <strong>{user}</strong></p>
    <button class="danger" on:click={logout}>{$t("settings.account.logout")}</button>
  </div>
{/if}

<style>
  .tabs { display: flex; gap: 0.4rem; flex-wrap: wrap; margin-bottom: 1rem; }
  .tabs button { background: transparent; border: 1px solid var(--border); color: var(--muted); border-radius: 8px; padding: 0.4rem 0.9rem; cursor: pointer; font: inherit; }
  .tabs button.active { background: color-mix(in srgb, var(--accent) 18%, transparent); color: var(--text); border-color: var(--accent); }
  .themes { display: flex; gap: 0.6rem; flex-wrap: wrap; margin-top: 0.3rem; }
  .swatch { display: flex; align-items: center; gap: 0.4rem; background: var(--s-bg); border: 2px solid var(--border); color: var(--text); border-radius: 10px; padding: 0.5rem 0.8rem; cursor: pointer; font: inherit; font-size: 0.85rem; }
  .swatch.active { border-color: var(--s-accent); }
  .dot1, .dot2 { width: 12px; height: 12px; border-radius: 50%; display: inline-block; }
  .dot1 { background: var(--s-accent); }
  .dot2 { background: var(--s-panel); border: 1px solid var(--border); }
  table { width: 100%; border-collapse: collapse; font-size: 0.85rem; }
  th { text-align: left; color: var(--muted); font-weight: 500; padding: 0.4rem 0.5rem; border-bottom: 1px solid var(--border); }
  td { padding: 0.5rem; border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent); }
  .form { display: grid; grid-template-columns: 1fr 1fr; gap: 0.5rem 1rem; }
  label { display: block; font-size: 0.82rem; color: var(--muted); }
  input { width: 100%; background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 7px; padding: 0.45rem 0.6rem; font: inherit; margin-top: 0.2rem; }
  textarea { width: 100%; background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.6rem; font-family: ui-monospace, monospace; font-size: 0.8rem; }
  .check { display: flex; align-items: center; gap: 0.4rem; margin-top: 1.4rem; }
  .check input { width: auto; }
  .row { display: flex; gap: 0.8rem; align-items: center; margin-top: 0.8rem; }
  button.primary { background: var(--accent); border: 1px solid var(--accent); color: #04231a; font-weight: 600; border-radius: 8px; padding: 0.5rem 0.9rem; cursor: pointer; font: inherit; }
  button.danger { background: var(--err); border: 1px solid var(--err); color: #2a0707; border-radius: 8px; padding: 0.5rem 0.9rem; cursor: pointer; font: inherit; }
  .small { font-size: 0.78rem; }
</style>
