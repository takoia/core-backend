<script lang="ts">
  import { Handle, Position } from "@xyflow/svelte";

  // Custom node for any builder block (trigger, step, tool, control, emit).
  export let id = "";
  export let data: {
    label: string;
    kind: string; // trigger | step | tool | control | emit
    glyph?: string;
    sub?: string;
    status?: "running" | "done" | "pending";
    root?: boolean; // the agent node: no incoming handle
    branches?: { id: string; label: string }[]; // multiple labeled outputs (if/loop)
  };

  // Control blocks always expose distinct labeled outputs, derived from the
  // node id so even pre-existing nodes get the right handles. One output goes
  // down, the other to the right, each with its own color.
  type Branch = { id: string; label: string; side: "bottom" | "right" };
  const CTL_BRANCHES: Record<string, Branch[]> = {
    if: [{ id: "then", label: "Alors", side: "bottom" }, { id: "else", label: "Sinon", side: "right" }],
    for: [{ id: "each", label: "Pour chaque", side: "bottom" }, { id: "done", label: "Fin", side: "right" }],
    loop: [{ id: "body", label: "Répéter", side: "bottom" }, { id: "done", label: "Fin", side: "right" }],
  };

  // A glyph can be an emoji/text OR an uploaded image (data: URL or http URL).
  $: isImage = !!data.glyph && /^(data:|https?:|\/)/.test(data.glyph);
  $: ctlKey = id.replace(/-\d+$/, "");
  $: branches = (data.branches as Branch[] | undefined) ?? CTL_BRANCHES[ctlKey] ?? [];
  const sidePos = (s: string) => (s === "right" ? Position.Right : Position.Bottom);
</script>

<div class="snode {data.kind} {data.status ?? ''} {data.root ? 'root' : ''}">
  {#if !data.root}<Handle type="target" position={Position.Top} />{/if}
  <div class="top">
    {#if data.status === "running"}
      <span class="snode-spin" title="en cours…"></span>
    {:else if data.glyph}
      {#if isImage}<img class="glyph-img" src={data.glyph} alt="" />{:else}<span class="glyph">{data.glyph}</span>{/if}
    {/if}
    <span class="title">{data.label}</span>
    {#if data.status === "done"}<span class="snode-done" title="terminé">✓</span>{/if}
  </div>
  {#if data.sub}<div class="sub">{data.sub}</div>{/if}
  {#if branches.length}
    {#each branches as b}
      <Handle type="source" id={b.id} position={sidePos(b.side)} />
      <span class="hlabel {b.side}">{b.label}</span>
    {/each}
  {:else if data.kind !== "emit"}
    <Handle type="source" position={Position.Bottom} />
  {/if}
</div>

<style>
  .snode {
    width: 190px; background: var(--panel); border: 1.5px solid var(--border);
    border-radius: 14px; padding: 0.65rem 0.8rem; color: var(--text); font: inherit;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.25);
  }
  .snode.trigger, .snode.emit { background: color-mix(in srgb, var(--accent) 16%, var(--panel)); border-color: color-mix(in srgb, var(--accent) 50%, var(--border)); }
  /* The agent (general) box stands out with its own color. */
  .snode.root { background: color-mix(in srgb, #a855f7 22%, var(--panel)); border-color: #a855f7; box-shadow: 0 6px 22px color-mix(in srgb, #a855f7 30%, transparent); width: 210px; }
  .snode.tool { background: color-mix(in srgb, var(--ok) 10%, var(--panel)); border-color: color-mix(in srgb, var(--ok) 40%, var(--border)); }
  /* Output blocks (file/mail/discord/webhook/ftp) — distinct "sortie" color. */
  .snode.out { background: color-mix(in srgb, #e879f9 14%, var(--panel)); border-color: #e879f9; }
  .snode.control { background: color-mix(in srgb, var(--warn) 12%, var(--panel)); border-color: color-mix(in srgb, var(--warn) 45%, var(--border)); }
  :global(.svelte-flow__node.selected) .snode { border-color: var(--accent); box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 45%, transparent); }
  .snode.running { border-color: var(--warn); box-shadow: 0 0 0 3px color-mix(in srgb, var(--warn) 40%, transparent); animation: pulse 1s infinite; }
  .snode.done { border-color: var(--ok); box-shadow: 0 0 0 2px color-mix(in srgb, var(--ok) 35%, transparent); }
  @keyframes pulse { 50% { opacity: 0.6; } }
  .top { display: flex; align-items: center; gap: 0.45rem; }
  .glyph { font-size: 1.1rem; }
  .glyph-img { width: 22px; height: 22px; border-radius: 6px; object-fit: cover; flex: 0 0 auto; }
  .title { font-weight: 600; font-size: 0.88rem; flex: 1; }
  .snode-spin { width: 16px; height: 16px; flex: 0 0 auto; border: 2px solid color-mix(in srgb, var(--warn) 35%, transparent); border-top-color: var(--warn); border-radius: 50%; animation: snspin 0.7s linear infinite; }
  @keyframes snspin { to { transform: rotate(360deg); } }
  .snode-done { color: var(--ok); font-weight: 700; flex: 0 0 auto; }
  .sub { color: var(--muted); font-size: 0.74rem; margin-top: 0.25rem; }
  .hlabel { position: absolute; font-size: 0.62rem; color: var(--muted); white-space: nowrap; pointer-events: none; }
  .hlabel.bottom { bottom: -1.05rem; left: 50%; transform: translateX(-50%); }
  .hlabel.right { left: 100%; top: 50%; transform: translateY(-50%); margin-left: 8px; }
  .snode { position: relative; }
  /* Make every connection point clearly visible, with a color per role. */
  .snode :global(.svelte-flow__handle) { width: 12px; height: 12px; border: 2px solid var(--panel); background: var(--accent); }
  .snode :global(.svelte-flow__handle-top) { top: -6px; }
  .snode :global(.svelte-flow__handle-bottom) { bottom: -6px; }
  .snode :global(.svelte-flow__handle-right) { right: -6px; }
  /* Incoming = accent; first output (then/each/body) = green; second (else/done) = red. */
  .snode :global(.svelte-flow__handle[data-handleid="then"]),
  .snode :global(.svelte-flow__handle[data-handleid="each"]),
  .snode :global(.svelte-flow__handle[data-handleid="body"]) { background: var(--ok); }
  .snode :global(.svelte-flow__handle[data-handleid="else"]),
  .snode :global(.svelte-flow__handle[data-handleid="done"]) { background: var(--err); }
</style>
