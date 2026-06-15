<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Agent } from "./api";
  import { toast } from "./toast";

  type PubAgent = Agent & { price_per_1k_output_tokens?: number; revenue_share?: number; published_at?: string };

  let pub = $state<PubAgent[]>([]);
  let mine = $state<Agent[]>([]);
  let keys = $state<{ id: string; name: string; key_prefix: string; revoked: number; last_used_at: string | null }[]>([]);
  let earn = $state<{ invokes: number; output_tokens: number; billed_usd: number; publisher_usd: number }>({ invokes: 0, output_tokens: 0, billed_usd: 0, publisher_usd: 0 });
  let usageRows = $state<{ id: string; agent_id: string; agent_name: string; prompt_tokens: number; completion_tokens: number; billed_usd: number; publisher_usd: number; created_at: string }[]>([]);

  let newKeyName = $state("");
  let freshKey = $state(""); // shown once after creation
  let selected = $state<PubAgent | null>(null);
  let testKey = $state("");
  let testInput = $state("");
  let testOut = $state<{ output: string; cost_usd: number; usage: { completion_tokens: number } } | null>(null);
  let busy = $state(false);

  // Publish form state per agent id.
  let priceById = $state<Record<string, number>>({});
  let shareById = $state<Record<string, number>>({});

  async function load() {
    try {
      [pub, mine, keys, earn, usageRows] = await Promise.all([
        api.marketplace() as Promise<PubAgent[]>,
        api.listAgents(),
        api.listKeys(),
        api.earnings(),
        api.marketplaceUsage(),
      ]);
    } catch (e) { toast(e instanceof Error ? e.message : String(e), "error"); }
  }
  onMount(load);

  async function createKey() {
    try {
      const r = await api.createKey(newKeyName.trim() || "default");
      freshKey = r.key;
      testKey = r.key;
      newKeyName = "";
      keys = await api.listKeys();
    } catch (e) { toast(e instanceof Error ? e.message : String(e), "error"); }
  }
  async function revoke(id: string) { await api.revokeKey(id); keys = await api.listKeys(); }

  async function publish(a: Agent, on: boolean) {
    try {
      await api.publishAgent(a.id, on ? "public" : "private", {
        pricePerKTokens: priceById[a.id] ?? 0.02,
        revenueShare: (shareById[a.id] ?? 70) / 100,
      });
      toast(on ? "Agent publié" : "Agent retiré", "success");
      await load();
    } catch (e) { toast(e instanceof Error ? e.message : String(e), "error"); }
  }

  const origin = typeof location !== "undefined" ? location.origin : "https://takoia.szymkowiak.fr";
  const curl = (a: PubAgent) =>
    `curl -X POST ${origin}/api/v1/agents/${a.id}/invoke \\\n  -H "Authorization: Bearer ${testKey || "sk_takoia_…"}" \\\n  -H "Content-Type: application/json" \\\n  -d '{"input": "votre demande"}'`;

  async function runTest() {
    if (!selected || !testKey || !testInput.trim()) return;
    busy = true; testOut = null;
    try {
      testOut = await api.invokeAgent(selected.id, testKey, testInput.trim());
      await load();
    } catch (e) { toast(e instanceof Error ? e.message : String(e), "error"); }
    finally { busy = false; }
  }

  const isPublic = (a: Agent) => (a as any).visibility === "public";
</script>

<div class="card">
  <h2>🛒 Marketplace <span class="muted small">— publie tes agents experts et revends-les en API (facturation au token sortant)</span></h2>
  <div class="stats">
    <div class="stat"><span class="n">{earn.invokes}</span><span class="l">appels API</span></div>
    <div class="stat"><span class="n">{earn.output_tokens.toLocaleString()}</span><span class="l">tokens sortants</span></div>
    <div class="stat"><span class="n">${earn.billed_usd.toFixed(4)}</span><span class="l">facturé conso</span></div>
    <div class="stat"><span class="n">${earn.publisher_usd.toFixed(4)}</span><span class="l">revenu publisher</span></div>
  </div>
</div>

