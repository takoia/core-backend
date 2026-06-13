<script lang="ts">
  import { api } from "./api";
  import { t } from "./i18n";
  import Icon from "./Icon.svelte";

  // ── Visual builder state ───────────────────────────────────────────────────
  // The page assembles a declarative agent TOML from a Scratch-like visual
  // pipeline and posts it to the existing /api/agents/import endpoint.

  type StepKey = "analyse" | "decision" | "action" | "restitution";

  const PALETTE = [
    "web_search",
    "write_report",
    "send_discord",
    "extract_fields",
    "send_email",
  ];

  // Top-level agent fields.
  let name = "Market Watcher";
  let author = "You";
  let expertise = "trading";
  let autonomy: "full_auto" | "confirm_before_action" = "confirm_before_action";
  let description = "Watches the market and reports interesting setups.";
  let triggerOn = "schedule.daily";
  let emit = "report.ready";

  // Per-step system prompts.
  let prompts: Record<StepKey, string> = {
    analyse: "Gather the relevant context and summarise what is happening.",
    decision: "Decide whether an action is worth taking and which one.",
    action: "Execute the chosen tools to carry out the decision.",
    restitution: "Write a clear, human-readable report of what was done.",
  };

  // Tools chosen for the Action node.
  let tools: string[] = ["web_search"];

  // Which node's inline editor is open.
  let selected: "trigger" | StepKey | "emit" | null = null;

  // Outcome banner.
  let createdId = "";
  let errorMsg = "";

  const STEP_NODES: { key: StepKey; labelKey: string }[] = [
    { key: "analyse", labelKey: "builder.step.analyse" },
    { key: "decision", labelKey: "builder.step.decision" },
    { key: "action", labelKey: "builder.step.action" },
    { key: "restitution", labelKey: "builder.step.restitution" },
  ];

  function slug(s: string): string {
    return (
      s
        .toLowerCase()
        .trim()
        .replace(/[^a-z0-9]+/g, "-")
        .replace(/^-+|-+$/g, "") || "agent"
    );
  }

  function addTool(tool: string) {
    if (!tools.includes(tool)) tools = [...tools, tool];
    selected = "action";
  }

  function removeTool(tool: string) {
    tools = tools.filter((x) => x !== tool);
  }

  // ── Native HTML5 drag-and-drop from palette onto the Action node ───────────
  let dragTool: string | null = null;
  let dropHot = false;

  function onDragStart(ev: DragEvent, tool: string) {
    dragTool = tool;
    ev.dataTransfer?.setData("text/plain", tool);
    if (ev.dataTransfer) ev.dataTransfer.effectAllowed = "copy";
  }

  function onDropTool(ev: DragEvent) {
    ev.preventDefault();
    dropHot = false;
    const tool = ev.dataTransfer?.getData("text/plain") || dragTool;
    if (tool) addTool(tool);
    dragTool = null;
  }

  function tomlList(items: string[]): string {
    return "[" + items.map((x) => `"${x}"`).join(", ") + "]";
  }

  function esc(s: string): string {
    return s.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
  }

  function buildToml(): string {
    const id = slug(name);
    const emitItems = emit
      .split(",")
      .map((x) => x.trim())
      .filter(Boolean);
    const lines: string[] = [];
    lines.push("[agent]");
    lines.push(`id = "${id}"`);
    lines.push(`name = "${esc(name)}"`);
    lines.push(`author = "${esc(author)}"`);
    lines.push(`version = "0.1.0"`);
    lines.push(`description = "${esc(description)}"`);
    lines.push(`expertise = "${esc(expertise)}"`);
    lines.push(`autonomy = "${autonomy}"`);
    lines.push(`emit = ${tomlList(emitItems)}`);
    lines.push("");
    lines.push("[trigger]");
    lines.push(`on = "${esc(triggerOn)}"`);
    lines.push("");
    for (const { key } of STEP_NODES) {
      lines.push(`[steps.${key}]`);
      lines.push(`system_prompt = "${esc(prompts[key])}"`);
      if (key === "action") lines.push(`allowed_tools = ${tomlList(tools)}`);
      lines.push("");
    }
    return lines.join("\n");
  }

  async function createAgent() {
    createdId = "";
    errorMsg = "";
    try {
      const r = await api.importToml(buildToml());
      createdId = r.id;
      selected = null;
    } catch (e) {
      errorMsg = e instanceof Error ? e.message : String(e);
    }
  }

  $: previewToml = buildToml();
</script>

