<script lang="ts">
  import { api, type Connector } from "./api";

  export let connectors: Connector[] = [];
  export let onChanged: () => void = () => {};

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
      msg = "saved";
      name = base_url = model = secret = "";
      is_default = false;
      onChanged();
    } catch (e) {
      msg = e instanceof Error ? e.message : String(e);
    }
  }
</script>

<div class="card">
  <h2>LLM providers</h2>
  <p class="muted small">Secrets are encrypted at rest (chacha20poly1305) and never returned in clear.
    Use <code>claude-cli</code> as base URL to consume the Claude plan via <code>claude -p</code>.</p>
  <table>
    <thead><tr><th>Name</th><th>Base URL</th><th>Model</th><th>Secret</th><th>Default</th></tr></thead>
    <tbody>
      {#each connectors as c}
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
  <h2>Add / update a provider</h2>
  <div class="form">
    <label>Name <input bind:value={name} placeholder="claude_max | ollama | gemini | codex" /></label>
    <label>Base URL <input bind:value={base_url} placeholder="claude-cli or https://…/v1" /></label>
    <label>Model <input bind:value={model} placeholder="(optional)" /></label>
    <label>Secret / token <input type="password" bind:value={secret} placeholder="kept if empty" /></label>
    <label class="check"><input type="checkbox" bind:checked={is_default} /> Default provider</label>
  </div>
  <div class="row">
    <button class="primary" on:click={save} disabled={!name}>Save</button>
    {#if msg}<span class="muted small">{msg}</span>{/if}
  </div>
</div>

<style>
  table { width: 100%; border-collapse: collapse; font-size: 0.85rem; }
  th { text-align: left; color: var(--muted); font-weight: 500; padding: 0.4rem 0.5rem; border-bottom: 1px solid var(--border); }
  td { padding: 0.5rem; border-bottom: 1px solid #161c2a; }
  .form { display: grid; grid-template-columns: 1fr 1fr; gap: 0.5rem 1rem; }
  label { display: block; font-size: 0.82rem; color: var(--muted); }
  input { width: 100%; background: #0b0e14; border: 1px solid var(--border); color: var(--text); border-radius: 7px; padding: 0.45rem 0.6rem; font: inherit; margin-top: 0.2rem; }
  .check { display: flex; align-items: center; gap: 0.4rem; margin-top: 1.4rem; }
  .check input { width: auto; }
  .row { display: flex; gap: 0.8rem; align-items: center; margin-top: 0.8rem; }
  button.primary { background: var(--accent); border: 1px solid var(--accent); color: #fff; border-radius: 8px; padding: 0.5rem 0.9rem; cursor: pointer; font: inherit; }
  .small { font-size: 0.78rem; }
</style>
