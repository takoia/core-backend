<script lang="ts">
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
  import { api } from "./api";
  import { t } from "./i18n";

  const nodeTypes = { step: StepNode } as any;

  // The four explicit contest steps + trigger/emit framing.
  const STEPS = ["analyse", "decision", "action", "restitution"] as const;
  type StepKey = (typeof STEPS)[number];
  const TOOLS = ["web_search", "write_report", "send_discord", "send_email", "extract_fields"];

  // ── Agent state ────────────────────────────────────────────────────────────
  let name = $state("Market Watcher");
  let author = $state("You");
  let expertise = $state("market intelligence");
  let autonomy = $state<"full_auto" | "confirm_before_action">("confirm_before_action");
  let triggerOn = $state("schedule.daily");
  let emit = $state("report.ready");

  // Loop / goals / checks (recurring autonomous run).
  let loopMinutes = $state(0);
  let goals = $state("");
  let checks = $state("");

  // Per-step system prompts + action tools.
  let prompts = $state<Record<StepKey, string>>({
    analyse: "",
    decision: "",
    action: "",
    restitution: "",
  });
  let tools = $state<string[]>(["web_search"]);
  let selected = $state<string | null>("analyse");
  let createdMsg = $state("");

  const STEP_LABEL: Record<string, string> = {
    analyse: "Analyse",
    decision: "Décision",
    action: "Action",
    restitution: "Restitution",
  };

  // ── Svelte Flow graph ──────────────────────────────────────────────────────
  const COL = 360;
  let nodes = $state.raw<Node[]>([
    { id: "trigger", type: "step", position: { x: COL, y: 0 }, data: { label: "Trigger", kind: "trigger", sub: triggerOn } },
    { id: "analyse", type: "step", position: { x: COL, y: 150 }, data: { label: STEP_LABEL.analyse, kind: "step", idx: 1 } },
    { id: "decision", type: "step", position: { x: COL, y: 300 }, data: { label: STEP_LABEL.decision, kind: "step", idx: 2 } },
    { id: "action", type: "step", position: { x: COL, y: 450 }, data: { label: STEP_LABEL.action, kind: "step", idx: 3, tools } },
    { id: "restitution", type: "step", position: { x: COL, y: 600 }, data: { label: STEP_LABEL.restitution, kind: "step", idx: 4 } },
    { id: "emit", type: "step", position: { x: COL, y: 750 }, data: { label: "Emit", kind: "emit", sub: emit } },
  ]);
  let edges = $state.raw<Edge[]>([
    { id: "e1", source: "trigger", target: "analyse", animated: true },
    { id: "e2", source: "analyse", target: "decision", animated: true },
    { id: "e3", source: "decision", target: "action", animated: true },
    { id: "e4", source: "action", target: "restitution", animated: true },
    { id: "e5", source: "restitution", target: "emit", animated: true },
  ]);

  // Keep the trigger/emit/action nodes in sync with the editable fields.
  $effect(() => {
    nodes = nodes.map((n) => {
      if (n.id === "trigger") return { ...n, data: { ...n.data, sub: triggerOn } };
      if (n.id === "emit") return { ...n, data: { ...n.data, sub: emit } };
      if (n.id === "action") return { ...n, data: { ...n.data, tools: [...tools] } };
      return n;
    });
  });

  function onNodeClick(e: any) {
    const id = e?.node?.id ?? e?.detail?.node?.id;
    if (id) selected = id;
  }

  function addTool(tool: string) {
    if (!tools.includes(tool)) tools = [...tools, tool];
  }
  function removeTool(tool: string) {
    tools = tools.filter((t) => t !== tool);
  }

  function slug(s: string) {
    return s.toLowerCase().trim().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "agent";
  }

  function buildToml(): string {
    const emitArr = emit.split(",").map((e) => e.trim()).filter(Boolean);
    let toml = `[agent]\n`;
    toml += `id = "${slug(name)}"\n`;
    toml += `name = "${name}"\n`;
    toml += `author = "${author}"\n`;
    toml += `version = "0.1.0"\n`;
    toml += `description = "${goals ? goals.replace(/\n/g, " ") : "Built visually in TakoIA."}"\n`;
    toml += `expertise = "${expertise}"\n`;
    toml += `autonomy = "${autonomy}"\n`;
    toml += `emit = [${emitArr.map((e) => `"${e}"`).join(", ")}]\n`;
    if (triggerOn.trim()) toml += `\n[trigger]\non = "${triggerOn.trim()}"\n`;
    for (const s of STEPS) {
      const p = prompts[s].trim();
      const isAction = s === "action";
      if (!p && !(isAction && tools.length)) continue;
      toml += `\n[steps.${s}]\n`;
      if (p) toml += `system_prompt = ${JSON.stringify(p)}\n`;
      if (isAction && tools.length) toml += `allowed_tools = [${tools.map((x) => `"${x}"`).join(", ")}]\n`;
    }
    return toml;
  }

  async function create() {
    createdMsg = "";
    try {
      const r = await api.importToml(buildToml());
      // If a loop interval is set, also create a recurring schedule.
      if (loopMinutes > 0) {
        const goal = goals.trim() || "Run the recurring objective.";
        const checkLine = checks.trim() ? `\n\nVerify before finishing:\n${checks.trim()}` : "";
        await fetch("/api/schedules", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            agent_id: r.id,
            title: `${name} loop`,
            prompt: goal + checkLine,
            interval_seconds: loopMinutes * 60,
          }),
        });
      }
      createdMsg = $t("builder.created") + ` (${r.id})`;
    } catch (e) {
      createdMsg = e instanceof Error ? e.message : String(e);
    }
  }

  const isStep = $derived(selected !== null && (STEPS as readonly string[]).includes(selected));
