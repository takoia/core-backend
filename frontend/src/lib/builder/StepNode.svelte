<script lang="ts">
  import { Handle, Position } from "@xyflow/svelte";

  // Custom Svelte Flow node for an agent pipeline step.
  export let data: {
    label: string;
    kind: "trigger" | "step" | "emit";
    sub?: string;
    idx?: number;
    tools?: string[];
  };
</script>

<div class="snode {data.kind}">
  {#if data.kind !== "trigger"}
    <Handle type="target" position={Position.Top} />
  {/if}

  <div class="top">
    {#if data.idx !== undefined}<span class="idx">{data.idx}</span>{/if}
    <span class="title">{data.label}</span>
  </div>
  {#if data.sub}<div class="sub">{data.sub}</div>{/if}
  {#if data.kind === "step" && data.tools}
    <div class="tools">
      {#if data.tools.length === 0}
        <span class="empty">—</span>
      {:else}
        {#each data.tools as t}<span class="chip">{t}</span>{/each}
      {/if}
    </div>
  {/if}

  {#if data.kind !== "emit"}
    <Handle type="source" position={Position.Bottom} />
  {/if}
</div>

<style>
  .snode {
    width: 220px;
    background: var(--panel);
    border: 1.5px solid var(--border);
    border-radius: 14px;
    padding: 0.7rem 0.85rem;
    color: var(--text);
    font: inherit;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.25);
  }
  .snode.trigger,
  .snode.emit {
    background: color-mix(in srgb, var(--accent) 16%, var(--panel));
    border-color: color-mix(in srgb, var(--accent) 50%, var(--border));
  }
  :global(.svelte-flow__node.selected) .snode {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent) 45%, transparent);
  }
  .top { display: flex; align-items: center; gap: 0.45rem; }
  .idx {
    width: 20px; height: 20px; border-radius: 50%;
    background: var(--accent); color: #04231a;
    display: inline-flex; align-items: center; justify-content: center;
    font-size: 0.72rem; font-weight: 700; flex: none;
  }
  .title { font-weight: 600; font-size: 0.9rem; }
  .sub { color: var(--muted); font-size: 0.74rem; margin-top: 0.25rem; }
  .tools { display: flex; gap: 0.3rem; flex-wrap: wrap; margin-top: 0.45rem; }
  .chip {
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    color: var(--accent); border-radius: 20px;
    padding: 0.05rem 0.45rem; font-size: 0.7rem;
  }
  .empty { color: var(--muted); font-size: 0.72rem; }
</style>
