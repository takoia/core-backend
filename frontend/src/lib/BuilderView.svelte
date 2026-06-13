<script lang="ts">
  import { onMount, onDestroy, untrack } from "svelte";
  import { SvelteFlow, Background, Controls, MiniMap, type Node, type Edge } from "@xyflow/svelte";
  import "@xyflow/svelte/dist/style.css";
  import StepNode from "./builder/StepNode.svelte";
  import VideoView from "./VideoView.svelte";
  import { api, subscribeJob, type Agent } from "./api";
  import { t } from "./i18n";
  import { toast, confirmModal } from "./toast";

  const nodeTypes = { block: StepNode } as any;

  // ── Palette: the blocks you drag onto the canvas (Scratch-like) ────────────
  const PALETTE = [
    { group: "Déclencheurs", items: [
      { key: "trigger_manual", label: "Manuel", glyph: "▶️", kind: "trigger" },
      { key: "trigger_email", label: "Email reçu", glyph: "📧", kind: "trigger" },
      { key: "trigger_webhook", label: "Webhook", glyph: "🔗", kind: "trigger" },
      { key: "trigger_ftp", label: "Fichier FTP", glyph: "📁", kind: "trigger" },
      { key: "trigger_schedule", label: "Planifié", glyph: "⏰", kind: "trigger" },
    ]},
    { group: "Analyse", items: [
      { key: "analyse", label: "Analyse", glyph: "🔍", kind: "step" },
      { key: "decision", label: "Décision", glyph: "🧭", kind: "step" },
      { key: "web_search", label: "Recherche web", glyph: "🌐", kind: "tool" },
      { key: "market_data", label: "Données marché", glyph: "📈", kind: "tool" },
      { key: "analyse_video", label: "Analyse vidéo", glyph: "🎥", kind: "tool" },
      { key: "analyse_image", label: "Analyse image", glyph: "🖼️", kind: "tool" },
      { key: "analyse_sound", label: "Analyse son", glyph: "🎙️", kind: "tool" },
      { key: "analyse_text", label: "Analyse texte", glyph: "📝", kind: "tool" },
    ]},
    { group: "Conditions", items: [
      { key: "if", label: "Condition", glyph: "❓", kind: "control" },
      { key: "for", label: "Pour chaque", glyph: "🔢", kind: "control" },
      { key: "loop", label: "Boucle", glyph: "🔁", kind: "control" },
    ]},
    { group: "Restitutions", items: [
      { key: "action", label: "Action", glyph: "⚙️", kind: "step" },
      { key: "restitution", label: "Restitution", glyph: "📄", kind: "step" },
      { key: "send_email", label: "Email", glyph: "✉️", kind: "tool" },
      { key: "send_discord", label: "Webhook / Discord", glyph: "🔔", kind: "tool" },
      { key: "write_file", label: "Écrire un fichier", glyph: "💾", kind: "tool" },
      { key: "write_calendar", label: "Calendrier", glyph: "📅", kind: "tool" },
    ]},
  ];
  // Default event each trigger block emits as `[trigger] on = "..."`.
  const TRIGGER_EVENT: Record<string, string> = {
    trigger_manual: "manual", trigger_email: "email.received",
    trigger_webhook: "webhook.received", trigger_ftp: "file.received",
    trigger_schedule: "schedule",
  };
  type Block = { key: string; label: string; glyph: string; kind: string };
  const ALL_BLOCKS: Record<string, Block> = Object.fromEntries(
    PALETTE.flatMap((g) => g.items).map((b) => [b.key, b]),
  );
  const PARAM_KEYS: Record<string, string[]> = {
    trigger_manual: ["event"], trigger_email: ["event"], trigger_webhook: ["event"],
    trigger_ftp: ["event"], trigger_schedule: ["event"],
    web_search: ["site"],
    market_data: ["symbol"],
    send_discord: ["discord_webhook"],
    send_email: ["recipient", "subject"],
    write_file: ["filename"],
    write_calendar: ["calendar_url", "title"],
    analyse_video: ["source_url"],
    analyse_image: ["source_url"],
    analyse_sound: ["source_url"],
    analyse_text: ["source_url"],
  };
  const PARAM_PH: Record<string, string> = {
    event: "ex. email.received, webhook.received, schedule",
    site: "ex. windguru.com (laisser vide = tout le web)",
    symbol: "^IXIC, AAPL, NVDA…", discord_webhook: "https://discord.com/api/webhooks/…",
    recipient: "name@example.com", subject: "Objet de l'email",
    filename: "rapport.md", calendar_url: "ICS / CalDAV URL", title: "Titre de l'événement",
    source_url: "URL du média (ou laisser vide)",
  };
  const PARAM_LABEL: Record<string, string> = {
    event: "Événement déclencheur",
    site: "Site web", symbol: "Symbole", discord_webhook: "URL Webhook", recipient: "Destinataire",
    subject: "Objet", filename: "Nom du fichier", calendar_url: "URL calendrier", title: "Titre",
    source_url: "URL source",
  };

  // ── Agent state ────────────────────────────────────────────────────────────
  let editingId = $state<string | null>(null);
  let name = $state("Nouvel agent");
  let icon = $state("🐙");
  let description = $state("");
  let author = $state("You");
  let expertise = $state("");
  let autonomy = $state<"full_auto" | "confirm_before_action">("full_auto");
  let triggerOn = $state("");
  let emit = $state("");
  let loopMinutes = $state(0);
  let goals = $state("");
  let checks = $state("");

  // Per-node config (keyed by node id): prompts for steps, params for tools.
  let prompts = $state<Record<string, string>>({});
  let params = $state<Record<string, Record<string, string>>>({ "trigger-0": { event: "manual" } });

  let selected = $state<string | null>("agent");
  let busy = $state(false);
  const isImg = (s: string) => /^(https?:|data:)/.test(s || "");

  // ── Agent list ─────────────────────────────────────────────────────────────
  let agentList = $state<Agent[]>([]);
  async function refreshAgents() { try { agentList = await api.listAgents(); } catch { agentList = []; } }
  onMount(refreshAgents);

  function agentEmoji(a: Agent): string {
    if (a.icon) return a.icon;
    const e = (a.expertise_domain || "").toLowerCase();
    if (e.includes("trad") || e.includes("market") || e.includes("nasdaq")) return "📈";
    if (e.includes("invoice") || e.includes("fact")) return "🧾";
    if (e.includes("meteo") || e.includes("weather")) return "🌦️";
    return "🐙";
  }

  // ── Canvas: starts with ONE agent box; you drag blocks to add steps ────────
  let counter = $state(0);
  // The agent (root) node, plus a default Manual trigger wired right after it.
  function freshGraph(): { nodes: Node[]; edges: Edge[] } {
    return {
      nodes: [
        { id: "agent", type: "block", position: { x: 0, y: 0 }, data: { label: name, kind: "trigger", glyph: icon, sub: "agent", root: true } },
        { id: "trigger-0", type: "block", position: { x: 0, y: 130 }, data: { label: "Manuel", kind: "trigger", glyph: "▶️", sub: "déclencheur" } },
      ],
      edges: [{ id: "e-agent-trigger-0", source: "agent", target: "trigger-0", animated: true }],
    };
  }
  const _g0 = freshGraph();
  let nodes = $state.raw<Node[]>(_g0.nodes);
  let edges = $state.raw<Edge[]>(_g0.edges);
  let lastId = $state("trigger-0");

  function newAgent() {
    editingId = null;
    name = "Nouvel agent"; icon = "🐙"; description = ""; author = "You"; expertise = "";
    autonomy = "full_auto"; triggerOn = ""; emit = ""; loopMinutes = 0; goals = ""; checks = "";
    prompts = {}; params = { "trigger-0": { event: "manual" } }; counter = 0; lastId = "trigger-0";
    const g = freshGraph();
    nodes = g.nodes;
    edges = g.edges;
    selected = "agent"; resetRun();
  }

  function addBlock(key: string, pos?: { x: number; y: number }) {
    const b = ALL_BLOCKS[key];
    if (!b) return;
    counter += 1;
    const id = `${key}-${counter}`;
    const position = pos ?? { x: 0, y: 130 + counter * 120 };
    const node: Node = { id, type: "block", position, data: { label: b.label, kind: b.kind, glyph: b.glyph } };
    const edge: Edge = { id: `e-${lastId}-${id}`, source: lastId, target: id, animated: true };
    nodes = [...nodes, node];
    edges = [...edges, edge];
    lastId = id;
    selected = id;
    // A trigger block carries its event and also seeds the agent trigger field.
    if (b.kind === "trigger") {
      const ev = TRIGGER_EVENT[key] ?? "";
      params = { ...params, [id]: { event: ev } };
      if (ev && ev !== "manual") triggerOn = ev;
    }
    return id;
  }

  function removeNode(id: string) {
    if (id === "agent") return;
    nodes = nodes.filter((n) => n.id !== id);
    edges = edges.filter((e) => e.source !== id && e.target !== id);
    if (selected === id) selected = "agent";
  }

  // Drag from palette -> drop on canvas.
  let flowEl: HTMLDivElement;
  function onDrop(e: DragEvent) {
    e.preventDefault();
    const key = e.dataTransfer?.getData("text/plain");
    if (!key) return;
    const rect = flowEl.getBoundingClientRect();
    addBlock(key, { x: e.clientX - rect.left - 95, y: e.clientY - rect.top - 30 });
  }

  function onNodeClick(e: any) {
    const id = e?.node?.id ?? e?.detail?.node?.id;
    if (!id) return;
    selected = id;
    // The agent (trigger) node is edited in the inspector; every other node
    // opens the properties modal so it works even with panels collapsed.
    if (id !== "agent") modalOpen = true;
  }

  // Right-click context menu: on the canvas it adds a block (New); on a node
  // it offers Modify / Delete.
  let ctxMenu = $state<{ sx: number; sy: number; cx: number; cy: number; nodeId?: string } | null>(null);
  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    const rect = flowEl.getBoundingClientRect();
    ctxMenu = { sx: e.clientX, sy: e.clientY, cx: e.clientX - rect.left - 95, cy: e.clientY - rect.top - 30 };
  }
  function onNodeContextMenu(e: any) {
    const ev: MouseEvent = e?.event ?? e;
    const id: string = e?.node?.id ?? e?.detail?.node?.id;
    ev?.preventDefault?.();
    if (!id) return;
    ctxMenu = { sx: (ev as MouseEvent).clientX, sy: (ev as MouseEvent).clientY, cx: 0, cy: 0, nodeId: id };
  }
  function addFromMenu(key: string) {
    if (ctxMenu) addBlock(key, { x: ctxMenu.cx, y: ctxMenu.cy });
    ctxMenu = null;
  }
  function editFromMenu() {
    if (ctxMenu?.nodeId) { selected = ctxMenu.nodeId; if (ctxMenu.nodeId !== "agent") modalOpen = true; }
    ctxMenu = null;
  }
  function deleteFromMenu() {
    if (ctxMenu?.nodeId) removeNode(ctxMenu.nodeId);
    ctxMenu = null;
  }

  // Keep the agent node + live status in sync without looping (untrack).
  $effect(() => {
    const nm = name, ic = icon, ss = stepStatus;
    nodes = untrack(() => nodes).map((n) => {
      if (n.id === "agent") return { ...n, data: { ...n.data, label: nm, glyph: ic } };
      const st = ss[n.id];
      return st ? { ...n, data: { ...n.data, status: st } } : n;
    });
  });

  // Selected node helpers.
  const selNode = $derived(nodes.find((n) => n.id === selected) ?? null);
  // A trigger node on the canvas supplies the event, so the global trigger
  // field is hidden to avoid duplication.
  const hasTriggerNode = $derived(nodes.some((n) => n.data.kind === "trigger" && n.id !== "agent"));
  function blockKey(id: string): string { return id.replace(/-\d+$/, ""); }

  // ── Build TOML from the graph ──────────────────────────────────────────────
  const slug = (s: string) => s.toLowerCase().trim().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "agent";
  function buildToml(): string {
    const stepNodes = nodes.filter((n) => n.data.kind === "step");
    const toolNodes = nodes.filter((n) => n.data.kind === "tool");
    const desc = (description || goals || "Built in TakoIA.").replace(/\n/g, " ");
    const emitArr = emit.split(",").map((x) => x.trim()).filter(Boolean);
    let toml = `[agent]\nid = "${editingId ?? slug(name)}"\nname = "${name}"\nauthor = "${author}"\nversion = "0.1.0"\n`;
    toml += `description = ${JSON.stringify(desc)}\nexpertise = "${expertise}"\nautonomy = "${autonomy}"\nicon = "${icon}"\n`;
    toml += `emit = [${emitArr.map((x) => `"${x}"`).join(", ")}]\n`;
    // Prefer an explicit trigger node's event; fall back to the trigger field.
    const trigNode = nodes.find((n) => n.data.kind === "trigger" && n.id !== "agent");
    const trigEvent = (trigNode && params[trigNode.id]?.event?.trim()) || triggerOn.trim();
    if (trigEvent && trigEvent !== "manual") toml += `\n[trigger]\non = "${trigEvent}"\n`;
    // Steps (by step key; prompt from the node).
    const seen = new Set<string>();
    for (const n of stepNodes) {
      const key = blockKey(n.id);
      if (seen.has(key)) continue;
      seen.add(key);
      const p = (prompts[n.id] || "").trim();
      const isAction = key === "action";
      const tools = isAction ? toolNodes.map((tn) => blockKey(tn.id)) : [];
      if (!p && !tools.length) continue;
      toml += `\n[steps.${key}]\n`;
      if (p) toml += `system_prompt = ${JSON.stringify(p)}\n`;
      if (tools.length) toml += `allowed_tools = [${tools.map((x) => `"${x}"`).join(", ")}]\n`;
      // Tool params merged from all tool nodes.
      if (isAction) {
        const tp: Record<string, string> = {};
        for (const tn of toolNodes) for (const pk of PARAM_KEYS[blockKey(tn.id)] ?? []) {
          const v = params[tn.id]?.[pk]; if (v) tp[pk] = v;
        }
        const pairs = Object.entries(tp).map(([k, v]) => `${k} = ${JSON.stringify(v)}`);
        if (pairs.length) toml += `tool_params = { ${pairs.join(", ")} }\n`;
      }
    }
    // If there are tools but no explicit action step, add one.
    if (toolNodes.length && !seen.has("action")) {
      const tools = toolNodes.map((tn) => blockKey(tn.id));
      toml += `\n[steps.action]\nallowed_tools = [${tools.map((x) => `"${x}"`).join(", ")}]\n`;
      const tp: Record<string, string> = {};
      for (const tn of toolNodes) for (const pk of PARAM_KEYS[blockKey(tn.id)] ?? []) { const v = params[tn.id]?.[pk]; if (v) tp[pk] = v; }
      const pairs = Object.entries(tp).map(([k, v]) => `${k} = ${JSON.stringify(v)}`);
      if (pairs.length) toml += `tool_params = { ${pairs.join(", ")} }\n`;
    }
    return toml;
  }

  async function loadAgent(id: string) {
    const d = await api.getAgent(id);
    newAgent();
    editingId = d.agent.id;
    name = d.agent.name; icon = d.agent.icon || ""; description = d.agent.description ?? "";
    author = d.agent.author ?? ""; expertise = d.agent.expertise_domain ?? "";
    autonomy = d.agent.autonomy_level === "full_auto" ? "full_auto" : "confirm_before_action";
    triggerOn = (d.agent as any).trigger_on ?? "";
    try { emit = (JSON.parse((d.agent as any).emit ?? "[]") as string[]).join(", "); } catch { emit = ""; }
    // Rebuild the graph from the saved steps + tools.
    let toolKeys: string[] = [];
    const tParams: Record<string, string> = {};
    for (const sc of d.steps) {
      let opt: any = {}; try { opt = JSON.parse(sc.options || "{}"); } catch { /* */ }
      if (Array.isArray(opt.allowed_tools)) toolKeys = opt.allowed_tools;
      if (opt.tool_params) Object.assign(tParams, opt.tool_params);
      if ((sc.system_prompt || "").trim() || ["analyse","decision","action","restitution"].includes(sc.step_type)) {
        // only add a step node if it has a prompt or is one of the 4 with content
      }
    }
    // Add step nodes that have prompts, in order.
    for (const sc of d.steps) {
      if ((sc.system_prompt || "").trim()) {
        addBlock(sc.step_type);
        const id2 = lastId; prompts[id2] = sc.system_prompt;
      }
    }
    for (const tk of toolKeys) {
      if (ALL_BLOCKS[tk]) { addBlock(tk); const id2 = lastId;
        for (const pk of PARAM_KEYS[tk] ?? []) if (tParams[pk]) { params[id2] = { ...(params[id2]||{}), [pk]: tParams[pk] }; } }
    }
    selected = "agent"; resetRun();
  }

  async function save() {
    busy = true;
    try {
      const r = await api.importToml(buildToml());
      editingId = r.id;
      if (loopMinutes > 0) {
        const checkLine = checks.trim() ? `\n\nVérifie:\n${checks.trim()}` : "";
        await fetch("/api/schedules", { method: "POST", headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ agent_id: r.id, title: `${name} loop`, prompt: (goals.trim() || `Run ${name}`) + checkLine, interval_seconds: loopMinutes * 60 }) });
      }
      toast($t("builder.created"), "success");
      await refreshAgents();
      return r.id;
    } catch (e) { toast(e instanceof Error ? e.message : String(e), "error"); }
    finally { busy = false; }
  }

  async function deleteAgent(id: string, ev: Event) {
    ev.stopPropagation();
    if (!(await confirmModal($t("builder.confirmDelete")))) return;
    await api.deleteAgent(id);
    if (editingId === id) newAgent();
    await refreshAgents();
    toast("Agent supprimé", "success");
  }

  // ── Live run ───────────────────────────────────────────────────────────────
  let runJobId = $state<string | null>(null);
  let runStatus = $state("");
  let stepStatus = $state<Record<string, string>>({});
  let runLogs = $state<string[]>([]);
  let cleanup: (() => void) | null = null;
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  function resetRun() {
    runJobId = null; runStatus = ""; stepStatus = {}; runLogs = [];
    if (cleanup) cleanup(); cleanup = null;
    if (pollTimer) clearInterval(pollTimer); pollTimer = null;
  }

  function stepNodeId(stepKey: string): string | undefined {
    return nodes.find((n) => blockKey(n.id) === stepKey && n.data.kind === "step")?.id;
  }

  async function start() {
    const id = await save();
    if (!id) return;
    resetRun();
    const res = await api.createObjective(id, `${name} run`, goals.trim() || `Run ${name}`);
    runJobId = res.job_id; runStatus = "queued";
    cleanup = subscribeJob(runJobId, (ev) => {
      const k = ev.kind as string, step = ev.step_type as string | undefined;
      if ((k === "step_started" || k === "step_completed") && step) {
        const nid = stepNodeId(step);
        if (nid) stepStatus = { ...stepStatus, [nid]: k === "step_completed" ? "done" : "running" };
      }
      if (k === "job_status") runStatus = (ev.status as string) ?? runStatus;
      if (ev.message) runLogs = [...runLogs, ev.message as string].slice(-40);
    });
    pollTimer = setInterval(pollRun, 2500); pollRun();
  }

  async function pollRun() {
    if (!runJobId) return;
    try {
      const d = await api.getJob(runJobId);
      runStatus = d.job.status;
      const ss: Record<string, string> = {};
      for (const s of d.steps) { const nid = stepNodeId(s.step_type); if (nid) ss[nid] = s.status === "done" ? "done" : "running"; }
      stepStatus = ss;
      if (["done", "failed"].includes(d.job.status) && pollTimer) { clearInterval(pollTimer); pollTimer = null; }
    } catch { /* */ }
  }

  function stop() {
    if (cleanup) cleanup(); cleanup = null;
    if (pollTimer) clearInterval(pollTimer); pollTimer = null;
    runStatus = "stopped";
  }
  onDestroy(() => { if (cleanup) cleanup(); if (pollTimer) clearInterval(pollTimer); });

  // Agent image upload -> data URL.
  let agentsOpen = $state(true);
  // Collapsible side panels (toolbox + properties) and the per-node edit modal.
  let paletteOpen = $state(true);
  let inspectorOpen = $state(true);
  let modalOpen = $state(false);
  function onIconFile(e: Event) {
    const f = (e.target as HTMLInputElement).files?.[0];
    if (!f) return;
    const img = new Image();
    const reader = new FileReader();
    reader.onload = () => {
      img.onload = () => {
        const c = document.createElement("canvas");
        const s = Math.min(1, 96 / Math.max(img.width, img.height));
        c.width = img.width * s; c.height = img.height * s;
        c.getContext("2d")!.drawImage(img, 0, 0, c.width, c.height);
        icon = c.toDataURL("image/png");
      };
      img.src = reader.result as string;
    };
    reader.readAsDataURL(f);
  }
