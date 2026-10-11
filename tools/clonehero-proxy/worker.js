// A Cloudflare Worker that lets the web version of Rainbow read Clone Hero's leaderboards.
//
// Clone Hero's API only answers requests from its own website, so a page on another site (like
// Rainbow on GitHub Pages) can't call it directly. This forwards read-only requests to the API
// and adds the header that lets Rainbow's pages read the answer. It only accepts requests from
// the origins below, only GETs, and only API paths, so it can't be used as a general proxy.
//
// Deploy: see README.md next to this file. Then build the web version with
// RAINBOW_CH_PROXY=https://<your worker>.workers.dev/v1

const API = "https://api.clonehero.net";

const ALLOWED_ORIGINS = [
  "https://renzora.github.io",
  "http://localhost:8765",
  "http://127.0.0.1:8765",
];

// Answers are cached at Cloudflare's edge for this long, so many visitors don't mean many
// requests to Clone Hero.
const CACHE_SECONDS = 60;

function cors(origin) {
  return {
    "Access-Control-Allow-Origin": origin,
    "Access-Control-Allow-Methods": "GET, OPTIONS",
    "Access-Control-Max-Age": "86400",
    "Vary": "Origin",
  };
}

export default {
  async fetch(request, env, ctx) {
    const origin = request.headers.get("Origin") || "";
    if (!ALLOWED_ORIGINS.includes(origin)) {
      return new Response("Not allowed", { status: 403 });
    }
    if (request.method === "OPTIONS") {
      return new Response(null, { status: 204, headers: cors(origin) });
    }
    const url = new URL(request.url);
    if (request.method !== "GET" || !url.pathname.startsWith("/v1/")) {
      return new Response("Not found", { status: 404, headers: cors(origin) });
    }

    const cache = caches.default;
    const key = new Request(API + url.pathname + url.search);
    let answer = await cache.match(key);
    if (!answer) {
      const upstream = await fetch(key, {
        headers: { "User-Agent": "rainbow-web-proxy" },
      });
      answer = new Response(upstream.body, upstream);
      answer.headers.delete("Set-Cookie");
      if (upstream.ok) {
        answer.headers.set("Cache-Control", `public, max-age=${CACHE_SECONDS}`);
        ctx.waitUntil(cache.put(key, answer.clone()));
      }
    }
    const out = new Response(answer.body, answer);
    for (const [k, v] of Object.entries(cors(origin))) out.headers.set(k, v);
    return out;
  },
};
