<script lang="ts">
  // Brand logo from simple-icons CDN, with a graceful letter fallback when the
  // slug doesn't exist (e.g. brands removed from simple-icons).
  export let slug = "";
  export let label = "";
  export let size = 28;
  let failed = false;
  $: src = `https://cdn.simpleicons.org/${slug}`;
</script>

{#if slug && !failed}
  <img
    {src}
    alt={label}
    width={size}
    height={size}
    loading="lazy"
    on:error={() => (failed = true)}
  />
{:else}
  <span class="fallback" style="width:{size}px;height:{size}px;font-size:{size * 0.5}px">
    {(label || "?").slice(0, 1).toUpperCase()}
  </span>
{/if}

<style>
  img { display: block; object-fit: contain; }
  .fallback {
    display: inline-flex; align-items: center; justify-content: center;
    background: #1a2133; border: 1px solid var(--border); border-radius: 7px;
    color: var(--accent); font-weight: 700;
  }
</style>
