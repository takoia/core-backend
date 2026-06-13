<script lang="ts">
  import { onMount, onDestroy, untrack } from "svelte";
  import {
    SvelteFlow,
    Background,
    Controls,
    MiniMap,
    type Node,
    type Edge,
  } from "@xyflow/svelte";
  import "@xyflow/svelte/dist/style.css";
  import StepNode from "./builder/StepNode.svelte";
  import Icon from "./Icon.svelte";
  import { api, subscribeJob, type Agent } from "./api";
  import { t } from "./i18n";
  import { toast, confirmModal } from "./toast";

  const nodeTypes = { step: StepNode } as any;
  const STEPS = ["analyse", "decision", "action", "restitution"] as const;
  type StepKey = (typeof STEPS)[number];
  const TOOLS = ["web_search", "market_data", "send_discord", "write_report", "extract_fields"];
  // Tools that need parameters -> {tool: [param keys]}.
  const TOOL_PARAMS: Record<string, string[]> = {
    market_data: ["symbol"],
    send_discord: ["discord_webhook"],
  };
  const PARAM_PLACEHOLDER: Record<string, string> = {
    symbol: "^IXIC (NASDAQ), AAPL, NVDA…",
    discord_webhook: "https://discord.com/api/webhooks/…",
  };
  const STEP_LABEL: Record<string, string> = {
    analyse: "Analyse", decision: "Décision", action: "Action", restitution: "Restitution",
  };

  // ── Agent state ──
  let name = $state("New agent");
  let author = $state("You");
  let expertise = $state("");
  let autonomy = $state<"full_auto" | "confirm_before_action">("full_auto");
  let triggerOn = $state("");
  let emit = $state("");
  let loopMinutes = $state(0);
  let goals = $state("");
  let checks = $state("");
  let description = $state("");
  let icon = $state("");
  const ICON_CHOICES = ["🐙", "📈", "🧾", "🌦️", "🛰️", "✉️", "🤖", "🔎", "📊", "🛒", "💬", "🧠"];
  const TRIGGER_TYPES = [
    { icon: "🖐️", label: "Manuel", event: "" },
    { icon: "📨", label: "Email reçu", event: "email.received" },
    { icon: "🔗", label: "Webhook", event: "webhook.received" },
    { icon: "🧾", label: "Facture", event: "invoice.received" },
    { icon: "📁", label: "Fichier FTP", event: "ftp.file" },
    { icon: "⏰", label: "Chaque jour", event: "schedule.daily" },
    { icon: "⏱️", label: "Chaque heure", event: "schedule.hourly" },
  ];
  let prompts = $state<Record<StepKey, string>>({ analyse: "", decision: "", action: "", restitution: "" });
  let tools = $state<string[]>([]);
  let toolParams = $state<Record<string, string>>({ symbol: "^IXIC", discord_webhook: "" });
  let selected = $state<string | null>("analyse");
  let createdMsg = $state("");
  // Floating overlay panels (canvas is full-screen underneath).
  let agentsOpen = $state(true);
  let inspectorOpen = $state(false);
  let busy = $state(false);

  // Param fields to show for the currently chosen action tools.
  const activeParams = $derived(
    Array.from(new Set(tools.flatMap((t) => TOOL_PARAMS[t] ?? []))),
  );

  // ── Agent dashboard list ──
  let agentList = $state<Agent[]>([]);
  let editingId = $state<string | null>(null);

  async function refreshAgents() {
    try { agentList = await api.listAgents(); } catch { agentList = []; }
  }
  onMount(refreshAgents);

  function newAgent() {
    editingId = null;
    name = "New agent"; author = "You"; expertise = ""; autonomy = "confirm_before_action";
    triggerOn = ""; emit = ""; loopMinutes = 0; goals = ""; checks = "";
    description = ""; icon = "🐙";
    prompts = { analyse: "", decision: "", action: "", restitution: "" };
    tools = []; selected = "analyse"; resetRun();
  }

  async function deleteAgent(id: string, ev: Event) {
    ev.stopPropagation();
    if (!(await confirmModal($t("builder.confirmDelete")))) return;
    await api.deleteAgent(id);
    if (editingId === id) newAgent();
    await refreshAgents();
    toast(`${id} ${$t("builder.delete").toLowerCase()}`, "success");
  }

  async function loadAgent(id: string) {
    const d = await api.getAgent(id);
    editingId = d.agent.id;
    name = d.agent.name; author = d.agent.author ?? ""; expertise = d.agent.expertise_domain ?? "";
    description = d.agent.description ?? ""; icon = d.agent.icon || "";
    autonomy = d.agent.autonomy_level === "full_auto" ? "full_auto" : "confirm_before_action";
    triggerOn = (d.agent as any).trigger_on ?? "";
    try { emit = (JSON.parse((d.agent as any).emit ?? "[]") as string[]).join(", "); } catch { emit = ""; }
    const np: Record<StepKey, string> = { analyse: "", decision: "", action: "", restitution: "" };
    let nt: string[] = [];
    for (const sc of d.steps) {
      if ((STEPS as readonly string[]).includes(sc.step_type)) np[sc.step_type as StepKey] = sc.system_prompt;
      try {
        const o = JSON.parse(sc.options || "{}");
        if (sc.step_type === "action" && Array.isArray(o.allowed_tools)) nt = o.allowed_tools;
        if (sc.step_type === "action" && o.tool_params) toolParams = { ...toolParams, ...o.tool_params };
      } catch { /* */ }
    }
    prompts = np; tools = nt; selected = "analyse"; resetRun();
  }

  // ── Svelte Flow graph ──
  const X = 80;
  let nodes = $state.raw<Node[]>([
    { id: "trigger", type: "step", position: { x: X, y: 0 }, data: { label: "Trigger", kind: "trigger" } },
    { id: "analyse", type: "step", position: { x: X, y: 150 }, data: { label: "Analyse", kind: "step", idx: 1 } },
    { id: "decision", type: "step", position: { x: X, y: 300 }, data: { label: "Décision", kind: "step", idx: 2 } },
    { id: "action", type: "step", position: { x: X, y: 450 }, data: { label: "Action", kind: "step", idx: 3 } },
    { id: "restitution", type: "step", position: { x: X, y: 600 }, data: { label: "Restitution", kind: "step", idx: 4 } },
    { id: "emit", type: "step", position: { x: X, y: 750 }, data: { label: "Emit", kind: "emit" } },
  ]);
  let edges = $state.raw<Edge[]>([
    { id: "e1", source: "trigger", target: "analyse", animated: true },
    { id: "e2", source: "analyse", target: "decision", animated: true },
    { id: "e3", source: "decision", target: "action", animated: true },
    { id: "e4", source: "action", target: "restitution", animated: true },
    { id: "e5", source: "restitution", target: "emit", animated: true },
  ]);

  // Sync editable fields + live status onto the nodes. Read `nodes` via
  // untrack() so writing it back does not re-trigger this effect (which would
  // loop forever and crash with effect_update_depth_exceeded).
  $effect(() => {
    const tr = triggerOn, em = emit, tl = tools, ss = stepStatus;
    nodes = untrack(() => nodes).map((n) => {
      if (n.id === "trigger") return { ...n, data: { ...n.data, sub: tr || "manual" } };
      if (n.id === "emit") return { ...n, data: { ...n.data, sub: em || "—" } };
      const base: any = { ...n.data };
      if (n.id === "action") base.tools = [...tl];
      if ((STEPS as readonly string[]).includes(n.id)) base.status = ss[n.id] ?? "pending";
      return { ...n, data: base };
    });
  });

  function onNodeClick(e: any) {
    const id = e?.node?.id ?? e?.detail?.node?.id;
    if (id) {
      selected = id;
      inspectorOpen = true;
    }
  }
  function addTool(tool: string) { if (!tools.includes(tool)) tools = [...tools, tool]; }
  function removeTool(tool: string) { tools = tools.filter((x) => x !== tool); }
  function onPaletteDrop(e: DragEvent) {
    e.preventDefault();
    const tool = e.dataTransfer?.getData("text/plain");
    if (tool) { addTool(tool); selected = "action"; }
  }

  const slug = (s: string) => s.toLowerCase().trim().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "agent";

  function buildToml(): string {
    const emitArr = emit.split(",").map((e) => e.trim()).filter(Boolean);
    const desc = (description || goals || "Built in TakoIA.").replace(/\n/g, " ");
    let toml = `[agent]\nid = "${editingId ?? slug(name)}"\nname = "${name}"\nauthor = "${author}"\nversion = "0.1.0"\n`;
    toml += `description = ${JSON.stringify(desc)}\nexpertise = "${expertise}"\nautonomy = "${autonomy}"\nicon = "${icon}"\n`;
    toml += `emit = [${emitArr.map((e) => `"${e}"`).join(", ")}]\n`;
    if (triggerOn.trim()) toml += `\n[trigger]\non = "${triggerOn.trim()}"\n`;
    for (const s of STEPS) {
      const p = prompts[s].trim();
      const isAction = s === "action";
      if (!p && !(isAction && tools.length)) continue;
      toml += `\n[steps.${s}]\n`;
      if (p) toml += `system_prompt = ${JSON.stringify(p)}\n`;
      if (isAction && tools.length) toml += `allowed_tools = [${tools.map((x) => `"${x}"`).join(", ")}]\n`;
      if (isAction && activeParams.length) {
        const pairs = activeParams
          .filter((k) => (toolParams[k] ?? "").trim())
          .map((k) => `${k} = ${JSON.stringify(toolParams[k])}`);
        if (pairs.length) toml += `tool_params = { ${pairs.join(", ")} }\n`;
      }
    }
    return toml;
  }

  async function save() {
    createdMsg = "";
    busy = true;
    try {
    const r = await api.importToml(buildToml());
    editingId = r.id;
    if (loopMinutes > 0) {
      const checkLine = checks.trim() ? `\n\nVerify before finishing:\n${checks.trim()}` : "";
      await fetch("/api/schedules", { method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ agent_id: r.id, title: `${name} loop`, prompt: (goals.trim() || `Run ${name}`) + checkLine, interval_seconds: loopMinutes * 60 }) });
    }
    toast($t("builder.created"), "success");
    await refreshAgents();
    return r.id;
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "error");
    } finally { busy = false; }
  }

  // ── Live run ──
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

  async function start() {
    const id = await save();
    if (!id) return;
    resetRun();
    const res = await api.createObjective(id, `${name} run`, goals.trim() || `Run ${name}`);
    runJobId = res.job_id; runStatus = "queued";
    cleanup = subscribeJob(runJobId, (ev) => {
      const k = ev.kind as string, step = ev.step_type as string | undefined;
      if (k === "step_started" && step) stepStatus = { ...stepStatus, [step]: "running" };
      if (k === "step_completed" && step) stepStatus = { ...stepStatus, [step]: "done" };
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
      for (const s of d.steps) ss[s.step_type] = s.status === "done" ? "done" : "running";
      if (d.job.status === "running") for (const k of STEPS) { if (ss[k] !== "done") { ss[k] = "running"; break; } }
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

  const isStep = $derived(selected !== null && (STEPS as readonly string[]).includes(selected));
  const isImg = (s: string) => /^(https?:|data:)/.test(s || "");
  function emoji(a: Agent): string {
    if (a.icon) return a.icon;
    const e = (a.expertise_domain || "").toLowerCase();
    if (e.includes("trad")) return "📈"; if (e.includes("invoice") || e.includes("fact")) return "🧾";
    if (e.includes("meteo") || e.includes("weather")) return "🌦️"; if (e.includes("watch") || e.includes("veille")) return "🛰️";
    return "🐙";
  }
</script>

<div class="dash">
  <!-- Full-screen canvas underneath everything -->
  <div class="flowwrap">
    <SvelteFlow bind:nodes bind:edges {nodeTypes} fitView onnodeclick={onNodeClick}>
      <Background gap={22} />
      <Controls />
      <MiniMap pannable zoomable />
    </SvelteFlow>
  </div>

  <!-- Floating top bar -->
  <div class="topbar card">
    <button class="ptoggle" onclick={() => (agentsOpen = !agentsOpen)} title={$t("builder.myAgents")}>☰</button>
    <button class="iconbtn" onclick={() => { selected = "general"; inspectorOpen = true; }} title={$t("builder.general")}>
      {#if isImg(icon)}<img class="avimg" src={icon} alt="" />{:else}{icon || "🐙"}{/if}
    </button>
    <input class="agentname" bind:value={name} />
    <button class="gear" onclick={() => { selected = "general"; inspectorOpen = true; }} title={$t("builder.general")}>⚙</button>
    <div class="runctl">
      {#if busy || runStatus === "queued" || runStatus === "running"}<span class="spinner"></span>{/if}
      {#if runStatus}<span class="badge {runStatus}">{runStatus}</span>{/if}
      <button class="start" onclick={start}><Icon name="run" size={14} /> {$t("builder.start")}</button>
      <button class="stop" onclick={stop} disabled={!runJobId}>■ {$t("builder.stop")}</button>
      <button class="save" onclick={save}>{$t("builder.save")}</button>
    </div>
  </div>

  <!-- Floating agents panel -->
  {#if agentsOpen}
    <aside class="panel agents card">
      <div class="phead">
        <strong>{$t("builder.myAgents")}</strong>
        <span>
          <button class="new" onclick={newAgent} title={$t("builder.newAgent")}>＋</button>
          <button class="closep" onclick={() => (agentsOpen = false)}>×</button>
        </span>
      </div>
      <div class="alist">
        {#each agentList as a}
          <div class="arow" class:on={editingId === a.id}>
            <button class="arow-main" onclick={() => loadAgent(a.id)}>
              <span class="av">{#if isImg(emoji(a))}<img class="avimg" src={emoji(a)} alt="" />{:else}{emoji(a)}{/if}</span>
              <span class="an">
                <span class="anm">{a.name}</span>
                <span class="asub muted">{a.autonomy_level === "full_auto" ? "auto" : "validation"} · {a.runs_count} exéc.</span>
              </span>
            </button>
            <button class="del" onclick={(e) => deleteAgent(a.id, e)} title={$t("builder.delete")}>🗑</button>
          </div>
        {/each}
        {#if agentList.length === 0}<p class="muted small">{$t("agents.empty")}</p>{/if}
      </div>
    </aside>
  {/if}

  <!-- Floating log feed -->
  {#if runLogs.length}
    <div class="logfeed card">
      {#each runLogs.slice(-6) as l}<div class="lf">▸ {l}</div>{/each}
    </div>
  {/if}

  <!-- Floating inspector (opens on node click) -->
  {#if inspectorOpen}
  <aside class="panel inspector card">
    <button class="closep abs" onclick={() => (inspectorOpen = false)}>×</button>
    {#if isStep}
      {@const sk = selected as StepKey}
      <h3>{STEP_LABEL[sk]}</h3>
      <label class="blk">{$t("builder.systemPrompt")}
        <textarea rows="6" bind:value={prompts[sk]} placeholder={$t("builder.promptPlaceholder")}></textarea>
      </label>
      {#if sk === "action"}
        <div class="palette" ondragover={(e) => e.preventDefault()} ondrop={onPaletteDrop}>
          <span class="muted small">{$t("builder.palette")} — {$t("builder.paletteHint")}</span>
          <div class="chips">
            {#each TOOLS as tool}
              <button class="add" draggable="true"
                ondragstart={(e) => e.dataTransfer?.setData("text/plain", tool)}
                onclick={() => addTool(tool)} disabled={tools.includes(tool)}>⠿ {tool}</button>
            {/each}
          </div>
          <div class="chosen">
            {#each tools as tool}<span class="chip">{tool} <button class="x" onclick={() => removeTool(tool)}>×</button></span>{/each}
          </div>
        </div>
        {#if activeParams.length}
          <div class="params">
            <span class="muted small">{$t("builder.toolParams")}</span>
            {#each activeParams as p}
              <label class="blk">{p}<input bind:value={toolParams[p]} placeholder={PARAM_PLACEHOLDER[p] ?? ""} /></label>
            {/each}
          </div>
        {/if}
      {/if}
    {:else if selected === "trigger"}
      <h3>{$t("builder.node.trigger")}</h3>
      <p class="muted small">{$t("builder.triggerPick")}</p>
      <div class="trigtypes">
        {#each TRIGGER_TYPES as tt}
          <button class="ttype" class:on={triggerOn === tt.event} onclick={() => (triggerOn = tt.event)}>
            <span class="tic">{tt.icon}</span>
            <span class="tn">{tt.label}<span class="muted small"> · {tt.event}</span></span>
          </button>
        {/each}
      </div>
      <label class="blk">{$t("builder.triggerOn")}<input bind:value={triggerOn} placeholder="invoice.received" /></label>
    {:else if selected === "emit"}
      <h3>{$t("builder.node.emit")}</h3>
      <label class="blk">{$t("builder.emit")}<input bind:value={emit} placeholder="report.ready" /></label>
    {:else if selected === "general"}
      <h3>{$t("builder.general")}</h3>
      <label class="blk">{$t("builder.icon")}</label>
      <div class="iconpick">
        {#each ICON_CHOICES as ic}
          <button class="ic" class:on={icon === ic} onclick={() => (icon = ic)}>{ic}</button>
        {/each}
      </div>
      <label class="blk">{$t("builder.iconImage")}<input bind:value={icon} placeholder="https://…/logo.png" /></label>
      <label class="blk">{$t("builder.name")}<input bind:value={name} /></label>
      <label class="blk">{$t("builder.descr")}<textarea rows="3" bind:value={description} placeholder={$t("builder.descrPlaceholder")}></textarea></label>
      <label class="blk">{$t("builder.author")}<input bind:value={author} /></label>
      <label class="blk">{$t("builder.expertise")}<input bind:value={expertise} /></label>
      <label class="blk">{$t("builder.autonomy")}
        <select bind:value={autonomy}>
          <option value="confirm_before_action">{$t("builder.confirm")}</option>
          <option value="full_auto">{$t("builder.fullAuto")}</option>
        </select>
      </label>
      <label class="blk">{$t("builder.loopEvery")}<input type="number" min="0" bind:value={loopMinutes} /></label>
      <label class="blk">{$t("builder.goals")}<input bind:value={goals} placeholder={$t("builder.goalsPlaceholder")} /></label>
      <label class="blk">{$t("builder.checks")}<input bind:value={checks} placeholder={$t("builder.checksPlaceholder")} /></label>
    {/if}
    {#if createdMsg}<p class="muted small">{createdMsg}</p>{/if}
  </aside>
  {/if}
</div>

<style>
  /* Canvas fills the whole page; panels float on top. */
  .dash { position: relative; height: calc(100vh - 90px); width: 100%; }
  .card { background: var(--panel); border: 1px solid var(--border); border-radius: 12px; }
  .flowwrap { position: absolute; inset: 0; }

  .topbar { position: absolute; top: 12px; left: 50%; transform: translateX(-50%); z-index: 5;
    display: flex; align-items: center; gap: 0.6rem; padding: 0.4rem 0.6rem; box-shadow: 0 8px 24px rgba(0,0,0,0.35); }
  .ptoggle { background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.35rem 0.55rem; cursor: pointer; font-size: 1rem; }
  .agentname { background: transparent; border: none; color: var(--text); font: inherit; font-size: 1.05rem; font-weight: 600; width: 200px; }
  .runctl { display: flex; align-items: center; gap: 0.4rem; }
  .runctl button { display: inline-flex; align-items: center; gap: 0.3rem; border-radius: 8px; padding: 0.4rem 0.7rem; cursor: pointer; font: inherit; font-size: 0.82rem; border: 1px solid var(--border); background: var(--bg); color: var(--text); }
  .start { background: var(--ok) !important; border-color: var(--ok) !important; color: #04231a !important; font-weight: 600; }
  .stop { background: var(--err) !important; border-color: var(--err) !important; color: #2a0707 !important; }
  .stop:disabled { opacity: 0.5; }
  .save { background: var(--accent) !important; border-color: var(--accent) !important; color: #04231a !important; }
  .badge { font-size: 0.7rem; padding: 0.15rem 0.5rem; border-radius: 20px; background: var(--border); }
  .badge.running, .badge.queued { background: var(--warn); color: #2a2410; }
  .badge.done { background: var(--ok); color: #04231a; }
  .badge.failed { background: var(--err); color: #2a0707; }

  .panel { position: absolute; top: 12px; bottom: 12px; z-index: 6; width: 300px; padding: 0.9rem; overflow-y: auto; box-shadow: 0 8px 28px rgba(0,0,0,0.4); }
  .panel.agents { left: 12px; }
  .panel.inspector { right: 12px; }
  .phead { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.6rem; }
  .closep { background: transparent; border: none; color: var(--muted); cursor: pointer; font-size: 1.2rem; line-height: 1; }
  .closep.abs { position: absolute; top: 8px; right: 10px; }
  .new { width: 26px; height: 26px; border-radius: 8px; border: 1px solid var(--accent); background: color-mix(in srgb, var(--accent) 16%, transparent); color: var(--accent); cursor: pointer; font-size: 1rem; }
  .alist { display: flex; flex-direction: column; gap: 0.3rem; }
  .arow { display: flex; align-items: center; gap: 0.2rem; border-radius: 9px; }
  .arow.on { background: color-mix(in srgb, var(--accent) 16%, transparent); }
  .arow-main { flex: 1; display: flex; align-items: center; gap: 0.5rem; background: transparent; border: none; border-radius: 9px; padding: 0.45rem 0.5rem; cursor: pointer; text-align: left; color: var(--text); font: inherit; min-width: 0; }
  .arow-main:hover { background: var(--bg); }
  .del { background: transparent; border: none; cursor: pointer; opacity: 0.5; padding: 0.3rem; border-radius: 7px; }
  .del:hover { opacity: 1; background: color-mix(in srgb, var(--err) 18%, transparent); }
  .av { font-size: 1.2rem; }
  .an { display: flex; flex-direction: column; min-width: 0; }
  .anm { font-size: 0.86rem; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .asub { font-size: 0.72rem; }
  .logfeed { position: absolute; left: 50%; transform: translateX(-50%); bottom: 12px; z-index: 5; width: 420px; max-width: 50%; padding: 0.5rem 0.8rem; font-family: ui-monospace, monospace; font-size: 0.76rem; max-height: 130px; overflow-y: auto; }
  .lf { color: var(--muted); padding: 0.1rem 0; }
  .inspector h3 { margin: 0 0 0.6rem; }
  .blk { display: block; font-size: 0.8rem; color: var(--muted); margin-top: 0.6rem; }
  input, select, textarea { width: 100%; background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.45rem 0.6rem; font: inherit; margin-top: 0.2rem; }
  .blk textarea { font-family: ui-monospace, monospace; font-size: 0.82rem; }
  .palette { margin-top: 0.8rem; padding: 0.6rem; border: 1px dashed var(--border); border-radius: 10px; }
  .chips { display: flex; gap: 0.3rem; flex-wrap: wrap; margin-top: 0.4rem; }
  .add { background: var(--panel); border: 1px solid var(--border); color: var(--text); border-radius: 7px; padding: 0.25rem 0.5rem; cursor: grab; font: inherit; font-size: 0.74rem; }
  .add:disabled { opacity: 0.4; cursor: default; }
  .chosen { display: flex; gap: 0.35rem; flex-wrap: wrap; margin-top: 0.5rem; }
  .chip { background: color-mix(in srgb, var(--accent) 16%, transparent); color: var(--accent); border-radius: 20px; padding: 0.1rem 0.5rem; font-size: 0.78rem; }
  .chip .x { background: none; border: none; color: var(--accent); cursor: pointer; }
  .more { margin-top: 1rem; }
  .more summary { cursor: pointer; color: var(--muted); font-size: 0.82rem; }
  .small { font-size: 0.78rem; }
  :global(.svelte-flow__attribution) { display: none; }

  .iconbtn { background: var(--bg); border: 1px solid var(--border); border-radius: 8px; padding: 0.2rem 0.4rem; cursor: pointer; font-size: 1.15rem; }
  .gear { background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.35rem 0.5rem; cursor: pointer; font-size: 0.95rem; }
  .iconpick { display: flex; gap: 0.3rem; flex-wrap: wrap; margin: 0.2rem 0 0.5rem; }
  .ic { background: var(--bg); border: 1px solid var(--border); border-radius: 8px; padding: 0.2rem 0.4rem; cursor: pointer; font-size: 1.25rem; line-height: 1.4; }
  .ic.on { border-color: var(--accent); background: color-mix(in srgb, var(--accent) 18%, transparent); }
  .avimg { width: 1.2em; height: 1.2em; border-radius: 50%; object-fit: cover; vertical-align: middle; }
  .iconbtn .avimg, .av .avimg { width: 22px; height: 22px; }
  .trigtypes { display: flex; flex-direction: column; gap: 0.3rem; margin: 0.4rem 0 0.6rem; }
  .ttype { display: flex; align-items: center; gap: 0.5rem; background: var(--bg); border: 1px solid var(--border); border-radius: 9px; padding: 0.45rem 0.6rem; cursor: pointer; color: var(--text); font: inherit; text-align: left; }
  .ttype:hover { border-color: var(--accent); }
  .ttype.on { border-color: var(--accent); background: color-mix(in srgb, var(--accent) 14%, transparent); }
  .tic { font-size: 1.1rem; }
  .tn { display: flex; flex-direction: column; font-size: 0.82rem; }
  .spinner { width: 16px; height: 16px; border: 2px solid var(--border); border-top-color: var(--accent); border-radius: 50%; animation: spin 0.7s linear infinite; flex: none; }
  @keyframes spin { to { transform: rotate(360deg); } }
  /* Make the task palette stand out (Scratch-like) */
  .palette { border: 1.5px dashed var(--accent) !important; background: color-mix(in srgb, var(--accent) 7%, transparent); }
  .palette > span:first-child { display: block; color: var(--text) !important; font-weight: 700; font-size: 0.85rem; margin-bottom: 0.4rem; }
  .add { font-size: 0.8rem !important; padding: 0.35rem 0.6rem !important; }
</style>
