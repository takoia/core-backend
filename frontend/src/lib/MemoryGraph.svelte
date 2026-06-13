<script lang="ts">
  import { onMount } from "svelte";
  import { SvelteFlow, Background, Controls, MiniMap, type Node, type Edge } from "@xyflow/svelte";
  import "@xyflow/svelte/dist/style.css";
  import { api, type Agent } from "./api";
  import { t } from "./i18n";

  // Memory map: a 2D graph of stored memories, either for a single agent or
  // for the whole organization (all agents at once).
  let scope = $state<"agent" | "org">("org");
  let agents = $state<Agent[]>([]);
  let selectedAgent = $state<string | null>(null);
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

  function memNode(id: string, label: string, x: number, y: number, kind: string): Node {
    return { id, position: { x, y }, data: { label }, class: `mg-${kind}` } as Node;
  }

  async function buildAgent(aid: string) {
    const mems = await api.memories(aid);
    truncated = Math.max(0, mems.length - MAX_PER_AGENT);
    const ns: Node[] = [memNode(`a:${aid}`, `${agentIcon(aid)} ${agentName(aid)}`, 0, 0, "agent")];
    const es: Edge[] = [];
    const shown = mems.slice(0, MAX_PER_AGENT);
    const R = 320;
    shown.forEach((m, i) => {
      const ang = (i / Math.max(1, shown.length)) * Math.PI * 2;
      const id = `m:${aid}:${i}`;
      ns.push(memNode(id, `[${m.key}] ${short(m.content)}`, Math.cos(ang) * R, Math.sin(ang) * R, "mem"));
      es.push({ id: `e:${id}`, source: `a:${aid}`, target: id });
    });
    nodes = ns;
    edges = es;
  }

  async function buildOrg() {
    const ids = agents.map((a) => a.id);
    const ns: Node[] = [memNode("org", "🏢 Organisation", 0, 0, "org")];
    const es: Edge[] = [];
    let trunc = 0;
    const AR = 520; // agent ring radius
    const results = await Promise.all(ids.map((id) => api.memories(id).then((m) => ({ id, m })).catch(() => ({ id, m: [] }))));
    results.forEach(({ id, m }, ai) => {
      const ang = (ai / Math.max(1, ids.length)) * Math.PI * 2;
      const ax = Math.cos(ang) * AR, ay = Math.sin(ang) * AR;
      const aNode = `a:${id}`;
      ns.push(memNode(aNode, `${agentIcon(id)} ${agentName(id)} (${m.length})`, ax, ay, "agent"));
      es.push({ id: `e:org:${id}`, source: "org", target: aNode });
      trunc += Math.max(0, m.length - MAX_PER_AGENT);
      m.slice(0, MAX_PER_AGENT).forEach((mem, i) => {
        const mid = `m:${id}:${i}`;
        const ma = ang + (i - (Math.min(m.length, MAX_PER_AGENT) - 1) / 2) * 0.22;
        ns.push(memNode(mid, `[${mem.key}] ${short(mem.content, 32)}`, ax + Math.cos(ma) * 200, ay + Math.sin(ma) * 200, "mem"));
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
      if (scope === "org") await buildOrg();
      else if (selectedAgent) await buildAgent(selectedAgent);
      else { nodes = []; edges = []; }
    } finally {
      loading = false;
    }
  }

  onMount(async () => {
    try { agents = await api.listAgents(); } catch { agents = []; }
    if (!selectedAgent && agents.length) selectedAgent = agents[0].id;
    await rebuild();
  });

  function setScope(s: "agent" | "org") { scope = s; rebuild(); }
  function pickAgent(e: Event) { selectedAgent = (e.target as HTMLSelectElement).value; if (scope === "agent") rebuild(); }
</script>

<div class="mgwrap card">
  <div class="mgbar">
    <strong>{$t("memory.mapTitle")}</strong>
    <label class="chk"><input type="radio" name="scope" checked={scope === "org"} onchange={() => setScope("org")} /> {$t("memory.scopeOrg")}</label>
    <label class="chk"><input type="radio" name="scope" checked={scope === "agent"} onchange={() => setScope("agent")} /> {$t("memory.scopeAgent")}</label>
    {#if scope === "agent"}
      <select value={selectedAgent} onchange={pickAgent}>
        {#each agents as a}<option value={a.id}>{a.name}</option>{/each}
      </select>
    {/if}
    {#if loading}<span class="mgsp"></span>{/if}
    {#if truncated > 0}<span class="mgtr">+{truncated} {$t("memory.hidden")}</span>{/if}
    <button class="mgrf" onclick={rebuild}>↻</button>
  </div>
  <div class="mgflow">
    <SvelteFlow bind:nodes bind:edges fitView>
      <Background gap={24} />
      <Controls />
      <MiniMap pannable zoomable />
    </SvelteFlow>
  </div>
</div>

<style>
  .mgwrap { padding: 0.6rem; }
  .mgbar { display: flex; align-items: center; gap: 0.8rem; padding: 0.3rem 0.4rem 0.6rem; flex-wrap: wrap; }
  .chk { display: inline-flex; align-items: center; gap: 0.3rem; font-size: 0.85rem; color: var(--text); cursor: pointer; }
  .mgbar select { background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.3rem 0.5rem; font: inherit; }
  .mgrf { margin-left: auto; background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.3rem 0.55rem; cursor: pointer; }
  .mgtr { font-size: 0.74rem; color: var(--muted); }
  .mgsp { width: 14px; height: 14px; border: 2px solid var(--border); border-top-color: var(--accent); border-radius: 50%; animation: mgspin 0.7s linear infinite; }
  @keyframes mgspin { to { transform: rotate(360deg); } }
  .mgflow { height: 60vh; border: 1px solid var(--border); border-radius: 10px; overflow: hidden; }
  .mgflow :global(.mg-org) { background: color-mix(in srgb, var(--accent) 30%, var(--panel)); border: 2px solid var(--accent); color: var(--text); font-weight: 700; border-radius: 12px; }
  .mgflow :global(.mg-agent) { background: color-mix(in srgb, var(--accent) 16%, var(--panel)); border: 1.5px solid color-mix(in srgb, var(--accent) 55%, var(--border)); color: var(--text); font-weight: 600; border-radius: 10px; }
  .mgflow :global(.mg-mem) { background: color-mix(in srgb, var(--ok) 10%, var(--panel)); border: 1px solid color-mix(in srgb, var(--ok) 40%, var(--border)); color: var(--text); font-size: 0.74rem; border-radius: 8px; max-width: 220px; }
</style>
