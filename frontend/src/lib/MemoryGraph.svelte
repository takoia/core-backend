<script lang="ts">
  import { onMount } from "svelte";
  import { SvelteFlow, Background, Controls, MiniMap, type Node, type Edge } from "@xyflow/svelte";
  import "@xyflow/svelte/dist/style.css";
  import { api, type Agent } from "./api";
  import { t } from "./i18n";

  // Memory map: a 2D graph of stored memories for any selection of agents
  // (1 to n via the listbox); selecting all gives the org-wide view.
  let agents = $state<Agent[]>([]);
  let selectedAgents = $state<string[]>([]);
  let loading = $state(false);
  const MAX_PER_AGENT = 8; // cap leaf nodes per agent to keep the map readable
  let truncated = $state(0);

  let nodes = $state.raw<Node[]>([]);
  let edges = $state.raw<Edge[]>([]);

  const short = (s: string, n = 46) => (s.length > n ? s.slice(0, n - 1) + "…" : s);
  const agentName = (id: string) => agents.find((a) => a.id === id)?.name ?? id;
  const agentIcon = (id: string) => {
    const ic = agents.find((a) => a.id === id)?.icon;
    return ic && !/^(data:|https?:)/.test(ic) ? ic : "🐙";
  };
  const mkNode = (id: string, label: string, x: number, y: number, kind: string): Node =>
    ({ id, position: { x, y }, data: { label }, class: `mg-${kind}` }) as Node;

  // A memory normalized with its ICM importance metadata.
  type Mem = { text: string; weight: number; access: number; imp: string };
  async function loadMems(aid: string): Promise<Mem[]> {
    try {
      const e = await api.icmMemories(aid);
      if (e.length) return e.map((x) => ({ text: x.summary, weight: x.weight, access: x.access_count, imp: x.importance }));
    } catch { /* fall back to DB mirror */ }
    const m = await api.memories(aid).catch(() => []);
    return m.map((x) => ({ text: x.content, weight: 1, access: 0, imp: "medium" }));
  }

  // Node size/color reflect REAL ICM importance: weight × recall reinforcement
  // (access_count). Frequently recalled + high-weight memories stand out.
  const mkMem = (id: string, mem: Mem, x: number, y: number): Node => {
    const score = mem.weight * (1 + Math.log2(1 + mem.access));
    const imp = mem.imp === "high" || score > 1.5 ? "hi" : (mem.imp === "low" || score < 0.6) ? "lo" : "mid";
    const w = Math.round(Math.min(280, 120 + score * 55 + (mem.text || "").length / 12));
    return {
      id, position: { x, y },
      data: { label: `${short(mem.text, 26)}  ↺${mem.access}`, full: mem.text, mkey: `${mem.imp} · poids ${mem.weight.toFixed(2)} · rappels ${mem.access}` },
      class: `mg-mem mg-${imp}`, style: `width:${w}px`,
    } as Node;
  };

  // Detail of the clicked memory node.
  let detail = $state<{ key: string; content: string } | null>(null);
  function onNodeClick(e: any) {
    const d = (e?.node?.data ?? e?.detail?.node?.data) as any;
    if (d?.full) detail = { key: d.mkey, content: d.full };
  }

  async function buildOne(aid: string) {
    const mems = await loadMems(aid);
    truncated = Math.max(0, mems.length - MAX_PER_AGENT);
    const ns: Node[] = [mkNode(`a:${aid}`, `${agentIcon(aid)} ${agentName(aid)}`, 0, 0, "agent")];
    const es: Edge[] = [];
    const shown = mems.slice(0, MAX_PER_AGENT);
    shown.forEach((m, i) => {
      const ang = (i / Math.max(1, shown.length)) * Math.PI * 2;
      const id = `m:${aid}:${i}`;
      ns.push(mkMem(id, m, Math.cos(ang) * 320, Math.sin(ang) * 320));
      es.push({ id: `e:${id}`, source: `a:${aid}`, target: id });
    });
    nodes = ns;
    edges = es;
  }

  async function buildMany(ids: string[]) {
    const ns: Node[] = [mkNode("org", "🏢 Organisation", 0, 0, "org")];
    const es: Edge[] = [];
    let trunc = 0;
    const AR = 520; // agent ring radius
    const results = await Promise.all(
      ids.map((id) => loadMems(id).then((m) => ({ id, m })).catch(() => ({ id, m: [] as Mem[] }))),
    );
    results.forEach(({ id, m }, ai) => {
      const ang = (ai / Math.max(1, ids.length)) * Math.PI * 2;
      const ax = Math.cos(ang) * AR, ay = Math.sin(ang) * AR;
      const aNode = `a:${id}`;
      ns.push(mkNode(aNode, `${agentIcon(id)} ${agentName(id)} (${m.length})`, ax, ay, "agent"));
      es.push({ id: `e:org:${id}`, source: "org", target: aNode });
      trunc += Math.max(0, m.length - MAX_PER_AGENT);
      m.slice(0, MAX_PER_AGENT).forEach((mem, i) => {
        const mid = `m:${id}:${i}`;
        const ma = ang + (i - (Math.min(m.length, MAX_PER_AGENT) - 1) / 2) * 0.22;
        ns.push(mkMem(mid, mem, ax + Math.cos(ma) * 200, ay + Math.sin(ma) * 200));
        es.push({ id: `e:${mid}`, source: aNode, target: mid });
      });
    });
    truncated = trunc;
    nodes = ns;
    edges = es;
  }

  async function rebuild() {
    loading = true;
    try {
      const ids = selectedAgents.length ? selectedAgents : agents.map((a) => a.id);
      if (ids.length === 0) { nodes = []; edges = []; }
      else if (ids.length === 1) await buildOne(ids[0]);
      else await buildMany(ids);
    } finally {
      loading = false;
    }
  }

  function onPick(e: Event) {
    const sel = e.target as HTMLSelectElement;
    selectedAgents = Array.from(sel.selectedOptions).map((o) => o.value);
    rebuild();
  }
  function selectAll() { selectedAgents = agents.map((a) => a.id); rebuild(); }
  function selectNone() { selectedAgents = []; rebuild(); }

  onMount(async () => {
    try { agents = await api.listAgents(); } catch { agents = []; }
    selectedAgents = agents.map((a) => a.id); // default: whole organization
    await rebuild();
  });
