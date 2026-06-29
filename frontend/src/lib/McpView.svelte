<script lang="ts">
  import { onMount } from "svelte";
  import { api, type McpServer } from "./api";
  import { t } from "./i18n";
  import Logo from "./Logo.svelte";

  let servers: McpServer[] = [];
  let installed: { name: string; connected: boolean }[] = [];
  let query = "";
  let category = "All";
  let busy: string | null = null;

  // Guided tutorial + one-click live demo (connects the keyless "Fetch" server).
  let tutorialOpen = false;
  let demoPhase = 0; // 0 idle · 1 running · 2 done
  let demoMsg = "";
  const DEMO_ID = "fetch";

  async function runDemo() {
    const target = servers.find((s) => s.id === DEMO_ID) ?? servers[0];
    if (!target) return;
    demoPhase = 1;
    demoMsg = "";
    try {
      const r = await api.mcpConnect(target.id);
      demoMsg = r.cli_registered
        ? $t("mcp.tuto.demo_ok")
        : `${$t("mcp.tuto.demo_recorded")} — ${r.message}`;
      installed = await api.mcpInstalled();
    } catch (e) {
      demoMsg = e instanceof Error ? e.message : String(e);
    } finally {
      demoPhase = 2;
    }
  }

  $: categories = ["All", ...Array.from(new Set(servers.map((s) => s.category)))];
  $: filtered = servers.filter(
    (s) =>
      (category === "All" || s.category === category) &&
      (query === "" ||
        s.name.toLowerCase().includes(query.toLowerCase()) ||
        s.description.toLowerCase().includes(query.toLowerCase())),
  );

  // Servers the user has connected, from the recorded connectors (kind="mcp").
  // This is the reliable signal: it persists even when the `claude` CLI can't
  // reach the server (so the "connecté" badge shows as soon as you connect).
  let connectorIds = new Set<string>();

  // Reactive so the badge updates the moment `connectorIds`/`installed` change.
  $: isConnected = (s: McpServer): boolean =>
    connectorIds.has(s.id) ||
    installed.some((i) => i.connected && (i.name === s.id || i.name.toLowerCase().includes(s.id)));

  async function loadConnectors() {
    try {
      const cs = await api.listConnectors();
      connectorIds = new Set(cs.filter((c) => c.kind === "mcp").map((c) => c.name));
    } catch {
      connectorIds = new Set();
    }
  }

  async function load() {
    servers = await api.mcpCatalog();
    try {
      installed = await api.mcpInstalled();
    } catch {
      installed = [];
    }
    loadConnectors();
  }

  // Inline connect: one click when no key is needed, otherwise a small form
  // that asks for exactly the credentials this server requires.
  let openId: string | null = null; // card whose key form is open
  let formEnv: Record<string, string> = {}; // values typed in the open form
  let result: Record<string, { ok: boolean; msg: string }> = {}; // per-server feedback

  function needsKey(s: McpServer): boolean {
    return !!(s.env && s.env.length);
  }

  function startConnect(s: McpServer) {
    if (needsKey(s)) {
      openId = openId === s.id ? null : s.id;
      formEnv = {};
    } else {
      doConnect(s, []);
    }
  }

  // Takes `env` explicitly so the markup expression depends on it — otherwise
  // Svelte doesn't re-evaluate the button's disabled state as the user types.
  function canSubmit(s: McpServer, env: Record<string, string>): boolean {
    return (s.env ?? []).every((f) => (env[f.key] ?? "").trim() !== "");
  }

  async function doConnect(s: McpServer, envPairs: string[]) {
    busy = s.id;
    try {
      const r = await api.mcpConnect(s.id, envPairs);
      result = {
        ...result,
        [s.id]: {
          ok: true,
          msg: r.cli_registered ? $t("mcp.connect_ok") : `${$t("mcp.connect_recorded")} — ${r.message}`,
        },
      };
      const next = new Set(connectorIds);
      next.add(s.id);
      connectorIds = next; // mark connected right away
      installed = await api.mcpInstalled();
      openId = null;
    } catch (e) {
      result = { ...result, [s.id]: { ok: false, msg: e instanceof Error ? e.message : String(e) } };
    } finally {
      busy = null;
    }
  }

  function submitForm(s: McpServer) {
    const pairs = (s.env ?? []).map((f) => `${f.key}=${(formEnv[f.key] ?? "").trim()}`);
    doConnect(s, pairs);
  }

  // One click to connect every server that needs no key. Servers requiring a
  // credential are left for the user to fill in (you can't auto-supply a token).
  let bulkBusy = false;
  let bulkMsg = "";
  async function connectAll() {
    bulkBusy = true;
    bulkMsg = "";
    let ok = 0;
    const keyless = servers.filter((s) => !needsKey(s) && !isConnected(s));
    for (const s of keyless) {
      busy = s.id;
      try {
        const r = await api.mcpConnect(s.id, []);
        result = {
          ...result,
          [s.id]: {
            ok: true,
            msg: r.cli_registered ? $t("mcp.connect_ok") : `${$t("mcp.connect_recorded")} — ${r.message}`,
          },
        };
        ok++;
      } catch (e) {
        result = { ...result, [s.id]: { ok: false, msg: e instanceof Error ? e.message : String(e) } };
      }
    }
    busy = null;
    try {
      installed = await api.mcpInstalled();
    } catch { /* keep current */ }
    await loadConnectors(); // refresh the connected badges
    const keyCount = servers.filter((s) => needsKey(s)).length;
    bulkMsg = $t("mcp.bulk_done", { ok, key: keyCount });
    bulkBusy = false;
  }

  onMount(load);
