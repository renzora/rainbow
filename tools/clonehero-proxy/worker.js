// A Cloudflare Worker for the web version of Rainbow.
//
// 1. Clone Hero's leaderboards. Clone Hero's API only answers requests from its own website, so
//    a page on another site (like Rainbow on GitHub Pages) can't call it directly. This forwards
//    read-only requests to the API (`/v1/...`) and adds the header that lets Rainbow's pages read
//    the answer.
// 2. Encore's newest charts (`/encore/newest`). Every visitor checks for new charts every few
//    minutes; this keeps Encore's newest page for a few minutes, so they all share one request to
//    Encore however many visitors there are.
//
// It only accepts requests from the origins below, and only those two kinds of GET, so it can't
// be used as a general proxy.
//
// Deploy: see README.md next to this file. Then build the web version with
// RAINBOW_CH_PROXY=https://<your worker>.workers.dev/v1

const API = "https://api.clonehero.net";
const ENCORE_SEARCH = "https://api.enchor.us/search";

const ALLOWED_ORIGINS = [
  "https://renzora.github.io",
  "http://localhost:8765",
  "http://127.0.0.1:8765",
];

// How long answers are kept at Cloudflare's edge.
const LEADERBOARD_SECONDS = 60;
const NEWEST_SECONDS = 300;

function cors(origin) {
  return {
    "Access-Control-Allow-Origin": origin,
    "Access-Control-Allow-Methods": "GET, OPTIONS",
    "Access-Control-Max-Age": "86400",
    "Vary": "Origin",
  };
}

// Encore's newest page of charts: the same search Rainbow's catalogue update makes.
function fetchNewest() {
  return fetch(ENCORE_SEARCH, {
    method: "POST",
    headers: { "Content-Type": "application/json", "User-Agent": "rainbow-web-proxy" },
    body: JSON.stringify({
      search: "*",
      per_page: 250,
      page: 1,
      instrument: null,
      difficulty: null,
      drumType: null,
      drumsReviewed: false,
      sort: null,
      source: "bridge",
    }),
  });
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
    const newest = url.pathname === "/encore/newest";
    if (request.method !== "GET" || !(newest || url.pathname.startsWith("/v1/"))) {
      return new Response("Not found", { status: 404, headers: cors(origin) });
    }

    const cache = caches.default;
    const key = new Request(newest ? ENCORE_SEARCH + "?newest" : API + url.pathname + url.search);
    let answer = await cache.match(key);
    if (!answer) {
      const upstream = newest
        ? await fetchNewest()
        : await fetch(key, { headers: { "Accept": "application/json", "User-Agent": "rainbow-web-proxy" } });
      // (Encore answers searches with 201 Created; it's kept as an ordinary 200.)
      answer = new Response(upstream.body, { status: upstream.ok ? 200 : upstream.status, headers: upstream.headers });
      answer.headers.delete("Set-Cookie");
      if (upstream.ok) {
        answer.headers.set("Cache-Control", `public, max-age=${newest ? NEWEST_SECONDS : LEADERBOARD_SECONDS}`);
        ctx.waitUntil(cache.put(key, answer.clone()));
      }
    }
    const out = new Response(answer.body, answer);
    for (const [k, v] of Object.entries(cors(origin))) out.headers.set(k, v);
    return out;
  },
};
