<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Connector, type User, type AgentPermission } from "./api";
  import { t } from "./i18n";
  import { toast } from "./toast";
  import { THEMES, themeId, setTheme } from "./theme";
  import IntegrationsView from "./IntegrationsView.svelte";
  import { loadDiscordHooks, saveDiscordHooks } from "./discordHooks";

  export let connectors: Connector[] = [];
  export let onChanged: () => void = () => {};

  type Tab = "appearance" | "agents" | "providers" | "integrations" | "expert" | "users" | "account";
  let tab: Tab = "appearance";

  // Users & Roles (admin only)
  let me: User | null = null;
  let users: User[] = [];
  // create-user form
  let nuEmail = "";
  let nuName = "";
  let nuPassword = "";
  let nuAdmin = false;
  // agent permissions
  let permAgents: { id: string; name: string }[] = [];
  let permAgentId = "";
  let perms: AgentPermission[] = [];
  let grantUserId = "";
  let grantRole: "owner" | "editor" | "viewer" = "viewer";

  onMount(async () => {
    try {
      me = await api.me();
    } catch {
      me = null;
    }
    if (me?.is_admin) {
      await loadUsers();
      try {
        permAgents = (await api.listAgents()).map((a) => ({ id: a.id, name: a.name }));
      } catch (e) {
        toast(e instanceof Error ? e.message : String(e), "error");
      }
    }
  });

  async function loadUsers() {
    try {
      users = await api.listUsers();
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    }
  }

  async function createUser() {
    if (!nuEmail.trim() || !nuPassword) return;
    try {
      await api.createUser({ email: nuEmail.trim(), name: nuName.trim() || undefined, password: nuPassword, is_admin: nuAdmin });
      toast($t("users.created"), "success");
      nuEmail = nuName = nuPassword = "";
      nuAdmin = false;
      await loadUsers();
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    }
  }

  async function toggleAdmin(u: User) {
    try {
      await api.updateUser(u.id, { is_admin: !u.is_admin });
      toast($t("users.updated"), "success");
      await loadUsers();
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    }
  }

  async function editName(u: User) {
    const name = prompt($t("users.name"), u.name);
    if (name === null) return;
    try {
      await api.updateUser(u.id, { name: name.trim() });
      toast($t("users.updated"), "success");
      await loadUsers();
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    }
  }

  async function resetPassword(u: User) {
    const password = prompt($t("users.newPassword"));
    if (!password) return;
    try {
      await api.updateUser(u.id, { password });
      toast($t("users.updated"), "success");
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    }
  }

  async function deleteUser(u: User) {
    if (!confirm($t("users.confirmDelete"))) return;
    try {
      await api.deleteUser(u.id);
      toast($t("users.deleted"), "success");
      await loadUsers();
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    }
  }

  async function loadPerms() {
    if (!permAgentId) {
      perms = [];
      return;
    }
    try {
      perms = await api.agentPermissions(permAgentId);
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    }
  }

  async function grantPerm() {
    if (!permAgentId || !grantUserId) return;
    try {
      await api.setAgentPermission(permAgentId, grantUserId, grantRole);
      toast($t("users.granted"), "success");
      grantUserId = "";
      await loadPerms();
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    }
  }

  async function setRole(p: AgentPermission, role: string) {
    try {
      await api.setAgentPermission(permAgentId, p.user_id, role);
      toast($t("users.updated"), "success");
      await loadPerms();
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    }
  }

  async function removePerm(p: AgentPermission) {
    try {
      await api.removeAgentPermission(permAgentId, p.user_id);
      toast($t("users.removed"), "success");
      await loadPerms();
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    }
  }

  // Agents: default loop interval (minutes) for newly built agents.
  let defaultLoopMin = parseInt(localStorage.getItem("takoia.defaultLoopMin") ?? "300", 10) || 300;
  function saveAgentDefaults() {
    localStorage.setItem("takoia.defaultLoopMin", String(defaultLoopMin > 0 ? defaultLoopMin : 300));
  }

  // Global named Discord webhooks (reusable by name in the builder).
  let discordHooks = loadDiscordHooks();
  let hookName = "";
  let hookUrl = "";
  function addHook() {
    if (!hookName.trim() || !hookUrl.trim()) return;
    discordHooks = [...discordHooks.filter((h) => h.name !== hookName.trim()), { name: hookName.trim(), url: hookUrl.trim() }];
    saveDiscordHooks(discordHooks);
    hookName = ""; hookUrl = "";
  }
  function removeHook(i: number) {
    discordHooks = discordHooks.filter((_, idx) => idx !== i);
    saveDiscordHooks(discordHooks);
  }

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
  <button class:active={tab === "agents"} on:click={() => (tab = "agents")}>{$t("settings.tab.agents")}</button>
  <button class:active={tab === "providers"} on:click={() => (tab = "providers")}>{$t("settings.tab.providers")}</button>
  <button class:active={tab === "integrations"} on:click={() => (tab = "integrations")}>{$t("settings.tab.integrations")}</button>
  <button class:active={tab === "expert"} on:click={() => (tab = "expert")}>{$t("settings.tab.expert")}</button>
  {#if me?.is_admin}
    <button class:active={tab === "users"} on:click={() => (tab = "users")}>{$t("settings.tab.users")}</button>
  {/if}
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

{:else if tab === "agents"}
  <div class="card">
    <h2>{$t("settings.agents.title")}</h2>
    <p class="muted small">{$t("settings.agents.hint")}</p>
    <div class="form">
      <label>{$t("settings.agents.loopDefault")}
        <input type="number" min="1" bind:value={defaultLoopMin} on:change={saveAgentDefaults} on:blur={saveAgentDefaults} />
      </label>
    </div>
  </div>

  <div class="card">
    <h2>{$t("settings.discord.title")}</h2>
    <p class="muted small">{$t("settings.discord.hint")}</p>
    {#if discordHooks.length}
      <table>
        <thead><tr><th>{$t("settings.discord.name")}</th><th>URL</th><th></th></tr></thead>
        <tbody>
          {#each discordHooks as h, i}
            <tr>
              <td><strong>{h.name}</strong></td>
              <td class="small">…{h.url.slice(-18)}</td>
              <td><button class="danger" on:click={() => removeHook(i)}>🗑</button></td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
    <div class="form">
      <label>{$t("settings.discord.name")} <input bind:value={hookName} placeholder="ex. Équipe Trading" /></label>
      <label>{$t("settings.discord.url")} <input bind:value={hookUrl} placeholder="https://discord.com/api/webhooks/…" /></label>
      <button class="primary" on:click={addHook} disabled={!hookName.trim() || !hookUrl.trim()}>{$t("settings.discord.add")}</button>
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

{:else if tab === "users"}
  {#if !me?.is_admin}
    <div class="card">
      <p class="muted">{$t("users.adminOnly")}</p>
    </div>
  {:else}
    <div class="card">
      <h2>{$t("users.title")}</h2>
      <table>
        <thead><tr>
          <th>{$t("users.email")}</th><th>{$t("users.name")}</th>
          <th>{$t("users.admin")}</th><th></th>
        </tr></thead>
        <tbody>
          {#each users as u (u.id)}
            <tr>
              <td><strong>{u.email}</strong></td>
              <td class="small">{u.name || "—"}</td>
              <td>{#if u.is_admin}<span class="badge">{$t("users.admin")}</span>{/if}</td>
              <td class="actions">
                <button on:click={() => editName(u)}>{$t("users.editName")}</button>
                <button on:click={() => resetPassword(u)}>{$t("users.resetPassword")}</button>
                <button on:click={() => toggleAdmin(u)}>{u.is_admin ? $t("users.revokeAdmin") : $t("users.makeAdmin")}</button>
                <button class="danger" on:click={() => deleteUser(u)} disabled={u.id === me.id}>{$t("users.delete")}</button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    <div class="card">
      <h2>{$t("users.create")}</h2>
      <div class="form">
        <label>{$t("users.email")} <input type="email" bind:value={nuEmail} placeholder="user@example.com" /></label>
        <label>{$t("users.name")} <input bind:value={nuName} /></label>
        <label>{$t("users.password")} <input type="password" bind:value={nuPassword} /></label>
        <label class="check"><input type="checkbox" bind:checked={nuAdmin} /> {$t("users.admin")}</label>
      </div>
      <div class="row">
        <button class="primary" on:click={createUser} disabled={!nuEmail.trim() || !nuPassword}>{$t("users.create")}</button>
      </div>
    </div>

    <div class="card">
      <h2>{$t("users.permissions")}</h2>
      <div class="form">
        <label>{$t("users.pickAgent")}
          <select bind:value={permAgentId} on:change={loadPerms}>
            <option value="">—</option>
            {#each permAgents as a (a.id)}
              <option value={a.id}>{a.name}</option>
            {/each}
          </select>
        </label>
      </div>
      {#if permAgentId}
        <table>
          <thead><tr>
            <th>{$t("users.email")}</th><th>{$t("users.role")}</th><th></th>
          </tr></thead>
          <tbody>
            {#each perms as p (p.user_id)}
              <tr>
                <td><strong>{p.email}</strong> <span class="small muted">{p.name}</span></td>
                <td>
                  <select value={p.role} on:change={(e) => setRole(p, e.currentTarget.value)}>
                    <option value="owner">{$t("users.role.owner")}</option>
                    <option value="editor">{$t("users.role.editor")}</option>
                    <option value="viewer">{$t("users.role.viewer")}</option>
                  </select>
                </td>
                <td><button class="danger" on:click={() => removePerm(p)}>{$t("users.remove")}</button></td>
              </tr>
            {/each}
          </tbody>
        </table>
        <div class="form">
          <label>{$t("users.name")}
            <select bind:value={grantUserId}>
              <option value="">—</option>
              {#each users as u (u.id)}
                <option value={u.id}>{u.email}</option>
              {/each}
            </select>
          </label>
          <label>{$t("users.role")}
            <select bind:value={grantRole}>
              <option value="owner">{$t("users.role.owner")}</option>
              <option value="editor">{$t("users.role.editor")}</option>
              <option value="viewer">{$t("users.role.viewer")}</option>
            </select>
          </label>
        </div>
        <div class="row">
          <button class="primary" on:click={grantPerm} disabled={!grantUserId}>{$t("users.grant")}</button>
        </div>
      {/if}
    </div>
  {/if}

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
  .muted { color: var(--muted); }
  select { width: 100%; background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 7px; padding: 0.45rem 0.6rem; font: inherit; margin-top: 0.2rem; }
  .badge { display: inline-block; background: color-mix(in srgb, var(--accent) 20%, transparent); color: var(--text); border: 1px solid var(--accent); border-radius: 6px; padding: 0.1rem 0.45rem; font-size: 0.72rem; }
  .actions { display: flex; flex-wrap: wrap; gap: 0.35rem; }
  .actions button { background: transparent; border: 1px solid var(--border); color: var(--text); border-radius: 7px; padding: 0.3rem 0.55rem; cursor: pointer; font: inherit; font-size: 0.76rem; }
  .actions button.danger { background: var(--err); border-color: var(--err); color: #2a0707; }
  .actions button:disabled { opacity: 0.4; cursor: not-allowed; }
</style>