</script>

<div class="card tuto">
  <button class="tuto-head" on:click={() => (tutorialOpen = !tutorialOpen)}>
    <span class="tuto-chev">{tutorialOpen ? "▾" : "▸"}</span>
    <h2>🔌 {$t("mcp.tuto.title")}</h2>
    <span class="muted small tuto-sub">{$t("mcp.tuto.subtitle")}</span>
  </button>

  {#if tutorialOpen}
    <ol class="steps">
      <li><span class="num">1</span><div><strong>{$t("mcp.tuto.s1_t")}</strong><p>{$t("mcp.tuto.s1_d")}</p></div></li>
      <li><span class="num">2</span><div><strong>{$t("mcp.tuto.s2_t")}</strong><p>{$t("mcp.tuto.s2_d")}</p></div></li>
      <li><span class="num">3</span><div><strong>{$t("mcp.tuto.s3_t")}</strong><p>{$t("mcp.tuto.s3_d")}</p></div></li>
      <li><span class="num">4</span><div><strong>{$t("mcp.tuto.s4_t")}</strong><p>{$t("mcp.tuto.s4_d")}</p></div></li>
    </ol>

    <div class="demo">
      <div class="demo-info">
        <strong>▶ {$t("mcp.tuto.demo_title")}</strong>
        <p class="muted small">{$t("mcp.tuto.demo_desc")}</p>
      </div>
      <button class="demo-btn" on:click={runDemo} disabled={demoPhase === 1}>
        {demoPhase === 1 ? "…" : $t("mcp.tuto.demo_run")}
      </button>
    </div>

    {#if demoPhase > 0}
      <div class="demo-flow">
        <span class="dstep done">{$t("mcp.tuto.demo_f1")}</span>
        <span class="darrow">→</span>
        <span class="dstep" class:done={demoPhase === 2} class:active={demoPhase === 1}>{$t("mcp.tuto.demo_f2")}</span>
        <span class="darrow">→</span>
        <span class="dstep" class:done={demoPhase === 2}>{$t("mcp.tuto.demo_f3")}</span>
      </div>
      {#if demoMsg}<p class="demo-msg">{demoMsg}</p>{/if}
    {/if}

    <p class="tuto-note">{$t("mcp.tuto.note")}</p>
  {/if}
</div>

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
  <div class="bulk">
    <div class="bulk-info">
      <strong>⚡ {$t("mcp.bulk_title")}</strong>
      <span class="muted small">{$t("mcp.bulk_desc")}</span>
    </div>
    <button class="bulk-btn" on:click={connectAll} disabled={bulkBusy}>
      {bulkBusy ? "…" : $t("mcp.bulk_btn")}
    </button>
  </div>
  {#if bulkMsg}<p class="result ok bulk-msg">{bulkMsg}</p>{/if}

  <div class="grid">
    {#each filtered as s}
      <div class="mcp" class:open={openId === s.id}>
        <div class="top">
          <Logo slug={s.logo} label={s.name} />
          <div class="meta">
            <strong>{s.name}</strong>
            <span class="cat">{s.category} · {s.transport}</span>
          </div>
          {#if isConnected(s)}
            <span class="badge ok">{$t("mcp.connected")}</span>
          {:else}
            <span class="badge {needsKey(s) ? 'key' : 'easy'}">{needsKey(s) ? $t("mcp.needs_key") : $t("mcp.one_click")}</span>
          {/if}
        </div>
        <p class="desc">{s.description}</p>

        <div class="actions">
          <button on:click={() => startConnect(s)} disabled={busy === s.id || isConnected(s)}>
            {busy === s.id ? "…" : isConnected(s) ? "✓ " + $t("mcp.connected") : needsKey(s) ? $t("mcp.connect_key") : $t("mcp.connect_now")}
          </button>
          <a href={s.docs} target="_blank" rel="noreferrer" class="docs">GitHub ↗</a>
        </div>

        {#if openId === s.id && s.env}
          <div class="keyform">
            {#each s.env as f}
              <label>{f.label}
                <input type="password" bind:value={formEnv[f.key]} placeholder={f.key} autocomplete="off" />
                {#if f.link}<a href={f.link} target="_blank" rel="noreferrer" class="getlink">{$t("mcp.get_key")} ↗</a>{/if}
              </label>
            {/each}
            <button class="confirm" on:click={() => submitForm(s)} disabled={busy === s.id || !canSubmit(s, formEnv)}>
              {busy === s.id ? "…" : $t("mcp.connect_now")}
            </button>
          </div>
        {/if}

        {#if result[s.id]}
          <p class="result {result[s.id].ok ? 'ok' : 'err'}">{result[s.id].msg}</p>
        {/if}
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
  .badge { font-size: 0.66rem; padding: 0.1rem 0.45rem; border-radius: 20px; white-space: nowrap; }
  .badge.ok { background: var(--ok); color: #04231a; }
  .badge.easy { background: color-mix(in srgb, var(--ok) 20%, transparent); color: var(--ok); border: 1px solid color-mix(in srgb, var(--ok) 45%, transparent); }
  .badge.key { background: color-mix(in srgb, var(--warn) 18%, transparent); color: var(--warn); border: 1px solid color-mix(in srgb, var(--warn) 45%, transparent); }
  .mcp.open { border-color: var(--accent); }
  .keyform { margin-top: 0.6rem; display: grid; gap: 0.55rem; border-top: 1px solid var(--border); padding-top: 0.6rem; }
  .keyform label { display: block; font-size: 0.76rem; color: var(--muted); }
  .keyform input { width: 100%; margin-top: 0.2rem; background: #0b0e14; border: 1px solid var(--border); color: var(--text); border-radius: 7px; padding: 0.4rem 0.55rem; font: inherit; font-size: 0.8rem; box-sizing: border-box; }
  .getlink { font-size: 0.72rem; color: var(--accent); text-decoration: none; display: inline-block; margin-top: 0.25rem; }
  .keyform .confirm { background: var(--accent); border-color: var(--accent); color: #04231a; font-weight: 600; }
  .result { margin: 0.55rem 0 0; font-size: 0.76rem; padding: 0.4rem 0.55rem; border-radius: 7px; }
  .result.ok { background: color-mix(in srgb, var(--ok) 14%, transparent); color: var(--ok); }
  .result.err { background: color-mix(in srgb, var(--warn) 14%, transparent); color: var(--warn); }
  .bulk { margin-top: 1rem; display: flex; align-items: center; justify-content: space-between; gap: 0.8rem; flex-wrap: wrap; background: color-mix(in srgb, var(--accent) 10%, #0f1422); border: 1px solid color-mix(in srgb, var(--accent) 40%, var(--border)); border-radius: 10px; padding: 0.7rem 1rem; }
  .bulk-info { display: flex; flex-direction: column; gap: 0.15rem; }
  .bulk-btn { border: 1px solid var(--accent); background: var(--accent); color: #04231a; font-weight: 700; border-radius: 8px; padding: 0.55rem 1.1rem; cursor: pointer; font: inherit; font-size: 0.88rem; white-space: nowrap; }
  .bulk-btn:disabled { opacity: 0.6; cursor: default; }
  .bulk-msg { margin-top: 0.6rem; }
  .small { font-size: 0.78rem; }

  /* Tutorial + demo */
  .tuto { margin-bottom: 1rem; border: 1px solid color-mix(in srgb, var(--accent) 35%, var(--border)); }
  .tuto-head { display: flex; align-items: center; gap: 0.6rem; width: 100%; background: none; border: none; padding: 0; cursor: pointer; color: var(--text); text-align: left; }
  .tuto-head h2 { margin: 0; font-size: 1.05rem; }
  .tuto-chev { color: var(--accent); font-size: 0.9rem; }
  .tuto-sub { flex: 1; }
  .steps { list-style: none; margin: 1rem 0 0; padding: 0; display: grid; gap: 0.7rem; }
  .steps li { display: flex; gap: 0.7rem; align-items: flex-start; }
  .steps .num { flex: 0 0 auto; width: 24px; height: 24px; border-radius: 50%; background: var(--accent); color: #04231a; font-weight: 700; font-size: 0.8rem; display: grid; place-items: center; }
  .steps strong { font-size: 0.9rem; }
  .steps p { margin: 0.15rem 0 0; color: var(--muted); font-size: 0.82rem; }
  .demo { margin-top: 1.1rem; display: flex; align-items: center; justify-content: space-between; gap: 0.8rem; background: #0f1422; border: 1px dashed color-mix(in srgb, var(--accent) 45%, var(--border)); border-radius: 10px; padding: 0.8rem 1rem; flex-wrap: wrap; }
  .demo-info p { margin: 0.2rem 0 0; }
  .demo-btn { border: 1px solid var(--accent); background: var(--accent); color: #04231a; font-weight: 600; border-radius: 8px; padding: 0.5rem 1rem; cursor: pointer; font: inherit; font-size: 0.85rem; }
  .demo-btn:disabled { opacity: 0.6; cursor: default; }
  .demo-flow { display: flex; align-items: center; gap: 0.5rem; margin-top: 0.8rem; flex-wrap: wrap; }
  .dstep { font-size: 0.76rem; padding: 0.25rem 0.6rem; border-radius: 20px; border: 1px solid var(--border); color: var(--muted); }
  .dstep.active { border-color: var(--warn); color: var(--warn); animation: pulse 1s infinite; }
  .dstep.done { border-color: var(--ok); color: var(--ok); }
  .darrow { color: var(--muted); }
  @keyframes pulse { 50% { opacity: 0.5; } }
  .demo-msg { margin-top: 0.6rem; background: #11203a; border: 1px solid var(--border); padding: 0.5rem 0.7rem; border-radius: 8px; font-size: 0.8rem; }
  .tuto-note { margin-top: 1rem; color: var(--muted); font-size: 0.76rem; border-top: 1px solid var(--border); padding-top: 0.7rem; }
</style>