</script>

<div class="dash" style="grid-template-columns: {paletteOpen ? '200px' : '36px'} 1fr {inspectorOpen ? '300px' : '36px'};">
  <!-- Palette (tools window) -->
  {#if paletteOpen}
    <aside class="palette card">
      <div class="phead"><h3>{$t("builder.toolbox")}</h3><button class="collapse" onclick={() => (paletteOpen = false)} title="Réduire">«</button></div>
      <p class="hint">{$t("builder.toolboxHint")}</p>
      {#each PALETTE as g}
        <div class="pgroup">{g.group}</div>
        {#each g.items as b}
          <button class="pblock {b.kind}" draggable="true"
            ondragstart={(e) => e.dataTransfer?.setData("text/plain", b.key)}
            onclick={() => addBlock(b.key)}>
            <span class="pg">{b.glyph}</span> {b.label}
          </button>
        {/each}
      {/each}
    </aside>
  {:else}
    <aside class="railbar card"><button class="collapse" onclick={() => (paletteOpen = true)} title={$t("builder.toolbox")}>»</button></aside>
  {/if}

  <!-- Canvas -->
  <section class="center">
    <div class="topbar card">
      <button class="ptoggle" onclick={() => (agentsOpen = !agentsOpen)} title={$t("builder.myAgents")}>☰</button>
      <span class="aname">{name}</span>
      <div class="runctl">
        {#if busy || runStatus === "queued" || runStatus === "running"}<span class="spinner"></span>{/if}
        {#if runStatus}<span class="badge {runStatus}">{runStatus}</span>{/if}
        <button class="start" onclick={start}>▶ {$t("builder.start")}</button>
        <button class="stop" onclick={stop} disabled={!runJobId}>■ {$t("builder.stop")}</button>
        <button class="save" onclick={save}>{$t("builder.save")}</button>
      </div>
    </div>
    <div class="flowwrap card" bind:this={flowEl} ondragover={(e) => e.preventDefault()} ondrop={onDrop} oncontextmenu={onContextMenu}>
      <SvelteFlow bind:nodes bind:edges {nodeTypes} fitView onnodeclick={onNodeClick} onnodecontextmenu={onNodeContextMenu}>
        <Background gap={22} />
        <Controls />
        <MiniMap pannable zoomable />
      </SvelteFlow>
    </div>
    {#if runLogs.length}
      <div class="logfeed card">{#each runLogs.slice(-5) as l}<div class="lf">▸ {l}</div>{/each}</div>
    {/if}
  </section>

  <!-- Inspector -->
  {#if !inspectorOpen}
    <aside class="railbar card"><button class="collapse" onclick={() => (inspectorOpen = true)} title="Propriétés">«</button></aside>
  {:else}
  <aside class="inspector card">
    <div class="phead insp-top"><strong>{$t("builder.properties")}</strong><button class="collapse" onclick={() => (inspectorOpen = false)} title="Réduire">»</button></div>
    {#if agentsOpen}
      <div class="phead"><strong>{$t("builder.myAgents")}</strong><button class="new" onclick={newAgent}>＋</button></div>
      <div class="alist">
        {#each agentList as a}
          <div class="arow" class:on={editingId === a.id}>
            <button class="arow-main" onclick={() => loadAgent(a.id)}>
              <span class="av">{#if isImg(agentEmoji(a))}<img class="avimg" src={agentEmoji(a)} alt="" />{:else}{agentEmoji(a)}{/if}</span>
              <span class="anm">{a.name}</span>
            </button>
            <button class="del" onclick={(e) => deleteAgent(a.id, e)} title={$t("builder.delete")}>🗑</button>
          </div>
        {/each}
        {#if agentList.length === 0}<p class="hint">{$t("agents.empty")}</p>{/if}
      </div>
      <hr />
    {/if}

    {#if selected === "agent"}
      <h3>{$t("builder.general")}</h3>
      <div class="iconrow">
        <span class="bigicon">{#if isImg(icon)}<img class="avimg2" src={icon} alt="" />{:else}{icon || "🐙"}{/if}</span>
        <label class="upl">{$t("builder.upload")}<input type="file" accept="image/*" onchange={onIconFile} /></label>
      </div>
      <div class="iconpick">{#each ["🐙","📈","🧾","🌦️","🛰️","✉️","🤖","🔎","📊","🛒"] as ic}<button class="ic" class:on={icon===ic} onclick={() => (icon=ic)}>{ic}</button>{/each}</div>
      <label class="blk">{$t("builder.name")}<input bind:value={name} /></label>
      <label class="blk">{$t("builder.descr")}<textarea rows="2" bind:value={description}></textarea></label>
      <label class="blk">{$t("builder.autonomy")}
        <select bind:value={autonomy}><option value="confirm_before_action">{$t("builder.confirm")}</option><option value="full_auto">{$t("builder.fullAuto")}</option></select>
      </label>
      {#if !hasTriggerNode}
        <label class="blk">{$t("builder.triggerOn")}<input bind:value={triggerOn} placeholder="invoice.received / email.received / webhook.received" /></label>
      {/if}
      <label class="blk">{$t("builder.loopEvery")}<input type="number" min="0" bind:value={loopMinutes} /></label>
      <label class="blk">{$t("builder.goals")}<input bind:value={goals} /></label>
    {:else if selNode}
      {@const kind = selNode.data.kind}
      <div class="phead"><h3>{#if isImg(selNode.data.glyph)}<img class="hicon" src={selNode.data.glyph} alt="" />{:else}{selNode.data.glyph}{/if} {selNode.data.label}</h3><button class="del" onclick={() => removeNode(selNode.id)}>🗑</button></div>
      {#if kind === "step"}
        <label class="blk">{$t("builder.systemPrompt")}<textarea rows="6" bind:value={prompts[selected]} placeholder={$t("builder.promptPlaceholder")}></textarea></label>
      {:else if kind === "tool"}
        <p class="hint">Outil exécuté à l'étape Action.</p>
        {#each PARAM_KEYS[blockKey(selected)] ?? [] as pk}
          <label class="blk">{PARAM_LABEL[pk] ?? pk}<input value={params[selected]?.[pk] ?? ""}
            oninput={(e) => params = { ...params, [selected]: { ...(params[selected]||{}), [pk]: (e.target as HTMLInputElement).value } }}
            placeholder={PARAM_PH[pk] ?? ""} /></label>
        {/each}
      {:else if kind === "control"}
        <p class="hint">Bloc logique (visuel) — exécution avancée à venir.</p>
      {/if}
    {/if}
  </aside>
  {/if}

  <!-- Per-node properties modal (modify / remove on click) -->
  {#if modalOpen && selNode && selected !== "agent"}
    {@const mkind = selNode.data.kind}
    {@const isVideoTool = mkind === "tool" && blockKey(selected) === "analyse_video"}
    <div class="overlay" onclick={() => (modalOpen = false)} role="presentation">
      <div class="nmodal" class:wide={isVideoTool} onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true">
        <div class="nmhead">
          <h3>{#if isImg(selNode.data.glyph)}<img class="hicon" src={selNode.data.glyph} alt="" />{:else}{selNode.data.glyph}{/if} {selNode.data.label}</h3>
          <button class="x" onclick={() => (modalOpen = false)} aria-label="close">×</button>
        </div>
        <div class="nmbody">
          {#if mkind === "step"}
            <label class="blk">{$t("builder.systemPrompt")}<textarea rows="6" bind:value={prompts[selected]} placeholder={$t("builder.promptPlaceholder")}></textarea></label>
          {:else if mkind === "trigger"}
            <p class="hint">Déclencheur branché après l'agent.</p>
            {#each PARAM_KEYS[blockKey(selected)] ?? [] as pk}
              <label class="blk">{PARAM_LABEL[pk] ?? pk}<input value={params[selected]?.[pk] ?? ""}
                oninput={(e) => params = { ...params, [selected]: { ...(params[selected]||{}), [pk]: (e.target as HTMLInputElement).value } }}
                placeholder={PARAM_PH[pk] ?? ""} /></label>
            {/each}
          {:else if isVideoTool}
            <p class="hint">Uploade ou enregistre une vidéo : elle est échantillonnée puis analysée par l'agent.</p>
            <VideoView />
          {:else if mkind === "tool"}
            <p class="hint">Outil exécuté à l'étape Action.</p>
            {#each PARAM_KEYS[blockKey(selected)] ?? [] as pk}
              <label class="blk">{PARAM_LABEL[pk] ?? pk}<input value={params[selected]?.[pk] ?? ""}
                oninput={(e) => params = { ...params, [selected]: { ...(params[selected]||{}), [pk]: (e.target as HTMLInputElement).value } }}
                placeholder={PARAM_PH[pk] ?? ""} /></label>
            {/each}
            {#if (PARAM_KEYS[blockKey(selected)] ?? []).length === 0}<p class="hint">Aucune propriété pour cet outil.</p>{/if}
          {:else if mkind === "control"}
            <p class="hint">Bloc logique (visuel) — exécution avancée à venir.</p>
          {/if}
        </div>
        <div class="nmfoot">
          <button class="danger" onclick={() => { removeNode(selNode.id); modalOpen = false; }}>🗑 Supprimer</button>
          <button class="ghost" onclick={() => (modalOpen = false)}>Fermer</button>
        </div>
      </div>
    </div>
  {/if}

  <!-- Canvas right-click context menu to add a block -->
  {#if ctxMenu}
    <div class="ctxback" onclick={() => (ctxMenu = null)} oncontextmenu={(e) => { e.preventDefault(); ctxMenu = null; }} role="presentation"></div>
    <div class="ctxmenu" style="left: {ctxMenu.sx}px; top: {ctxMenu.sy}px;">
      {#if ctxMenu.nodeId}
        <button class="ctxitem" onclick={editFromMenu}>✏️ Modifier</button>
        {#if ctxMenu.nodeId !== "agent"}<button class="ctxitem danger" onclick={deleteFromMenu}>🗑 Supprimer</button>{/if}
      {:else}
        <div class="ctxgroup">＋ Nouveau bloc</div>
        {#each PALETTE as g}
          <div class="ctxgroup">{g.group}</div>
          {#each g.items as b}
            <button class="ctxitem" onclick={() => addFromMenu(b.key)}><span class="pg">{b.glyph}</span> {b.label}</button>
          {/each}
        {/each}
      {/if}
    </div>
  {/if}
</div>

<style>
  .dash { position: relative; height: calc(100vh - 90px); width: 100%; display: grid; grid-template-columns: 200px 1fr 300px; gap: 0.7rem; padding: 0.7rem; box-sizing: border-box; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: 12px; }
  .palette { padding: 0.7rem; overflow-y: auto; }
  .palette h3 { margin: 0 0 0.2rem; font-size: 0.95rem; }
  .hint { color: var(--muted); font-size: 0.75rem; margin: 0 0 0.6rem; }
  .pgroup { color: var(--muted); font-size: 0.7rem; text-transform: uppercase; letter-spacing: 0.05em; margin: 0.6rem 0 0.3rem; }
  .pblock { display: flex; align-items: center; gap: 0.45rem; width: 100%; text-align: left; background: var(--bg); border: 1px solid var(--border); border-radius: 9px; padding: 0.4rem 0.55rem; margin-bottom: 0.3rem; cursor: grab; color: var(--text); font: inherit; font-size: 0.8rem; }
  .pblock:hover { border-color: var(--accent); }
  .pblock:active { cursor: grabbing; }
  .pblock.tool { border-left: 3px solid var(--ok); }
  .pblock.control { border-left: 3px solid var(--warn); }
  .pblock.step { border-left: 3px solid var(--accent); }
  .pg { font-size: 1rem; }
  .center { display: flex; flex-direction: column; gap: 0.6rem; min-width: 0; }
  .topbar { display: flex; align-items: center; gap: 0.6rem; padding: 0.45rem 0.7rem; }
  .ptoggle { background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.3rem 0.5rem; cursor: pointer; }
  .aname { font-weight: 600; flex: 1; }
  .runctl { display: flex; align-items: center; gap: 0.4rem; }
  .runctl button { border-radius: 8px; padding: 0.4rem 0.7rem; cursor: pointer; font: inherit; font-size: 0.82rem; border: 1px solid var(--border); background: var(--bg); color: var(--text); }
  .start { background: var(--ok) !important; border-color: var(--ok) !important; color: #04231a !important; font-weight: 600; }
  .stop { background: var(--err) !important; border-color: var(--err) !important; color: #2a0707 !important; }
  .stop:disabled { opacity: 0.5; }
  .save { background: var(--accent) !important; border-color: var(--accent) !important; color: #04231a !important; }
  .spinner { width: 15px; height: 15px; border: 2px solid var(--border); border-top-color: var(--accent); border-radius: 50%; animation: spin 0.7s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  .badge { font-size: 0.7rem; padding: 0.15rem 0.5rem; border-radius: 20px; background: var(--border); }
  .badge.running, .badge.queued { background: var(--warn); color: #2a2410; }
  .badge.done { background: var(--ok); color: #04231a; }
  .badge.failed { background: var(--err); color: #2a0707; }
  .flowwrap { flex: 1; overflow: hidden; }
  .logfeed { padding: 0.4rem 0.7rem; font-family: ui-monospace, monospace; font-size: 0.74rem; max-height: 90px; overflow-y: auto; }
  .lf { color: var(--muted); }
  .inspector { padding: 0.8rem; overflow-y: auto; }
  .inspector h3 { margin: 0 0 0.5rem; font-size: 0.95rem; }
  .phead { display: flex; justify-content: space-between; align-items: center; }
  .insp-top { margin-bottom: 0.5rem; }
  .collapse { background: var(--bg); border: 1px solid var(--border); color: var(--muted); border-radius: 7px; width: 24px; height: 24px; cursor: pointer; font-size: 0.9rem; line-height: 1; }
  .collapse:hover { color: var(--text); border-color: var(--accent); }
  .railbar { display: flex; align-items: flex-start; justify-content: center; padding: 0.5rem 0; }
  .new { width: 24px; height: 24px; border-radius: 7px; border: 1px solid var(--accent); background: color-mix(in srgb, var(--accent) 16%, transparent); color: var(--accent); cursor: pointer; }
  /* Per-node properties modal */
  .overlay { position: fixed; inset: 0; background: rgba(0,0,0,0.5); z-index: 1100; display: flex; align-items: center; justify-content: center; }
  .nmodal { background: var(--panel); border: 1px solid var(--border); border-radius: 14px; width: 380px; max-width: 92vw; box-shadow: 0 20px 60px rgba(0,0,0,0.5); overflow: hidden; }
  .nmodal.wide { width: 760px; }
  .nmodal.wide .nmbody { max-height: 72vh; overflow-y: auto; }
  .nmhead { display: flex; align-items: center; justify-content: space-between; padding: 0.8rem 1rem; border-bottom: 1px solid var(--border); }
  .nmhead h3 { margin: 0; font-size: 0.98rem; display: flex; align-items: center; gap: 0.4rem; }
  .nmhead .x { background: none; border: none; color: var(--muted); cursor: pointer; font-size: 1.3rem; line-height: 1; }
  .nmbody { padding: 1rem; }
  .nmfoot { display: flex; justify-content: space-between; gap: 0.6rem; padding: 0.8rem 1rem; border-top: 1px solid var(--border); }
  .nmfoot button { border-radius: 8px; padding: 0.5rem 0.9rem; cursor: pointer; font: inherit; border: 1px solid var(--border); background: var(--bg); color: var(--text); }
  .nmfoot .danger { background: var(--err); border-color: var(--err); color: #2a0707; font-weight: 600; }
  /* Canvas context menu */
  .ctxback { position: fixed; inset: 0; z-index: 1090; }
  .ctxmenu { position: fixed; z-index: 1091; background: var(--panel); border: 1px solid var(--border); border-radius: 10px; padding: 0.35rem; box-shadow: 0 12px 40px rgba(0,0,0,0.5); max-height: 70vh; overflow-y: auto; width: 200px; }
  .ctxgroup { color: var(--muted); font-size: 0.66rem; text-transform: uppercase; letter-spacing: 0.05em; margin: 0.4rem 0.4rem 0.2rem; }
  .ctxitem { display: flex; align-items: center; gap: 0.45rem; width: 100%; text-align: left; background: none; border: none; color: var(--text); font: inherit; font-size: 0.8rem; padding: 0.35rem 0.45rem; border-radius: 7px; cursor: pointer; }
  .ctxitem:hover { background: color-mix(in srgb, var(--accent) 18%, transparent); }
  .ctxitem.danger { color: var(--err); }
  .ctxitem.danger:hover { background: color-mix(in srgb, var(--err) 18%, transparent); }
  .alist { display: flex; flex-direction: column; gap: 0.2rem; margin-top: 0.4rem; }
  .arow { display: flex; align-items: center; }
  .arow.on { background: color-mix(in srgb, var(--accent) 14%, transparent); border-radius: 8px; }
  .arow-main { flex: 1; display: flex; align-items: center; gap: 0.45rem; background: none; border: none; color: var(--text); font: inherit; cursor: pointer; padding: 0.35rem 0.4rem; text-align: left; }
  .av { font-size: 1.1rem; }
  .avimg { width: 20px; height: 20px; border-radius: 50%; object-fit: cover; }
  .avimg2 { width: 40px; height: 40px; border-radius: 50%; object-fit: cover; }
  .anm { font-size: 0.84rem; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .del { background: none; border: none; cursor: pointer; opacity: 0.55; padding: 0.25rem; }
  .del:hover { opacity: 1; }
  hr { border: none; border-top: 1px solid var(--border); margin: 0.8rem 0; }
  .iconrow { display: flex; align-items: center; gap: 0.6rem; margin-bottom: 0.4rem; }
  .bigicon { font-size: 2rem; }
  .hicon { width: 24px; height: 24px; border-radius: 6px; object-fit: cover; vertical-align: middle; }
  .upl { font-size: 0.78rem; color: var(--muted); cursor: pointer; }
  .upl input { display: block; margin-top: 0.2rem; font-size: 0.72rem; }
  .iconpick { display: flex; gap: 0.25rem; flex-wrap: wrap; margin-bottom: 0.5rem; }
  .ic { background: var(--bg); border: 1px solid var(--border); border-radius: 8px; padding: 0.15rem 0.35rem; cursor: pointer; font-size: 1.15rem; }
  .ic.on { border-color: var(--accent); }
  .blk { display: block; font-size: 0.8rem; color: var(--muted); margin-top: 0.5rem; }
  input, select, textarea { width: 100%; background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.45rem 0.6rem; font: inherit; margin-top: 0.2rem; }
  .blk textarea { font-family: ui-monospace, monospace; font-size: 0.82rem; }
  :global(.svelte-flow__attribution) { display: none; }
</style>