<div class="card">
  <h2>Coût par requête <span class="muted small">— tokens & prix de chaque appel facturé</span></h2>
  {#if usageRows.length === 0}
    <p class="muted small">Aucun appel facturé pour l'instant.</p>
  {:else}
    <table class="usage">
      <thead>
        <tr><th>Date</th><th>Agent</th><th class="r">Tokens entrée</th><th class="r">Tokens sortie</th><th class="r">Facturé</th><th class="r">Revenu publisher</th></tr>
      </thead>
      <tbody>
        {#each usageRows as u}
          <tr>
            <td class="muted small">{new Date(u.created_at).toLocaleString()}</td>
            <td>{u.agent_name}</td>
            <td class="r">{u.prompt_tokens.toLocaleString()}</td>
            <td class="r">{u.completion_tokens.toLocaleString()}</td>
            <td class="r">${u.billed_usd.toFixed(4)}</td>
            <td class="r">${u.publisher_usd.toFixed(4)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<div class="card">
  <h2>🔑 Clés API</h2>
  <p class="muted small">Une clé permet d'appeler n'importe quel agent publié via l'API. La mémoire reste hébergée (jamais distribuée).</p>
  {#if freshKey}
    <div class="freshkey">Nouvelle clé (copie-la, affichée une seule fois) : <code>{freshKey}</code></div>
  {/if}
  <div class="row">
    <input placeholder="nom de la clé" bind:value={newKeyName} />
    <button class="primary" onclick={createKey}>Créer une clé</button>
  </div>
  {#if keys.length}
    <table>
      <thead><tr><th>Nom</th><th>Préfixe</th><th>Dernière utilisation</th><th></th></tr></thead>
      <tbody>
        {#each keys as k}
          <tr class:revoked={k.revoked}>
            <td>{k.name || "—"}</td><td class="mono">{k.key_prefix}…</td>
            <td class="small">{k.last_used_at ? k.last_used_at.slice(0, 19).replace("T", " ") : "jamais"}</td>
            <td>{#if !k.revoked}<button class="danger" onclick={() => revoke(k.id)}>révoquer</button>{:else}<span class="muted small">révoquée</span>{/if}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<div class="card">
  <h2>📢 Publier mes agents</h2>
  <div class="pubgrid">
    {#each mine as a}
      <div class="pubrow">
        <span class="nm">{(a as any).icon || "🐙"} {a.name}</span>
        <label class="f">$ / 1k tokens<input type="number" step="0.001" min="0" value={priceById[a.id] ?? 0.02} oninput={(e) => priceById[a.id] = parseFloat((e.target as HTMLInputElement).value)} /></label>
        <label class="f">part % publisher<input type="number" min="0" max="100" value={shareById[a.id] ?? 70} oninput={(e) => shareById[a.id] = parseFloat((e.target as HTMLInputElement).value)} /></label>
        {#if isPublic(a)}
          <span class="badge ok">publié</span>
          <button onclick={() => publish(a, false)}>retirer</button>
        {:else}
          <button class="primary" onclick={() => publish(a, true)}>publier</button>
        {/if}
      </div>
    {/each}
    {#if mine.length === 0}<p class="muted small">Aucun agent. Crée-en un dans le dashboard.</p>{/if}
  </div>
</div>

<div class="card">
  <h2>🌍 Agents publiés ({pub.length})</h2>
  <div class="grid">
    {#each pub as a}
      <button class="agentcard" class:sel={selected?.id === a.id} onclick={() => (selected = a)}>
        <div class="ac-top"><span class="ac-icon">{(a as any).icon || "🐙"}</span><strong>{a.name}</strong></div>
        <p class="muted small">{a.description || (a as any).expertise_domain || ""}</p>
        <div class="ac-meta">
          <span>${(a.price_per_1k_output_tokens ?? 0).toFixed(3)} / 1k tok</span>
          <span>{(a as any).runs_count ?? 0} runs</span>
        </div>
      </button>
    {/each}
    {#if pub.length === 0}<p class="muted small">Aucun agent publié pour l'instant.</p>{/if}
  </div>

  {#if selected}
    <div class="usebox">
      <h3>Utiliser « {selected.name} » via API</h3>
      <pre class="curl">{curl(selected)}</pre>
      <div class="trybox">
        <input placeholder="clé API (sk_takoia_…)" bind:value={testKey} />
        <input placeholder="ta demande à l'agent" bind:value={testInput} />
        <button class="primary" onclick={runTest} disabled={busy || !testKey || !testInput.trim()}>{busy ? "…" : "▶ Tester l'appel"}</button>
      </div>
      {#if testOut}
        <div class="result">
          <div class="muted small">{testOut.usage.completion_tokens} tokens sortants · facturé ${testOut.cost_usd.toFixed(5)}</div>
          <pre>{testOut.output}</pre>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .stats { display: flex; gap: 1.5rem; flex-wrap: wrap; margin-top: 0.5rem; }
  .stat { display: flex; flex-direction: column; }
  .stat .n { font-size: 1.6rem; font-weight: 600; }
  .stat .l { color: var(--muted); font-size: 0.78rem; }
  .row { display: flex; gap: 0.6rem; margin: 0.6rem 0; flex-wrap: wrap; }
  .row input { flex: 1; min-width: 160px; }
  input { background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.45rem 0.6rem; font: inherit; }
  .freshkey { background: color-mix(in srgb, var(--ok) 14%, var(--panel)); border: 1px solid var(--ok); border-radius: 8px; padding: 0.6rem 0.8rem; margin-bottom: 0.6rem; font-size: 0.85rem; word-break: break-all; }
  .freshkey code, .mono { font-family: ui-monospace, monospace; }
  table { width: 100%; border-collapse: collapse; font-size: 0.84rem; margin-top: 0.6rem; }
  th, td { text-align: left; padding: 0.4rem 0.5rem; border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent); }
  th.r, td.r { text-align: right; font-variant-numeric: tabular-nums; }
  tr.revoked { opacity: 0.5; }
  .small { font-size: 0.78rem; color: var(--muted); }
  .pubgrid { display: flex; flex-direction: column; gap: 0.4rem; }
  .pubrow { display: flex; align-items: center; gap: 0.8rem; flex-wrap: wrap; padding: 0.4rem 0; border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent); }
  .pubrow .nm { flex: 1; min-width: 140px; font-weight: 600; }
  .pubrow .f { display: flex; flex-direction: column; font-size: 0.7rem; color: var(--muted); }
  .pubrow .f input { width: 90px; }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 0.7rem; margin-top: 0.6rem; }
  .agentcard { text-align: left; background: var(--bg); border: 1px solid var(--border); border-radius: 12px; padding: 0.8rem; cursor: pointer; color: var(--text); font: inherit; }
  .agentcard:hover, .agentcard.sel { border-color: var(--accent); }
  .ac-top { display: flex; align-items: center; gap: 0.5rem; }
  .ac-icon { font-size: 1.3rem; }
  .ac-meta { display: flex; justify-content: space-between; font-size: 0.74rem; color: var(--muted); margin-top: 0.5rem; }
  .usebox { margin-top: 1rem; border-top: 1px solid var(--border); padding-top: 0.8rem; }
  .curl { background: var(--bg); border: 1px solid var(--border); border-radius: 8px; padding: 0.7rem; font-family: ui-monospace, monospace; font-size: 0.76rem; white-space: pre-wrap; word-break: break-all; }
  .trybox { display: flex; gap: 0.6rem; margin-top: 0.6rem; flex-wrap: wrap; }
  .trybox input { flex: 1; min-width: 160px; }
  .result { margin-top: 0.6rem; }
  .result pre { background: var(--bg); border: 1px solid var(--border); border-radius: 8px; padding: 0.7rem; white-space: pre-wrap; word-break: break-word; font-size: 0.8rem; max-height: 300px; overflow-y: auto; }
  .badge { font-size: 0.7rem; padding: 0.1rem 0.5rem; border-radius: 20px; background: var(--border); }
  .badge.ok { background: var(--ok); color: #04231a; }
  button { border-radius: 8px; padding: 0.4rem 0.7rem; cursor: pointer; font: inherit; border: 1px solid var(--border); background: var(--bg); color: var(--text); }
  button.primary { background: var(--accent); border-color: var(--accent); color: #04231a; font-weight: 600; }
  button.danger { color: var(--err); border-color: color-mix(in srgb, var(--err) 50%, var(--border)); }
</style>
