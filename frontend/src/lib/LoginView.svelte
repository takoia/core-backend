<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { api } from "./api";
  import { t, locale, setLocale } from "./i18n";
  import logo from "./assets/takoia.png";

  export let onAuthed: (token: string) => void = () => {};

  let username = "admin";
  let password = "";
  let error = "";
  let busy = false;

  let canvasEl: HTMLCanvasElement;
  let raf = 0;

  async function submit() {
    error = "";
    busy = true;
    try {
      const r = await api.login(username, password);
      localStorage.setItem("auth_token", r.token);
      localStorage.setItem("auth_user", r.username);
      onAuthed(r.token);
    } catch {
      error = $t("login.error");
    } finally {
      busy = false;
    }
  }

  // Animated 2D ocean waves + drifting bubbles behind the login box.
  function draw() {
    const c = canvasEl;
    if (!c) return;
    const dpr = window.devicePixelRatio || 1;
    const rect = c.getBoundingClientRect();
    if (c.width !== rect.width * dpr || c.height !== rect.height * dpr) {
      c.width = rect.width * dpr;
      c.height = rect.height * dpr;
    }
    const ctx = c.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const W = rect.width;
    const H = rect.height;
    const tms = performance.now() / 1000;

    const g = ctx.createRadialGradient(W / 2, -H * 0.1, 60, W / 2, H * 0.3, H);
    g.addColorStop(0, "#103a4d");
    g.addColorStop(1, "#06121c");
    ctx.fillStyle = g;
    ctx.fillRect(0, 0, W, H);

    // Layered waves
    const layers = [
      { amp: 26, len: 0.012, speed: 0.6, y: H * 0.62, color: "rgba(47,196,214,0.12)" },
      { amp: 34, len: 0.009, speed: 0.9, y: H * 0.72, color: "rgba(47,196,214,0.16)" },
      { amp: 46, len: 0.006, speed: 1.3, y: H * 0.82, color: "rgba(21,150,170,0.22)" },
    ];
    for (const L of layers) {
      ctx.beginPath();
      ctx.moveTo(0, H);
      for (let x = 0; x <= W; x += 6) {
        const y = L.y + Math.sin(x * L.len + tms * L.speed) * L.amp
          + Math.sin(x * L.len * 2.3 + tms * L.speed * 1.7) * (L.amp * 0.3);
        ctx.lineTo(x, y);
      }
      ctx.lineTo(W, H);
      ctx.closePath();
      ctx.fillStyle = L.color;
      ctx.fill();
    }

    // Drifting bubbles
    for (let i = 0; i < 22; i++) {
      const seed = i * 137.5;
      const x = (W * ((seed % 100) / 100) + tms * (8 + (i % 5) * 4)) % (W + 40) - 20;
      const y = H - ((tms * (18 + (i % 7) * 6) + seed * 3) % (H + 60)) + 30;
      const rad = 1.5 + (i % 4);
      ctx.beginPath();
      ctx.arc(x, y, rad, 0, Math.PI * 2);
      ctx.fillStyle = `rgba(150,220,232,${0.05 + (i % 3) * 0.04})`;
      ctx.fill();
    }

    raf = requestAnimationFrame(draw);
  }

  onMount(() => {
    raf = requestAnimationFrame(draw);
  });
  onDestroy(() => cancelAnimationFrame(raf));
</script>

<div class="wrap">
  <canvas bind:this={canvasEl} class="bg"></canvas>

  <form class="box" on:submit|preventDefault={submit}>
    <img class="logo" src={logo} alt="TakoIA" />
    <h1>TakoIA</h1>
    <h2>{$t("login.title")}</h2>
    <p class="muted small">{$t("login.subtitle")}</p>

    <label>{$t("login.username")}
      <input bind:value={username} autocomplete="username" />
    </label>
    <label>{$t("login.password")}
      <input type="password" bind:value={password} autocomplete="current-password" />
    </label>

    {#if error}<p class="err">{error}</p>{/if}
    <button class="primary" type="submit" disabled={busy || !password}>{$t("login.submit")}</button>

    <div class="lang">
      <button type="button" class:active={$locale === "fr"} on:click={() => setLocale("fr")}>FR</button>
      <button type="button" class:active={$locale === "en"} on:click={() => setLocale("en")}>EN</button>
    </div>
  </form>
</div>

<style>
  .wrap { position: fixed; inset: 0; display: flex; align-items: center; justify-content: center; overflow: hidden; }
  .bg { position: absolute; inset: 0; width: 100%; height: 100%; z-index: 0; }
  .box { position: relative; z-index: 1; background: color-mix(in srgb, var(--panel) 78%, transparent); border: 1px solid var(--border); border-radius: 16px; padding: 2rem; width: 360px; max-width: 90vw; text-align: center; backdrop-filter: blur(10px); box-shadow: 0 20px 60px rgba(0, 0, 0, 0.45); }
  .logo { width: 92px; height: 92px; border-radius: 50%; box-shadow: 0 0 30px color-mix(in srgb, var(--accent) 45%, transparent); }
  h1 { margin: 0.6rem 0 0; font-size: 1.7rem; letter-spacing: 0.04em; }
  h2 { margin: 0.5rem 0 0.2rem; font-size: 1.05rem; }
  label { display: block; font-size: 0.85rem; color: var(--muted); margin-top: 0.9rem; text-align: left; }
  input { width: 100%; background: var(--bg); border: 1px solid var(--border); color: var(--text); border-radius: 8px; padding: 0.55rem 0.7rem; font: inherit; margin-top: 0.25rem; }
  button.primary { width: 100%; margin-top: 1.2rem; background: var(--accent); border: 1px solid var(--accent); color: #04231a; font-weight: 600; border-radius: 8px; padding: 0.6rem; cursor: pointer; font: inherit; }
  button.primary:disabled { opacity: 0.5; cursor: default; }
  .err { color: var(--err); font-size: 0.85rem; margin: 0.6rem 0 0; }
  .small { font-size: 0.8rem; }
  .lang { display: flex; gap: 0.4rem; justify-content: center; margin-top: 1rem; }
  .lang button { background: transparent; border: 1px solid var(--border); color: var(--muted); border-radius: 6px; padding: 0.2rem 0.6rem; cursor: pointer; font: inherit; font-size: 0.78rem; }
  .lang button.active { background: color-mix(in srgb, var(--accent) 20%, transparent); color: var(--text); }
</style>
