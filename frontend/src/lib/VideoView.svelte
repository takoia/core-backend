<script lang="ts">
  import { onMount } from "svelte";
  import { api, type Agent } from "./api";
  import { t } from "./i18n";

  const MAX_FRAMES = 60;
  const FRAME_WIDTH = 640;

  let videoEl: HTMLVideoElement;
  let videoUrl: string | null = null;
  let prompt = "";
  let status = "";
  let busy = false;
  // Configurable analysis window (seconds), one frame per second.
  let durationSec = 30;

  // Improvement source: screen/video frames, a single photo, or free text.
  let mode: "video" | "photo" | "description" = "video";
  let imageUrl: string | null = null;
  let imageData = "";
  let descText = "";

  // Improve-an-agent target.
  let agents: Agent[] = [];
  let targetAgent = "";
  let savedMsg = "";

  onMount(async () => {
    try {
      agents = await api.listAgents();
      if (agents.length) targetAgent = agents[0].id;
    } catch {
      agents = [];
    }
  });

  // Extracted info items + per-item human confirmation (null = undecided).
  type Item = { info: string; detail: string; ok: boolean | null };
  let items: Item[] = [];

  // Screen recording state.
  let recorder: MediaRecorder | null = null;
  let recording = false;
  let chunks: BlobPart[] = [];

  function setVideo(blob: Blob) {
    if (videoUrl) URL.revokeObjectURL(videoUrl);
    videoUrl = URL.createObjectURL(blob);
    items = [];
  }

  function onFile(e: Event) {
    const file = (e.target as HTMLInputElement).files?.[0];
    if (file) setVideo(file);
  }

  // Photo mode: read a single image and analyze it as one frame.
  function onPhoto(e: Event) {
    const file = (e.target as HTMLInputElement).files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => { imageData = reader.result as string; imageUrl = imageData; items = []; };
    reader.readAsDataURL(file);
  }

  async function analyzePhoto() {
    if (!imageData) return;
    busy = true; items = [];
    try {
      status = $t("video.analyzing", { n: 1 });
      const r = await api.analyzeVideo([imageData], prompt || undefined);
      items = r.items.map((it) => ({ ...it, ok: null }));
      status = $t("video.done", { n: r.frame_count });
    } catch (e) {
      status = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  // Description mode: each non-empty line becomes a confirmable memory item.
  function applyDescription() {
    const lines = descText.split("\n").map((l) => l.trim()).filter(Boolean);
    const src = lines.length ? lines : [descText.trim()].filter(Boolean);
    items = src.map((l) => ({ info: l, detail: "", ok: true as boolean | null }));
  }

  async function startRecording() {
    try {
      const stream = await navigator.mediaDevices.getDisplayMedia({ video: true, audio: false });
      chunks = [];
      recorder = new MediaRecorder(stream);
      recorder.ondataavailable = (ev) => ev.data.size && chunks.push(ev.data);
      recorder.onstop = () => {
        stream.getTracks().forEach((tr) => tr.stop());
        setVideo(new Blob(chunks, { type: "video/webm" }));
      };
      recorder.start();
      recording = true;
      status = $t("video.recording");
    } catch (e) {
      status = e instanceof Error ? e.message : String(e);
    }
  }

  function stopRecording() {
    recorder?.stop();
    recording = false;
    status = "";
  }

  function seekTo(time: number): Promise<void> {
    return new Promise((resolve) => {
      const onSeeked = () => {
        videoEl.removeEventListener("seeked", onSeeked);
        resolve();
      };
      videoEl.addEventListener("seeked", onSeeked);
      videoEl.currentTime = time;
    });
  }

  async function extractFrames(): Promise<string[]> {
    const duration = videoEl.duration;
    const scale = Math.min(1, FRAME_WIDTH / (videoEl.videoWidth || FRAME_WIDTH));
    const canvas = document.createElement("canvas");
    canvas.width = Math.round((videoEl.videoWidth || FRAME_WIDTH) * scale);
    canvas.height = Math.round((videoEl.videoHeight || 360) * scale);
    const ctx = canvas.getContext("2d")!;
    const window = Math.min(durationSec || 30, MAX_FRAMES);
    const count = Math.min(window, Math.max(1, Math.floor(duration)));
    const out: string[] = [];
    for (let s = 0; s < count; s++) {
      status = $t("video.extracting", { i: s + 1, n: count });
      await seekTo(Math.min(s + 0.05, duration - 0.05));
      ctx.drawImage(videoEl, 0, 0, canvas.width, canvas.height);
      out.push(canvas.toDataURL("image/png"));
    }
    return out;
  }

  async function analyze() {
    if (!videoUrl) return;
    busy = true;
    items = [];
    try {
      const frames = await extractFrames();
      status = $t("video.analyzing", { n: frames.length });
      const r = await api.analyzeVideo(frames, prompt || undefined);
      items = r.items.map((it) => ({ ...it, ok: null }));
      status = $t("video.done", { n: r.frame_count });
    } catch (e) {
      status = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  $: confirmed = items.filter((i) => i.ok === true);

  // Improve an existing agent: store the confirmed info into its memory.
  async function improveAgent() {
    if (!targetAgent || !confirmed.length) return;
    savedMsg = "";
    try {
      for (const it of confirmed) {
        await api.addAgentMemory(targetAgent, `${it.info}: ${it.detail}`, "demonstration");
      }
      savedMsg = $t("video.improved", { n: confirmed.length });
    } catch (e) {
      savedMsg = e instanceof Error ? e.message : String(e);
    }
  }

  let speaking = false;
  // Speak the AI's understanding aloud via the backend OpenAI TTS endpoint.
  async function speak() {
    const chosen = confirmed.length ? confirmed : items;
    const text = chosen.map((i) => `${i.info}. ${i.detail}`).join(". ");
    if (!text) return;
    speaking = true;
    try {
      const res = await fetch("/api/tts", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ text }),
      });
      if (!res.ok) throw new Error(await res.text());
      const url = URL.createObjectURL(await res.blob());
      const audio = new Audio(url);
      audio.onended = () => URL.revokeObjectURL(url);
      await audio.play();
    } catch (e) {
      status = e instanceof Error ? e.message : String(e);
    } finally {
      speaking = false;
    }
  }
</script>

<div class="card">
  <h2>{$t("video.title")} <span class="muted small">— {$t("video.subtitle")}</span></h2>
  <p class="muted small">{$t("video.hint")}</p>

  <div class="modes">
    <button class:on={mode === "video"} on:click={() => (mode = "video")}>🎥 {$t("video.modeVideo")}</button>
    <button class:on={mode === "photo"} on:click={() => (mode = "photo")}>🖼️ {$t("video.modePhoto")}</button>
    <button class:on={mode === "description"} on:click={() => (mode = "description")}>📝 {$t("video.modeText")}</button>
  </div>

  {#if mode === "video"}
    <div class="sources">
      {#if !recording}
        <button class="rec" on:click={startRecording}>● {$t("video.record")}</button>
      {:else}
        <button class="stop" on:click={stopRecording}>■ {$t("video.stop")}</button>
      {/if}
      <span class="muted small">{$t("video.or")}</span>
      <input type="file" accept="video/*" on:change={onFile} />
    </div>

    {#if videoUrl}
      <video bind:this={videoEl} src={videoUrl} controls preload="auto" muted></video>
      <div class="opts">
        <label>{$t("video.prompt")}
          <textarea rows="2" bind:value={prompt} placeholder={$t("video.promptPlaceholder")}></textarea>
        </label>
        <label class="dur">{$t("video.duration")}
          <input type="number" min="5" max={MAX_FRAMES} bind:value={durationSec} />
        </label>
      </div>
      <div class="row">
        <button class="primary" on:click={analyze} disabled={busy}>{busy ? "…" : $t("video.analyze")}</button>
        {#if status}<span class="muted small">{status}</span>{/if}
      </div>
    {/if}
  {:else if mode === "photo"}
    <div class="sources">
      <input type="file" accept="image/*" on:change={onPhoto} />
    </div>
    {#if imageUrl}
      <img class="photo" src={imageUrl} alt="" />
      <label>{$t("video.prompt")}
        <textarea rows="2" bind:value={prompt} placeholder={$t("video.promptPlaceholder")}></textarea>
      </label>
      <div class="row">
        <button class="primary" on:click={analyzePhoto} disabled={busy}>{busy ? "…" : $t("video.analyze")}</button>
        {#if status}<span class="muted small">{status}</span>{/if}
      </div>
    {/if}
  {:else}
    <label>{$t("video.descLabel")}
      <textarea rows="5" bind:value={descText} placeholder={$t("video.descPlaceholder")}></textarea>
    </label>
    <div class="row">
      <button class="primary" on:click={applyDescription} disabled={!descText.trim()}>{$t("video.useText")}</button>
    </div>
  {/if}
</div>

{#if items.length}
  <div class="card">
    <div class="head">
      <h2>{$t("video.extracted")} <span class="muted small">— {$t("video.confirmHint")}</span></h2>
      <button class="listen" on:click={speak} disabled={speaking}>🔊 {$t("video.listen")}</button>
    </div>
    {#each items as it}
      <div class="item" class:ok={it.ok === true} class:no={it.ok === false}>
        <div class="info">
          <strong>{it.info}</strong>
          <span class="muted small">{it.detail}</span>
        </div>
        <div class="decide">
          <button class="yes" class:on={it.ok === true} on:click={() => (it.ok = true)}>✓</button>
          <button class="nope" class:on={it.ok === false} on:click={() => (it.ok = false)}>✗</button>
        </div>
      </div>
    {/each}
    <p class="muted small summary">{$t("video.confirmedCount", { n: confirmed.length, total: items.length })}</p>

    <div class="improve">
      <span class="muted small">{$t("video.improveHint")}</span>
      <div class="row">
        <select bind:value={targetAgent}>
          {#each agents as a}<option value={a.id}>{a.name}</option>{/each}
        </select>
        <button class="primary" on:click={improveAgent} disabled={!confirmed.length || !targetAgent}>
          {$t("video.improveBtn")}
        </button>
        {#if savedMsg}<span class="muted small">{savedMsg}</span>{/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .modes { display: flex; gap: 0.4rem; margin: 0.8rem 0 0.2rem; flex-wrap: wrap; }
  .modes button { background: var(--bg); border: 1px solid var(--border); color: var(--muted); border-radius: 8px; padding: 0.4rem 0.7rem; cursor: pointer; font: inherit; font-size: 0.84rem; }
  .modes button.on { background: color-mix(in srgb, var(--accent) 18%, transparent); border-color: var(--accent); color: var(--text); }
  .photo { max-width: 100%; max-height: 360px; border-radius: 10px; border: 1px solid var(--border); margin-top: 0.6rem; display: block; }
  .sources { display: flex; align-items: center; gap: 0.8rem; flex-wrap: wrap; margin: 0.8rem 0; }
  input[type="file"] { color: var(--muted); }
  .rec, .stop { border: 1px solid var(--err); background: color-mix(in srgb, var(--err) 18%, transparent); color: var(--err); border-radius: 8px; padding: 0.5rem 0.9rem; cursor: pointer; font: inherit; font-weight: 600; }
  .stop { background: var(--err); color: #2a0707; }
  video { width: 100%; max-height: 380px; border-radius: 10px; border: 1px solid var(--border); background: #000; margin-top: 0.3rem; }
  label { display: block; font-size: 0.85rem; color: var(--muted); margin-top: 0.8rem; }
  textarea { width: 100%; background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.55rem 0.7rem; font: inherit; margin-top: 0.25rem; }
  .row { display: flex; gap: 0.8rem; align-items: center; margin-top: 0.8rem; flex-wrap: wrap; }
  .opts { display: grid; grid-template-columns: 1fr 120px; gap: 0.8rem; align-items: start; }
  .dur input { width: 100%; background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.55rem 0.7rem; font: inherit; margin-top: 0.25rem; }
  .improve { margin-top: 1rem; padding-top: 0.8rem; border-top: 1px solid var(--border); }
  .improve select { background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.5rem 0.6rem; font: inherit; }
  button.primary { background: var(--accent); border: 1px solid var(--accent); color: #04231a; font-weight: 600; border-radius: 8px; padding: 0.55rem 1rem; cursor: pointer; font: inherit; }
  button.primary:disabled { opacity: 0.5; cursor: default; }
  button.primary:disabled { opacity: 0.6; cursor: default; }
  .head { display: flex; justify-content: space-between; align-items: center; gap: 1rem; flex-wrap: wrap; }
  .listen { border: 1px solid var(--accent); background: color-mix(in srgb, var(--accent) 16%, transparent); color: var(--accent); border-radius: 8px; padding: 0.4rem 0.8rem; cursor: pointer; font: inherit; font-size: 0.82rem; }
  .listen:disabled { opacity: 0.6; cursor: default; }
  .item { display: flex; justify-content: space-between; align-items: center; gap: 1rem; padding: 0.55rem 0.6rem; border: 1px solid var(--border); border-radius: 8px; margin-top: 0.5rem; }
  .item.ok { border-color: var(--ok); background: color-mix(in srgb, var(--ok) 10%, transparent); }
  .item.no { border-color: var(--err); opacity: 0.6; }
  .info { display: flex; flex-direction: column; min-width: 0; }
  .decide { display: flex; gap: 0.3rem; flex: 0 0 auto; }
  .decide button { width: 30px; height: 30px; border-radius: 7px; border: 1px solid var(--border); background: var(--bg); color: var(--muted); cursor: pointer; font-size: 0.9rem; }
  .decide .yes.on { background: var(--ok); color: #04231a; border-color: var(--ok); }
  .decide .nope.on { background: var(--err); color: #2a0707; border-color: var(--err); }
  .summary { margin-top: 0.8rem; }
  .small { font-size: 0.78rem; }
</style>
