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

  type Tab = "appearance" | "agents" | "providers" | "integrations" | "expert" | "users" | "secrets" | "sandbox" | "account";
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

  // Import an OpenClaw SOUL.md / Hermes agent definition.
  let soulText = "";
  let soulPublish = false;
  let soulPrice = 0.02;
  let soulBusy = false;
  let soulMsg = "";
  async function importSoul() {
    soulMsg = "";
    soulBusy = true;
    try {
      const r = await api.importSoul(soulText, soulPublish, soulPublish ? soulPrice : undefined);
      soulMsg = `${$t("agents.soulImported")}: ${r.name}${r.published ? " ✓ marketplace" : ""}`;
      onChanged();
    } catch (e) {
      soulMsg = e instanceof Error ? e.message : String(e);
    } finally {
      soulBusy = false;
    }
  }

  // Secret storage backend (admin only). Where connector / AI-tool secrets live.
  type SecretKind = "local" | "vault" | "azure" | "gcp" | "aws";
  let secretKind: SecretKind = "local";
  let secretBackends: string[] = ["local", "vault", "azure", "gcp", "aws"];
  let secretParams: Record<string, string> = {};
  // Tracks which sensitive fields the user edited, so we only send changed ones.
  let secretTouched: Record<string, boolean> = {};
  let secretMsg = "";
  let secretTestMsg = "";
  let secretTestOk = false;
  let secretLoaded = false;

  // Sensitive fields per backend: sent only when the user types a new value.
  const sensitiveFields: Record<string, string[]> = {
    vault: ["token"],
    aws: ["access_key", "secret_key"],
  };

  function isSensitive(field: string): boolean {
    return (sensitiveFields[secretKind] ?? []).includes(field);
  }

  function markTouched(field: string) {
    secretTouched[field] = true;
  }

  async function loadSecretBackend() {
    try {
      const r = await api.getSecretBackend();
      secretKind = (r.kind as SecretKind) ?? "local";
      secretParams = { ...r.params };
      secretBackends = r.backends?.length ? r.backends : secretBackends;
      secretTouched = {};
      secretLoaded = true;
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    }
  }

  // Build the params payload: for sensitive fields, only send the value if the
  // user typed a new one; otherwise send "" so the backend keeps the old value.
  function buildSecretParams(): Record<string, string> {
    const out: Record<string, string> = {};
    for (const [field, value] of Object.entries(secretParams)) {
      if (isSensitive(field) && !secretTouched[field]) {
        out[field] = "";
      } else {
        out[field] = value ?? "";
      }
    }
    return out;
  }

  async function saveSecretBackend() {
    secretMsg = "";
    secretTestMsg = "";
    try {
      await api.setSecretBackend(secretKind, buildSecretParams());
      secretMsg = $t("secrets.saved");
      toast($t("secrets.saved"), "success");
      await loadSecretBackend();
    } catch (e) {
      secretMsg = e instanceof Error ? e.message : String(e);
    }
  }

  async function testSecretBackend() {
    secretMsg = "";
    secretTestMsg = "";
    try {
      const r = await api.testSecretBackend(secretKind, buildSecretParams());
      secretTestOk = r.ok;
      secretTestMsg = r.message;
    } catch (e) {
      secretTestOk = false;
      secretTestMsg = e instanceof Error ? e.message : String(e);
    }
  }

  // Lazily load the secret config the first time the tab is opened.
  $: if (tab === "secrets" && me?.is_admin && !secretLoaded) {
    loadSecretBackend();
  }

  // ── Sandboxing backend (how agent tool execution is isolated) ─────────────
  let sandboxKind = "landlock";
  let sandboxBackends: string[] = ["none", "landlock", "bubblewrap", "nsjail", "docker", "podman", "firecracker", "microsandbox"];
  let sandboxParams: Record<string, unknown> = {};
  let sandboxMsg = "";
  let sandboxTestMsg = "";
  let sandboxTestOk = false;
  let sandboxLoaded = false;

  async function loadSandbox() {
    try {
      const r = await api.getSandbox();
      sandboxKind = r.kind || "landlock";
      sandboxParams = { ...r.params };
      sandboxBackends = r.backends?.length ? r.backends : sandboxBackends;
      sandboxLoaded = true;
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    }
  }

  // Build the params payload sent to the backend for the selected engine.
  // Only the fields relevant to the current backend are included.
  function buildSandboxParams(): Record<string, unknown> {
    const p = sandboxParams;
    switch (sandboxKind) {
      case "bubblewrap":
      case "nsjail":
        return { allow_network: p.allow_network !== false };
      case "docker":
      case "podman":
        return {
          image: (p.image as string) ?? "",
          allow_network: p.allow_network !== false,
          mem_mb: Number(p.mem_mb) || 0,
          cpus: Number(p.cpus) || 0,
        };
      case "firecracker":
      case "microsandbox":
        return { launcher: (p.launcher as string) ?? "" };
      default:
        return {};
    }
  }

  async function saveSandbox() {
    sandboxMsg = "";
    sandboxTestMsg = "";
    try {
      await api.setSandbox(sandboxKind, buildSandboxParams());
      sandboxMsg = $t("sandbox.saved");
      toast($t("sandbox.saved"), "success");
      await loadSandbox();
    } catch (e) {
      sandboxMsg = e instanceof Error ? e.message : String(e);
    }
  }

  async function testSandbox() {
    sandboxMsg = "";
    sandboxTestMsg = "";
    try {
      const r = await api.testSandbox(sandboxKind, buildSandboxParams());
      sandboxTestOk = r.ok;
      sandboxTestMsg = r.message;
    } catch (e) {
      sandboxTestOk = false;
      sandboxTestMsg = e instanceof Error ? e.message : String(e);
    }
  }

  // Lazily load the sandbox config the first time the tab is opened.
  $: if (tab === "sandbox" && me?.is_admin && !sandboxLoaded) {
    loadSandbox();
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
    <button class:active={tab === "secrets"} on:click={() => (tab = "secrets")}>{$t("settings.tab.secrets")}</button>
    <button class:active={tab === "sandbox"} on:click={() => (tab = "sandbox")}>{$t("settings.tab.sandbox")}</button>
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

  <div class="card">
    <h2>{$t("agents.soulTitle")}</h2>
    <p class="muted small">{$t("agents.soulHint")}</p>
    <textarea rows="12" bind:value={soulText} placeholder={$t("agents.soulPlaceholder")}></textarea>
    <label class="check">
      <input type="checkbox" bind:checked={soulPublish} />
      {$t("agents.soulPublish")}
    </label>
    {#if soulPublish}
      <label class="price">
        {$t("agents.soulPrice")}
        <input type="number" step="0.01" min="0" bind:value={soulPrice} />
      </label>
    {/if}
    <div class="row">
      <button class="primary" on:click={importSoul} disabled={soulBusy || !soulText.trim()}>
        {soulBusy ? $t("agents.soulImporting") : $t("agents.soulImport")}
      </button>
      {#if soulMsg}<span class="muted small">{soulMsg}</span>{/if}
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

{:else if tab === "secrets"}
  {#if !me?.is_admin}
    <div class="card">
      <p class="muted">{$t("users.adminOnly")}</p>
    </div>
  {:else}
    <div class="card">
      <h2>{$t("secrets.title")}</h2>
      <p class="muted small">{$t("secrets.hint")}</p>
      <div class="form">
        <label>{$t("secrets.backend")}
          <select bind:value={secretKind}>
            {#each secretBackends as b}
              <option value={b}>{$t(`secrets.backend.${b}`)}</option>
            {/each}
          </select>
        </label>
      </div>

      {#if secretKind === "local"}
        <p class="muted small">{$t("secrets.backend.local")}</p>

      {:else if secretKind === "vault"}
        <div class="form">
          <label>{$t("secrets.field.addr")}
            <input bind:value={secretParams.addr} placeholder="https://vault.example.com:8200" />
          </label>
          <label>{$t("secrets.field.token")}
            <input type="password" bind:value={secretParams.token} on:input={() => markTouched("token")} placeholder={$t("secrets.secretKept")} />
          </label>
          <label>{$t("secrets.field.mount")}
            <input bind:value={secretParams.mount} placeholder="secret" />
          </label>
        </div>

      {:else if secretKind === "azure"}
        <div class="form">
          <label>{$t("secrets.field.vaultName")}
            <input bind:value={secretParams.vault_name} />
          </label>
        </div>

      {:else if secretKind === "gcp"}
        <div class="form">
          <label>{$t("secrets.field.project")}
            <input bind:value={secretParams.project} />
          </label>
        </div>

      {:else if secretKind === "aws"}
        <div class="form">
          <label>{$t("secrets.field.region")}
            <input bind:value={secretParams.region} placeholder="eu-west-1" />
          </label>
          <label>{$t("secrets.field.accessKey")}
            <input type="password" bind:value={secretParams.access_key} on:input={() => markTouched("access_key")} placeholder={$t("secrets.secretKept")} />
          </label>
          <label>{$t("secrets.field.secretKey")}
            <input type="password" bind:value={secretParams.secret_key} on:input={() => markTouched("secret_key")} placeholder={$t("secrets.secretKept")} />
          </label>
        </div>
      {/if}

      <div class="row">
        <button class="primary" on:click={saveSecretBackend}>{$t("secrets.save")}</button>
        <button on:click={testSecretBackend}>{$t("secrets.test")}</button>
        {#if secretMsg}<span class="muted small">{secretMsg}</span>{/if}
        {#if secretTestMsg}<span class="small" class:ok={secretTestOk} class:err={!secretTestOk}>{secretTestMsg}</span>{/if}
      </div>
    </div>
  {/if}

{:else if tab === "sandbox"}
  {#if !me?.is_admin}
    <div class="card">
      <p class="muted">{$t("users.adminOnly")}</p>
    </div>
  {:else}
    <div class="card">
      <h2>{$t("sandbox.title")}</h2>
      <p class="muted small">{$t("sandbox.hint")}</p>
      <div class="form">
        <label>{$t("sandbox.engine")}
          <select bind:value={sandboxKind}>
            {#each sandboxBackends as b}
              <option value={b}>{$t(`sandbox.backend.${b}`)}</option>
            {/each}
          </select>
        </label>
      </div>

      {#if sandboxKind === "landlock"}
        <p class="muted small">{$t("sandbox.landlockHint")}</p>

      {:else if sandboxKind === "none"}
        <p class="muted small">{$t("sandbox.noneHint")}</p>

      {:else if sandboxKind === "bubblewrap" || sandboxKind === "nsjail"}
        <label class="check">
          <input type="checkbox" checked={sandboxParams.allow_network !== false} on:change={(e) => (sandboxParams.allow_network = e.currentTarget.checked)} />
          {$t("sandbox.field.allowNetwork")}
        </label>

      {:else if sandboxKind === "docker" || sandboxKind === "podman"}
        <div class="form">
          <label>{$t("sandbox.field.image")}
            <input value={(sandboxParams.image ?? "") as string} on:input={(e) => (sandboxParams.image = e.currentTarget.value)} placeholder="debian:stable-slim" />
          </label>
          <label>{$t("sandbox.field.memMb")}
            <input type="number" min="0" value={(sandboxParams.mem_mb ?? "") as number} on:input={(e) => (sandboxParams.mem_mb = e.currentTarget.value)} />
          </label>
          <label>{$t("sandbox.field.cpus")}
            <input type="number" min="0" step="0.5" value={(sandboxParams.cpus ?? "") as number} on:input={(e) => (sandboxParams.cpus = e.currentTarget.value)} />
          </label>
          <label class="check">
            <input type="checkbox" checked={sandboxParams.allow_network !== false} on:change={(e) => (sandboxParams.allow_network = e.currentTarget.checked)} />
            {$t("sandbox.field.allowNetwork")}
          </label>
        </div>

      {:else if sandboxKind === "firecracker" || sandboxKind === "microsandbox"}
        <div class="form">
          <label>{$t("sandbox.field.launcher")}
            <input value={(sandboxParams.launcher ?? "") as string} on:input={(e) => (sandboxParams.launcher = e.currentTarget.value)} placeholder={$t("sandbox.field.launcherOptional")} />
          </label>
        </div>
        <p class="muted small">{$t("sandbox.vmHint")}</p>
      {/if}

      <p class="muted small">{$t("sandbox.cliHint")}</p>

      <div class="row">
        <button class="primary" on:click={saveSandbox}>{$t("sandbox.save")}</button>
        <button on:click={testSandbox}>{$t("sandbox.test")}</button>
        {#if sandboxMsg}<span class="muted small">{sandboxMsg}</span>{/if}
        {#if sandboxTestMsg}<span class="small" class:ok={sandboxTestOk} class:err={!sandboxTestOk}>{sandboxTestMsg}</span>{/if}
      </div>
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
  .ok { color: var(--ok, #2ecc71); }
  .err { color: var(--err); }
  .row button:not(.primary):not(.danger) { background: transparent; border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.5rem 0.9rem; cursor: pointer; font: inherit; }
  select { width: 100%; background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 7px; padding: 0.45rem 0.6rem; font: inherit; margin-top: 0.2rem; }
  .badge { display: inline-block; background: color-mix(in srgb, var(--accent) 20%, transparent); color: var(--text); border: 1px solid var(--accent); border-radius: 6px; padding: 0.1rem 0.45rem; font-size: 0.72rem; }
  .actions { display: flex; flex-wrap: wrap; gap: 0.35rem; }
  .actions button { background: transparent; border: 1px solid var(--border); color: var(--text); border-radius: 7px; padding: 0.3rem 0.55rem; cursor: pointer; font: inherit; font-size: 0.76rem; }
  .actions button.danger { background: var(--err); border-color: var(--err); color: #2a0707; }
  .actions button:disabled { opacity: 0.4; cursor: not-allowed; }
</style>