</script>

<div class="builder">
  <!-- Top: agent meta + loop -->
  <div class="card meta">
    <div class="grid">
      <label>{$t("builder.name")}<input bind:value={name} /></label>
      <label>{$t("builder.author")}<input bind:value={author} /></label>
      <label>{$t("builder.expertise")}<input bind:value={expertise} /></label>
      <label>{$t("builder.autonomy")}
        <select bind:value={autonomy}>
          <option value="confirm_before_action">{$t("builder.confirm")}</option>
          <option value="full_auto">{$t("builder.fullAuto")}</option>
        </select>
      </label>
      <label>{$t("builder.triggerOn")}<input bind:value={triggerOn} placeholder="invoice.received" /></label>
      <label>{$t("builder.emit")}<input bind:value={emit} placeholder="report.ready" /></label>
    </div>
    <div class="loop">
      <strong class="muted small">{$t("builder.loopTitle")}</strong>
      <div class="grid3">
        <label>{$t("builder.loopEvery")}<input type="number" min="0" bind:value={loopMinutes} /></label>
        <label>{$t("builder.goals")}<input bind:value={goals} placeholder={$t("builder.goalsPlaceholder")} /></label>
        <label>{$t("builder.checks")}<input bind:value={checks} placeholder={$t("builder.checksPlaceholder")} /></label>
      </div>
    </div>
  </div>

  <!-- Canvas + inspector -->
  <div class="workspace">
    <div class="flowwrap">
      <SvelteFlow bind:nodes bind:edges {nodeTypes} fitView onnodeclick={onNodeClick}>
        <Background gap={22} />
        <Controls />
        <MiniMap pannable zoomable />
      </SvelteFlow>
    </div>

    <div class="card inspector">
      {#if isStep}
        {@const sk = selected as StepKey}
        <h3>{STEP_LABEL[sk]}</h3>
        <label class="blk">{$t("builder.systemPrompt")}
          <textarea rows="7" bind:value={prompts[sk]} placeholder={$t("builder.promptPlaceholder")}></textarea>
        </label>
        {#if sk === "action"}
          <div class="palette">
            <span class="muted small">{$t("builder.palette")}</span>
            <div class="chips">
              {#each TOOLS as tool}
                <button class="add" onclick={() => addTool(tool)} disabled={tools.includes(tool)}>+ {tool}</button>
              {/each}
            </div>
            <div class="chosen">
              {#each tools as tool}
                <span class="chip">{tool} <button class="x" onclick={() => removeTool(tool)}>×</button></span>
              {/each}
            </div>
          </div>
        {/if}
      {:else if selected === "trigger"}
        <h3>{$t("builder.node.trigger")}</h3>
        <label class="blk">{$t("builder.triggerOn")}<input bind:value={triggerOn} /></label>
        <p class="muted small">{$t("builder.triggerHint")}</p>
      {:else if selected === "emit"}
        <h3>{$t("builder.node.emit")}</h3>
        <label class="blk">{$t("builder.emit")}<input bind:value={emit} /></label>
        <p class="muted small">{$t("builder.emitHint")}</p>
      {:else}
        <p class="muted">{$t("builder.tapToEdit")}</p>
      {/if}

      <button class="primary create" onclick={create}>{$t("builder.create")}</button>
      {#if createdMsg}<p class="muted small">{createdMsg}</p>{/if}
    </div>
  </div>
</div>

<style>
  .builder { display: flex; flex-direction: column; gap: 1rem; }
  .meta .grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 0.6rem 1rem; }
  .grid3 { display: grid; grid-template-columns: repeat(3, 1fr); gap: 0.6rem 1rem; margin-top: 0.4rem; }
  .loop { margin-top: 0.9rem; padding-top: 0.8rem; border-top: 1px solid var(--border); }
  label { display: block; font-size: 0.8rem; color: var(--muted); }
  input, select, textarea {
    width: 100%; background: var(--bg); border: 1px solid var(--border); color: var(--text);
    border-radius: 8px; padding: 0.45rem 0.6rem; font: inherit; margin-top: 0.2rem;
  }
  .workspace { display: grid; grid-template-columns: 1fr 320px; gap: 1rem; align-items: stretch; }
  .flowwrap { height: 70vh; min-height: 520px; border: 1px solid var(--border); border-radius: 12px; overflow: hidden; background: var(--bg); }
  .inspector { display: flex; flex-direction: column; }
  .inspector h3 { margin: 0 0 0.6rem; }
  .blk { display: block; }
  .blk textarea { font-family: ui-monospace, monospace; font-size: 0.82rem; }
  .palette { margin-top: 0.8rem; }
  .chips { display: flex; gap: 0.3rem; flex-wrap: wrap; margin-top: 0.3rem; }
  .add { background: var(--panel); border: 1px solid var(--border); color: var(--text); border-radius: 7px; padding: 0.2rem 0.5rem; cursor: pointer; font: inherit; font-size: 0.75rem; }
  .add:disabled { opacity: 0.4; cursor: default; }
  .chosen { display: flex; gap: 0.35rem; flex-wrap: wrap; margin-top: 0.5rem; }
  .chip { background: color-mix(in srgb, var(--accent) 16%, transparent); color: var(--accent); border-radius: 20px; padding: 0.1rem 0.5rem; font-size: 0.78rem; }
  .chip .x { background: none; border: none; color: var(--accent); cursor: pointer; font-size: 0.9rem; padding: 0; }
  .create { margin-top: auto; background: var(--accent); border: 1px solid var(--accent); color: #04231a; font-weight: 600; border-radius: 8px; padding: 0.6rem; cursor: pointer; font: inherit; }
  .small { font-size: 0.78rem; }
  :global(.svelte-flow__attribution) { display: none; }
</style>
