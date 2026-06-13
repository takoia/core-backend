<script lang="ts">
  import { Handle, Position } from "@xyflow/svelte";

  // Custom node for any builder block (trigger, step, tool, control, emit).
  export let data: {
    label: string;
    kind: string; // trigger | step | tool | control | emit
    glyph?: string;
    sub?: string;
    status?: "running" | "done" | "pending";
  };

  // A glyph can be an emoji/text OR an uploaded image (data: URL or http URL).
  $: isImage = !!data.glyph && /^(data:|https?:|\/)/.test(data.glyph);
</script>

<div class="snode {data.kind} {data.status ?? ''}">
  {#if data.kind !== "trigger"}<Handle type="target" position={Position.Top} />{/if}
  <div class="top">
    {#if data.glyph}
      {#if isImage}<img class="glyph-img" src={data.glyph} alt="" />{:else}<span class="glyph">{data.glyph}</span>{/if}
    {/if}
    <span class="title">{data.label}</span>
  </div>
  {#if data.sub}<div class="sub">{data.sub}</div>{/if}
  {#if data.kind !== "emit"}<Handle type="source" position={Position.Bottom} />{/if}
</div>

<style>
  .snode {
    width: 190px; background: var(--panel); border: 1.5px solid var(--border);
    border-radius: 14px; padding: 0.65rem 0.8rem; color: var(--text); font: inherit;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.25);
  }
  .snode.trigger, .snode.emit { background: color-mix(in srgb, var(--accent) 16%, var(--panel)); border-color: color-mix(in srgb, var(--accent) 50%, var(--border)); }
  .snode.tool { background: color-mix(in srgb, var(--ok) 10%, var(--panel)); border-color: color-mix(in srgb, var(--ok) 40%, var(--border)); }
  .snode.control { background: color-mix(in srgb, var(--warn) 12%, var(--panel)); border-color: color-mix(in srgb, var(--warn) 45%, var(--border)); }
  :global(.svelte-flow__node.selected) .snode { border-color: var(--accent); box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 45%, transparent); }
  .snode.running { border-color: var(--warn); box-shadow: 0 0 0 3px color-mix(in srgb, var(--warn) 40%, transparent); animation: pulse 1s infinite; }
  .snode.done { border-color: var(--ok); box-shadow: 0 0 0 2px color-mix(in srgb, var(--ok) 35%, transparent); }
  @keyframes pulse { 50% { opacity: 0.6; } }
  .top { display: flex; align-items: center; gap: 0.45rem; }
  .glyph { font-size: 1.1rem; }
  .glyph-img { width: 22px; height: 22px; border-radius: 6px; object-fit: cover; flex: 0 0 auto; }
  .title { font-weight: 600; font-size: 0.88rem; }
  .sub { color: var(--muted); font-size: 0.74rem; margin-top: 0.25rem; }
</style>