</script>

<div class="mgwrap card">
  <div class="mgbar">
    <strong>{$t("memory.mapTitle")}</strong>
    <select multiple size="1" class="aglist" onchange={onPick} title={$t("memory.pickAgents")}>
      {#each agents as a}
        <option value={a.id} selected={selectedAgents.includes(a.id)}>{a.name}</option>
      {/each}
    </select>
    <button class="mgbtn" onclick={selectAll}>{$t("memory.scopeOrg")}</button>
    <button class="mgbtn" onclick={selectNone}>—</button>
    <span class="mgcount">{(selectedAgents.length || agents.length)} / {agents.length}</span>
    {#if loading}<span class="mgsp"></span>{/if}
    {#if truncated > 0}<span class="mgtr">+{truncated} {$t("memory.hidden")}</span>{/if}
    <button class="mgrf" onclick={rebuild}>↻</button>
  </div>
  <div class="mglegend">
    <span class="muted">Importance :</span>
    <span class="lg lg-hi">élevée</span><span class="lg lg-mid">moyenne</span><span class="lg lg-lo">faible</span>
    <span class="muted">(taille = richesse de la mémoire)</span>
  </div>
  <div class="mgflow">
    <SvelteFlow bind:nodes bind:edges fitView onnodeclick={onNodeClick}>
      <Background gap={24} />
      <Controls />
      <MiniMap pannable zoomable />
    </SvelteFlow>
    {#if detail}
      <div class="mgdetail">
        <div class="mgdhead"><strong>{detail.key}</strong><button class="mgx" onclick={() => (detail = null)} aria-label="close">×</button></div>
        <pre>{detail.content}</pre>
      </div>
    {/if}
  </div>
</div>

<style>
  .mgwrap { padding: 0.6rem; width: 100%; box-sizing: border-box; }
  .mglegend { display: flex; align-items: center; gap: 0.5rem; font-size: 0.72rem; padding: 0 0.4rem 0.4rem; flex-wrap: wrap; }
  .mglegend .muted { color: var(--muted); }
  .lg { padding: 0.05rem 0.45rem; border-radius: 20px; color: var(--text); }
  .lg-hi { background: color-mix(in srgb, var(--err) 40%, var(--panel)); border: 1px solid var(--err); }
  .lg-mid { background: color-mix(in srgb, var(--warn) 35%, var(--panel)); border: 1px solid var(--warn); }
  .lg-lo { background: color-mix(in srgb, var(--ok) 20%, var(--panel)); border: 1px solid color-mix(in srgb, var(--ok) 50%, var(--border)); }
  .mgbar { display: flex; align-items: center; gap: 0.7rem; padding: 0.3rem 0.4rem 0.6rem; flex-wrap: wrap; }
  .aglist { background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.25rem 0.4rem; font: inherit; font-size: 0.82rem; min-width: 160px; max-height: 92px; overflow-y: auto; }
  .aglist[multiple] { height: auto; }
  .mgbtn, .mgrf { background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.3rem 0.55rem; cursor: pointer; font: inherit; font-size: 0.8rem; }
  .mgrf { margin-left: auto; }
  .mgcount { font-size: 0.74rem; color: var(--muted); }
  .mgtr { font-size: 0.74rem; color: var(--muted); }
  .mgsp { width: 14px; height: 14px; border: 2px solid var(--border); border-top-color: var(--accent); border-radius: 50%; animation: mgspin 0.7s linear infinite; }
  @keyframes mgspin { to { transform: rotate(360deg); } }
  .mgflow { position: relative; height: 78vh; border: 1px solid var(--border); border-radius: 10px; overflow: hidden; }
  .mgflow :global(.mg-org) { background: color-mix(in srgb, var(--accent) 30%, var(--panel)); border: 2px solid var(--accent); color: var(--text); font-weight: 700; border-radius: 12px; }
  .mgflow :global(.mg-agent) { background: color-mix(in srgb, var(--accent) 16%, var(--panel)); border: 1.5px solid color-mix(in srgb, var(--accent) 55%, var(--border)); color: var(--text); font-weight: 600; border-radius: 10px; }
  .mgflow :global(.mg-mem) { color: var(--text); font-size: 0.74rem; border-radius: 8px; cursor: pointer; }
  /* Importance-driven color: high = red, mid = amber, low = green. */
  .mgflow :global(.mg-hi) { background: color-mix(in srgb, var(--err) 26%, var(--panel)); border: 1.5px solid var(--err); }
  .mgflow :global(.mg-mid) { background: color-mix(in srgb, var(--warn) 22%, var(--panel)); border: 1.5px solid var(--warn); }
  .mgflow :global(.mg-lo) { background: color-mix(in srgb, var(--ok) 12%, var(--panel)); border: 1px solid color-mix(in srgb, var(--ok) 45%, var(--border)); }
  .mgdetail { position: absolute; top: 10px; right: 10px; width: 340px; max-height: 70%; overflow-y: auto; background: var(--panel); border: 1px solid var(--accent); border-radius: 12px; box-shadow: 0 12px 40px rgba(0,0,0,0.5); z-index: 20; }
  .mgdhead { display: flex; align-items: center; justify-content: space-between; padding: 0.6rem 0.8rem; border-bottom: 1px solid var(--border); }
  .mgx { background: none; border: none; color: var(--muted); cursor: pointer; font-size: 1.2rem; line-height: 1; }
  .mgdetail pre { margin: 0; padding: 0.7rem 0.8rem; white-space: pre-wrap; word-break: break-word; font-family: ui-monospace, monospace; font-size: 0.76rem; }
</style>
