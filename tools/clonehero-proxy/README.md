# Proxy for the web version

Clone Hero's leaderboard API (`api.clonehero.net`) only answers requests from
`leaderboards.clonehero.net`, so the web version of Rainbow can't call it straight from the
browser. `worker.js` is a small Cloudflare Worker that forwards Rainbow's read-only requests and
allows Rainbow's pages to read the answers. Without it, the web version simply leaves out the
Players and Leaderboards tabs and the leaderboard in the details panel.

It also shares Encore's newest charts between visitors: the web version checks for new charts
every few minutes, and the worker keeps Encore's newest page for five minutes, so Encore gets at
most one such request every five minutes however many people have Rainbow open. (Without the
worker, each visitor asks Encore directly.)

## Deploying it (free tier is plenty)

1. Sign in at <https://dash.cloudflare.com> and open **Workers & Pages → Create → Worker**.
2. Name it (e.g. `rainbow-clonehero`), deploy the hello-world, then **Edit code**, paste in
   `worker.js` and **Deploy**.
3. Note its address, e.g. `https://rainbow-clonehero.<your-subdomain>.workers.dev`.
4. In the GitHub repo, go to **Settings → Secrets and variables → Actions → Variables** and add
   a variable `CH_PROXY` set to that address followed by `/v1`, e.g.
   `https://rainbow-clonehero.<your-subdomain>.workers.dev/v1`.
5. Re-run the **Web** workflow (or push to `main`); the web version now includes the
   leaderboards.

Or with Wrangler: `npx wrangler deploy worker.js --name rainbow-clonehero --compatibility-date 2026-10-01`.

The worker only accepts GET requests to `/v1/...` and `/encore/newest` from the origins listed at the top of
`worker.js` (GitHub Pages, plus `localhost:8765` for testing), and caches answers for a minute
at Cloudflare's edge (five minutes for the newest charts) so many visitors don't mean many
requests to Clone Hero or Encore.
