<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Connector } from "./api";
  import { t } from "./i18n";
  import Logo from "./Logo.svelte";

  // A field the user must fill for a given integration. `secret` fields are
  // merged into the encrypted JSON blob stored on the connector.
  interface Field {
    key: string;
    label: string;
    placeholder?: string;
    secret?: boolean;
  }

  interface Integration {
    id: string; // connector name (kind='integration')
    name: string;
    logo: string; // simple-icons slug
    description: string;
    baseUrlKey?: string; // which field maps to connector.base_url
    fields: Field[];
    functional?: boolean; // Email is the one fully-working send path
  }

  // Static catalog. Logos use simple-icons slugs.
  const catalog: Integration[] = [
    {
      id: "email",
      name: "Email (SMTP)",
      logo: "gmail",
      description:
        "Send transactional emails through any SMTP server. Fully functional send + test path.",
      baseUrlKey: "host",
      functional: true,
      fields: [
        { key: "host", label: "SMTP host", placeholder: "smtp.gmail.com" },
        { key: "port", label: "Port", placeholder: "587" },
        { key: "user", label: "Username", placeholder: "you@example.com" },
        { key: "password", label: "Password / app password", secret: true },
        { key: "from", label: "From address (optional)", placeholder: "you@example.com" },
      ],
    },
    {
      id: "twilio",
      name: "SMS (Twilio)",
      logo: "twilio",
      description:
        "Send SMS via Twilio. Requires a verified Twilio account; send is stubbed until credentials are validated.",
      baseUrlKey: "account_sid",
      fields: [
        { key: "account_sid", label: "Account SID", placeholder: "AC…" },
        { key: "auth_token", label: "Auth token", secret: true },
        { key: "from_number", label: "From number", placeholder: "+15551234567" },
      ],
    },
    {
      id: "whatsapp",
      name: "WhatsApp",
      logo: "whatsapp",
      description:
        "WhatsApp Business Cloud API messaging. Requires a verified Meta business account.",
      baseUrlKey: "phone_number_id",
      fields: [
        { key: "phone_number_id", label: "Phone number ID" },
        { key: "access_token", label: "Access token", secret: true },
      ],
    },
    {
      id: "googlecloud",
      name: "Google Cloud",
      logo: "googlecloud",
      description:
        "Google Cloud Platform service account for GCP APIs (storage, vision, etc.).",
      baseUrlKey: "project_id",
      fields: [
        { key: "project_id", label: "Project ID" },
        { key: "service_account_json", label: "Service account JSON key", secret: true },
      ],
    },
    {
      id: "microsoftazure",
      name: "Azure",
      logo: "microsoftazure",
      description: "Microsoft Azure credentials for Azure services and Cognitive APIs.",
      baseUrlKey: "endpoint",
      fields: [
        { key: "endpoint", label: "Endpoint", placeholder: "https://….azure.com" },
        { key: "api_key", label: "API key", secret: true },
      ],
    },
  ];

  let saved: Connector[] = [];
  let selected: Integration | null = null;
  let values: Record<string, string> = {};
  let msg = "";
  let busy = false;

  // Test-email form (only shown for the Email integration).
  let testTo = "";

  function isSaved(id: string): boolean {
    return saved.some((c) => c.kind === "integration" && c.name === id);
  }

  async function load() {
    try {
      saved = await api.listConnectors();
    } catch {
      saved = [];
    }
  }

  function open(i: Integration) {
    selected = i;
    values = {};
    msg = "";
  }

  function close() {
    selected = null;
    values = {};
    msg = "";
  }

  async function save() {
    if (!selected) return;
    busy = true;
    msg = "";
    try {
      const base_url = selected.baseUrlKey ? values[selected.baseUrlKey] ?? "" : "";
      // Bundle every field into the encrypted secret JSON blob.
      const secret = JSON.stringify(values);
      await api.upsertConnector({
        kind: "integration",
        name: selected.id,
        base_url,
        secret,
      });
      msg = $t("integrations.saved");
      await load();
    } catch (e) {
      msg = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  async function testEmail() {
    busy = true;
    msg = "";
    try {
      const r = await api.testEmailIntegration(testTo);
      msg = r.message;
    } catch (e) {
      msg = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  onMount(load);
</script>

<div class="card">
  <div class="head">
    <h2>
      {$t("integrations.title")}
      <span class="muted small">— {catalog.length} {$t("integrations.available")}</span>
    </h2>
  </div>
  <p class="muted small">{$t("integrations.hint")}</p>

  <div class="grid">
    {#each catalog as i}
      <div class="item">
        <div class="top">
          <Logo slug={i.logo} label={i.name} />
          <div class="meta">
            <strong>{i.name}</strong>
            <span class="cat">
              {#if i.functional}<span class="tag ok">{$t("integrations.functional")}</span>
              {:else}<span class="tag">{$t("integrations.configurable")}</span>{/if}
            </span>
          </div>
          {#if isSaved(i.id)}<span class="badge ok">{$t("integrations.configured")}</span>{/if}
        </div>
        <p class="desc">{i.description}</p>
        <div class="actions">
          <button on:click={() => open(i)}>
            {isSaved(i.id) ? $t("integrations.update") : $t("integrations.configure")}
          </button>
        </div>
      </div>
    {/each}
  </div>
</div>

{#if selected}
  <div class="card">
    <div class="head">
      <h2><Logo slug={selected.logo} label={selected.name} size={20} /> {selected.name}</h2>
      <button class="ghost" on:click={close}>{$t("integrations.cancel")}</button>
    </div>
    <p class="muted small">{$t("integrations.credentialsHint")}</p>
    <div class="form">
      {#each selected.fields as f}
        <label>
          {f.label}
          {#if f.secret}
            <input type="password" bind:value={values[f.key]} placeholder={f.placeholder ?? ""} />
          {:else}
            <input bind:value={values[f.key]} placeholder={f.placeholder ?? ""} />
          {/if}
        </label>
      {/each}
    </div>
    <div class="row">
      <button class="primary" on:click={save} disabled={busy}>{$t("integrations.save")}</button>
      {#if msg}<span class="muted small">{msg}</span>{/if}
    </div>

    {#if selected.functional && isSaved(selected.id)}
      <hr />
      <h3>{$t("integrations.sendTest")}</h3>
      <div class="form">
        <label>
          {$t("integrations.recipient")}
          <input bind:value={testTo} placeholder="someone@example.com" />
        </label>
      </div>
      <div class="row">
        <button on:click={testEmail} disabled={busy || !testTo}>
          {$t("integrations.sendTestBtn")}
        </button>
      </div>
    {/if}
  </div>
{/if}

<style>
  .head { display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 0.6rem; }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(240px, 1fr)); gap: 0.8rem; margin-top: 1rem; }
  .item { border: 1px solid var(--border); border-radius: 10px; padding: 0.8rem; background: #0f1422; display: flex; flex-direction: column; }
  .top { display: flex; align-items: center; gap: 0.6rem; }
  .meta { display: flex; flex-direction: column; line-height: 1.3; flex: 1; }
  .cat { color: var(--muted); font-size: 0.72rem; }
  .tag { font-size: 0.66rem; padding: 0.05rem 0.4rem; border: 1px solid var(--border); border-radius: 20px; color: var(--muted); }
  .tag.ok { border-color: var(--ok); color: var(--ok); }
  .desc { color: var(--muted); font-size: 0.8rem; margin: 0.5rem 0; flex: 1; }
  .actions { display: flex; justify-content: flex-end; }
  .badge.ok { background: var(--ok); color: #04231a; font-size: 0.66rem; padding: 0.1rem 0.45rem; border-radius: 20px; }
  .form { display: grid; grid-template-columns: 1fr 1fr; gap: 0.5rem 1rem; margin-top: 0.6rem; }
  label { display: block; font-size: 0.82rem; color: var(--muted); }
  input { width: 100%; background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 7px; padding: 0.45rem 0.6rem; font: inherit; margin-top: 0.2rem; }
  .row { display: flex; gap: 0.8rem; align-items: center; margin-top: 0.8rem; }
  button { border: 1px solid var(--accent); background: var(--accent); color: #fff; border-radius: 7px; padding: 0.4rem 0.8rem; cursor: pointer; font: inherit; font-size: 0.82rem; }
  button.primary { color: #04231a; font-weight: 600; }
  button.ghost { background: transparent; border: 1px solid var(--border); color: var(--muted); }
  button:disabled { background: #1a2133; border-color: var(--border); color: var(--muted); cursor: default; }
  hr { border: none; border-top: 1px solid var(--border); margin: 1rem 0; }
  h3 { margin: 0.4rem 0; font-size: 0.95rem; }
  .small { font-size: 0.78rem; }
</style>