<div class="builder">
  <!-- Top form: identity of the agent -->
  <div class="card head">
    <div class="head-grid">
      <label>
        <span>{$t("builder.name")}</span>
        <input bind:value={name} placeholder="Market Watcher" />
      </label>
      <label>
        <span>{$t("builder.author")}</span>
        <input bind:value={author} placeholder="You" />
      </label>
      <label>
        <span>{$t("builder.expertise")}</span>
        <input bind:value={expertise} placeholder="trading" />
      </label>
      <label class="auto">
        <span>{$t("builder.autonomy")}</span>
        <div class="toggle">
          <button
            class:active={autonomy === "confirm_before_action"}
            on:click={() => (autonomy = "confirm_before_action")}
            type="button">{$t("builder.confirm")}</button
          >
          <button
            class:active={autonomy === "full_auto"}
            on:click={() => (autonomy = "full_auto")}
            type="button">{$t("builder.fullAuto")}</button
          >
        </div>
      </label>
    </div>
  </div>

  <div class="stage">
    <!-- Palette of draggable task blocks -->
    <div class="card palette">
      <h3>{$t("builder.palette")}</h3>
      <p class="muted small">{$t("builder.paletteHint")}</p>
      {#each PALETTE as tool}
        <button
          class="block"
          draggable="true"
          on:dragstart={(e) => onDragStart(e, tool)}
          on:click={() => addTool(tool)}
          type="button"
        >
          <span class="dot"></span>{tool}
        </button>
      {/each}
    </div>

    <!-- 2D node canvas -->
    <div class="card canvas">
      <div class="flow">
        <!-- Trigger node -->
        <button
          class="node trigger"
          class:sel={selected === "trigger"}
          on:click={() => (selected = "trigger")}
          type="button"
        >
          <div class="node-icon"><Icon name="run" size={16} /></div>
          <div class="node-title">{$t("builder.node.trigger")}</div>
          <div class="node-sub">{triggerOn}</div>
        </button>

        <div class="wire"></div>

        {#each STEP_NODES as node, i}
          <button
            class="node step"
            class:sel={selected === node.key}
            class:hot={node.key === "action" && dropHot}
            on:click={() => (selected = node.key)}
            on:dragover={(e) =>
              node.key === "action" && (e.preventDefault(), (dropHot = true))}
            on:dragleave={() => node.key === "action" && (dropHot = false)}
            on:drop={(e) => node.key === "action" && onDropTool(e)}
            type="button"
          >
            <div class="node-idx">{i + 1}</div>
            <div class="node-title">{$t(node.labelKey)}</div>
            {#if node.key === "action"}
              <div class="chips">
                {#if tools.length === 0}
                  <span class="muted small">{$t("builder.dropHere")}</span>
                {/if}
                {#each tools as tool}
                  <span class="chip">
                    {tool}
                    <span
                      class="x"
                      role="button"
                      tabindex="0"
                      on:click|stopPropagation={() => removeTool(tool)}
                      on:keydown|stopPropagation={(e) =>
                        e.key === "Enter" && removeTool(tool)}>×</span
                    >
                  </span>
                {/each}
              </div>
            {:else}
              <div class="node-sub">{$t("builder.tapToEdit")}</div>
            {/if}
          </button>

          <div class="wire"></div>
        {/each}

        <!-- Emit node -->
        <button
          class="node emit"
          class:sel={selected === "emit"}
          on:click={() => (selected = "emit")}
          type="button"
        >
          <div class="node-icon"><Icon name="agents" size={16} /></div>
          <div class="node-title">{$t("builder.node.emit")}</div>
          <div class="node-sub">{emit || "—"}</div>
        </button>
      </div>

      <div class="actions-bar">
        <button class="primary" on:click={createAgent} type="button">
          {$t("builder.create")}
        </button>
        {#if createdId}
          <span class="ok small">{$t("builder.created")}: {createdId}</span>
        {/if}
        {#if errorMsg}
          <span class="err small">{errorMsg}</span>
        {/if}
      </div>
    </div>

    <!-- Inline editor side panel -->
    <div class="card editor">
      {#if selected === "trigger"}
        <h3>{$t("builder.node.trigger")}</h3>
        <label>
          <span>{$t("builder.triggerOn")}</span>
          <input bind:value={triggerOn} placeholder="schedule.daily" />
        </label>
        <p class="muted small">{$t("builder.triggerHint")}</p>
      {:else if selected === "emit"}
        <h3>{$t("builder.node.emit")}</h3>
        <label>
          <span>{$t("builder.emit")}</span>
          <input bind:value={emit} placeholder="report.ready" />
        </label>
        <p class="muted small">{$t("builder.emitHint")}</p>
      {:else if selected === "analyse" || selected === "decision" || selected === "action" || selected === "restitution"}
        <h3>{$t(`builder.step.${selected}`)}</h3>
        <label>
          <span>{$t("builder.systemPrompt")}</span>
          <textarea rows="6" bind:value={prompts[selected]}></textarea>
        </label>
        {#if selected === "action"}
          <p class="muted small">{$t("builder.actionHint")}</p>
        {/if}
      {:else}
        <h3>{$t("builder.previewTitle")}</h3>
        <p class="muted small">{$t("builder.previewHint")}</p>
        <pre class="preview">{previewToml}</pre>
      {/if}
    </div>
  </div>
</div>

<style>
  .builder {
    padding: 1.2rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }
  .head-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 0.8rem;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    font-size: 0.8rem;
  }
  label > span {
    color: var(--muted);
  }
  input,
  textarea {
    background: var(--bg);
    border: 1px solid var(--border);
    color: var(--text);
    border-radius: 8px;
    padding: 0.5rem 0.6rem;
    font: inherit;
    font-size: 0.85rem;
  }
  textarea {
    font-family: ui-monospace, monospace;
    font-size: 0.8rem;
    resize: vertical;
  }
  .toggle {
    display: flex;
    gap: 0.3rem;
  }
  .toggle button {
    flex: 1;
    border: 1px solid var(--border);
    background: var(--bg);
    color: var(--muted);
    border-radius: 8px;
    padding: 0.45rem;
    cursor: pointer;
    font: inherit;
    font-size: 0.78rem;
  }
  .toggle button.active {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
  }

  .stage {
    display: grid;
    grid-template-columns: 180px 1fr 280px;
    gap: 1rem;
    align-items: start;
  }
  h3 {
    margin: 0 0 0.6rem;
    font-size: 0.95rem;
  }

  /* Palette */
  .palette .block {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.45rem;
    border: 1px solid var(--border);
    background: var(--panel);
    color: var(--text);
    border-radius: 10px;
    padding: 0.5rem 0.6rem;
    cursor: grab;
    font: inherit;
    font-size: 0.8rem;
    text-align: left;
    transition: border-color 0.12s;
  }
  .palette .block:hover {
    border-color: var(--accent);
  }
  .palette .block:active {
    cursor: grabbing;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
    flex: none;
  }

  /* Canvas + flow */
  .canvas {
    overflow-x: auto;
  }
  .flow {
    display: flex;
    align-items: center;
    gap: 0;
    padding: 0.5rem 0.2rem 1rem;
    min-width: max-content;
  }
  .node {
    border: 1.5px solid var(--border);
    background: var(--panel);
    color: var(--text);
    border-radius: 14px;
    padding: 0.7rem 0.8rem;
    min-width: 130px;
    cursor: pointer;
    font: inherit;
    text-align: left;
    flex: none;
    transition:
      border-color 0.12s,
      box-shadow 0.12s;
  }
  .node:hover {
    border-color: var(--accent);
  }
  .node.sel {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 35%, transparent);
  }
  .node.trigger,
  .node.emit {
    background: color-mix(in srgb, var(--accent) 14%, var(--panel));
    border-color: color-mix(in srgb, var(--accent) 45%, var(--border));
  }
  .node.hot {
    border-color: var(--ok);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--ok) 40%, transparent);
  }
  .node-icon {
    color: var(--accent);
    margin-bottom: 0.2rem;
  }
  .node-idx {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    background: var(--accent);
    color: #fff;
    display: grid;
    place-items: center;
    font-size: 0.75rem;
    font-weight: 600;
    margin-bottom: 0.35rem;
  }
  .node-title {
    font-weight: 600;
    font-size: 0.85rem;
  }
  .node-sub {
    color: var(--muted);
    font-size: 0.72rem;
    margin-top: 0.2rem;
  }
  .wire {
    width: 34px;
    height: 2px;
    background: var(--border);
    flex: none;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem;
    margin-top: 0.4rem;
    max-width: 150px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 20px;
    padding: 0.12rem 0.45rem;
    font-size: 0.7rem;
  }
  .chip .x {
    cursor: pointer;
    color: var(--muted);
    font-size: 0.85rem;
    line-height: 1;
  }
  .chip .x:hover {
    color: var(--err);
  }

  .actions-bar {
    display: flex;
    align-items: center;
    gap: 0.9rem;
    margin-top: 0.4rem;
    flex-wrap: wrap;
  }
  .primary {
    border: 1px solid var(--accent);
    background: var(--accent);
    color: #fff;
    border-radius: 9px;
    padding: 0.55rem 1.1rem;
    cursor: pointer;
    font: inherit;
    font-weight: 600;
  }
  .ok {
    color: var(--ok);
  }
  .err {
    color: var(--err);
  }

  /* Editor */
  .editor label {
    margin-bottom: 0.5rem;
  }
  .preview {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 0.6rem;
    font-family: ui-monospace, monospace;
    font-size: 0.72rem;
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 360px;
    overflow: auto;
  }
  .small {
    font-size: 0.75rem;
  }
  @media (max-width: 1000px) {
    .stage {
      grid-template-columns: 1fr;
    }
  }
</style>
