// Offline, and a second visit that does not fetch the whole app again.
//
// The build stamps `VERSION` with the crate version and the commit, which is
// what makes a deploy replace the cache rather than sit behind it: the name of
// the cache changes, `install` fills the new one, and `activate` deletes every
// older one. Without that stamp a service worker is the most reliable way
// there is to serve somebody last week's build forever.
const VERSION = "__VERSION__";
const CACHE = `mbrd-${VERSION}`;

// Everything the app is. The wasm is fifteen megabytes and is the reason this
// file exists at all; the rest is small enough that listing it costs nothing.
const SHELL = [
  "./",
  "./index.html",
  "./mbrd.js",
  "./mbrd_bg.wasm",
  "./manifest.webmanifest",
  "./icon.svg",
  "./icon-192.png",
  "./icon-512.png",
];

self.addEventListener("install", (event) => {
  // The new worker takes over at the next load rather than waiting for every
  // tab of the old one to close.
  self.skipWaiting();
  event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(SHELL)));
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((names) => Promise.all(names.filter((name) => name !== CACHE).map((name) => caches.delete(name))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const request = event.request;
  if (request.method !== "GET" || new URL(request.url).origin !== location.origin) return;

  // Cache first, and deliberately: every file here is versioned by the cache
  // name, so a hit is never stale — a new build is a new cache. What this buys
  // is a second visit that starts at once and a tab that survives a tunnel.
  event.respondWith(
    caches.match(request).then(
      (hit) =>
        hit ??
        fetch(request).then((response) => {
          if (response.ok && response.type === "basic") {
            const copy = response.clone();
            caches.open(CACHE).then((cache) => cache.put(request, copy));
          }
          return response;
        }),
    ),
  );
});
