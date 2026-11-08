// Strata service worker: offline support for the installed app.
//
// - index.html (navigations): network first, so a new build is picked up on
//   the next load; falls back to the cached copy offline.
// - Everything else on this origin: cache first. Trunk hashes file names, so
//   a cached file never goes stale; new builds simply use new names.
// - The deploy workflow stamps VERSION; a changed worker activates at once
//   and tells open pages that a reload will pick up the new version.

const VERSION = "__STRATA_BUILD__";
const CACHE = `strata-${VERSION}`;
const SHELL = ["./", "./manifest.webmanifest", "./icons/icon-192.png", "./icons/icon-512.png", "./icons/icon.svg"];
// Matches "./name-hash.ext" references, quoted or not (release HTML is minified).
const ASSET_RE = /\.\/[A-Za-z0-9_\-./]+\.(?:js|mjs|wasm|css|png|svg|webmanifest)/g;

self.addEventListener("install", (event) => {
  event.waitUntil(
    (async () => {
      const cache = await caches.open(CACHE);
      try {
        await cache.addAll(SHELL);
        // Precache the hashed app files the current index.html points at.
        const html = await (await fetch("./", { cache: "no-cache" })).text();
        const urls = [...html.matchAll(ASSET_RE)]
          .map((m) => new URL(m[0], self.registration.scope))
          .filter((u) => u.origin === self.location.origin)
          .map((u) => u.href);
        await cache.addAll([...new Set(urls)]);
      } catch (err) {
        // Offline or partial deploy: runtime caching fills the gaps later.
        console.warn("Strata: precaching incomplete:", err);
      }
      await self.skipWaiting();
    })()
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      const keys = await caches.keys();
      const old = keys.filter((k) => k.startsWith("strata-") && k !== CACHE);
      await Promise.all(old.map((k) => caches.delete(k)));
      await self.clients.claim();
      if (old.length > 0) {
        const pages = await self.clients.matchAll({ type: "window" });
        for (const page of pages) page.postMessage({ type: "strata-update-ready", version: VERSION });
      }
    })()
  );
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  if (req.method !== "GET") return;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin) return;

  if (req.mode === "navigate") {
    event.respondWith(
      (async () => {
        const cache = await caches.open(CACHE);
        try {
          const fresh = await fetch(req);
          if (fresh.ok) cache.put("./", fresh.clone());
          return fresh;
        } catch (err) {
          return (await cache.match("./")) || (await cache.match(req)) || Response.error();
        }
      })()
    );
    return;
  }

  event.respondWith(
    (async () => {
      const cache = await caches.open(CACHE);
      const hit = await cache.match(req);
      if (hit) return hit;
      const res = await fetch(req);
      if (res.ok && res.type === "basic") cache.put(req, res.clone());
      return res;
    })()
  );
});
