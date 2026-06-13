<script lang="ts">
  import { onMount } from "svelte";
  import { skillsApi, type Skill } from "./api";
  import Logo from "./Logo.svelte";

  let skills: Skill[] = [];
  let installed: string[] = [];
  let query = "";
  let busy: string | null = null;
  let toast = "";

  // GitHub discovery.
  let repo = "anthropics/skills";
  let path = "";
  let discovered: { name: string; path: string }[] = [];
  let discovering = false;

  $: filtered = skills.filter(
    (s) =>
      query === "" ||
      s.name.toLowerCase().includes(query.toLowerCase()) ||
      s.description.toLowerCase().includes(query.toLowerCase()),
  );

  async function load() {
    skills = await skillsApi.catalog();
    try {
      installed = await skillsApi.installed();
    } catch {
      installed = [];
    }
  }

  async function install(s: Skill) {
    busy = s.id;
    toast = "";
    try {
      await skillsApi.install({ id: s.id, repo: s.repo, path: s.path, branch: s.branch });
      toast = `Installed ${s.name}`;
      installed = await skillsApi.installed();
    } catch (e) {
      toast = `${s.name}: ${e instanceof Error ? e.message : e}`;
    } finally {
      busy = null;
    }
  }

  async function discover() {
    discovering = true;
    toast = "";
    try {
      discovered = await skillsApi.github(repo, path);
    } catch (e) {
      toast = e instanceof Error ? e.message : String(e);
      discovered = [];
    } finally {
      discovering = false;
    }
  }

  async function installDiscovered(folder: { name: string; path: string }) {
    busy = folder.path;
    try {
      await skillsApi.install({ id: folder.name, repo, path: folder.path, branch: "main" });
      toast = `Installed ${folder.name}`;
      installed = await skillsApi.installed();
    } catch (e) {
      toast = `${folder.name}: ${e instanceof Error ? e.message : e}`;
    } finally {
      busy = null;
    }
  }

  onMount(load);
</script>

<div class="card">
  <div class="head">
    <h2>Skills <span class="muted small">— extend what your agents can do</span></h2>
    <input placeholder="Search…" bind:value={query} />
  </div>
  {#if toast}<p class="toast">{toast}</p>{/if}

  <div class="grid">
    {#each filtered as s}
      <div class="skill">
        <div class="top">
          <Logo slug={s.logo} label={s.name} />
          <div class="meta">
            <strong>{s.name}</strong>
            <span class="cat">{s.category}</span>
          </div>
          {#if installed.includes(s.id)}<span class="badge ok">installed</span>{/if}
        </div>
        <p class="desc">{s.description}</p>
        <div class="actions">
          <button on:click={() => install(s)} disabled={busy === s.id || installed.includes(s.id)}>
            {busy === s.id ? "…" : installed.includes(s.id) ? "✓ Installed" : "Install"}
          </button>
          <a href={`https://github.com/${s.repo}`} target="_blank" rel="noreferrer" class="docs">GitHub ↗</a>
        </div>
      </div>
    {/each}
  </div>
</div>

<div class="card">
  <h2>Install from a GitHub repo</h2>
  <p class="muted small">Point at any repo of skill folders (each with a <code>SKILL.md</code>).</p>
  <div class="row">
    <input placeholder="owner/name" bind:value={repo} />
    <input placeholder="path (optional)" bind:value={path} />
    <button class="primary" on:click={discover} disabled={discovering}>{discovering ? "…" : "Discover"}</button>
  </div>
  {#if discovered.length}
    <div class="discovered">
      {#each discovered as f}
        <div class="drow">
          <span>{f.name}<span class="muted small"> · {f.path}</span></span>
          <button on:click={() => installDiscovered(f)} disabled={busy === f.path}>Install</button>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .head { display: flex; justify-content: space-between; align-items: center; gap: 0.6rem; flex-wrap: wrap; }
  input { background: #0b0e14; border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.45rem 0.6rem; font: inherit; }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(230px, 1fr)); gap: 0.8rem; margin-top: 1rem; }
  .skill { border: 1px solid var(--border); border-radius: 10px; padding: 0.8rem; background: #0f1422; display: flex; flex-direction: column; }
  .top { display: flex; align-items: center; gap: 0.6rem; }
  .meta { display: flex; flex-direction: column; line-height: 1.2; flex: 1; }
  .cat { color: var(--muted); font-size: 0.72rem; }
  .desc { color: var(--muted); font-size: 0.8rem; margin: 0.5rem 0; flex: 1; }
  .actions { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; }
  button { border: 1px solid var(--accent); background: var(--accent); color: #fff; border-radius: 7px; padding: 0.35rem 0.7rem; cursor: pointer; font: inherit; font-size: 0.8rem; }
  button:disabled { background: #1a2133; border-color: var(--border); color: var(--muted); cursor: default; }
  button.primary { white-space: nowrap; }
  .docs { color: var(--muted); font-size: 0.76rem; text-decoration: none; }
  .badge.ok { background: var(--ok); color: #04231a; font-size: 0.68rem; padding: 0.1rem 0.45rem; border-radius: 20px; }
  .toast { background: #11203a; border: 1px solid var(--border); padding: 0.5rem 0.7rem; border-radius: 8px; font-size: 0.82rem; }
  .row { display: flex; gap: 0.5rem; margin-top: 0.6rem; }
  .row input { flex: 1; }
  .discovered { margin-top: 0.8rem; }
  .drow { display: flex; justify-content: space-between; align-items: center; padding: 0.4rem 0; border-bottom: 1px solid #161c2a; font-size: 0.85rem; }
  .small { font-size: 0.78rem; }
</style>
