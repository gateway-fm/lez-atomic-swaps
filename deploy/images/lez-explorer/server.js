// Minimal LEZ v0.2 explorer: static UI + JSON-RPC proxy to the indexer.
// No dependencies; Node >= 18.
const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");

const INDEXER = process.env.LEZ_INDEXER_URL || "http://indexer:8779";
const PORT = Number(process.env.LEZ_EXPLORER_PORT || 3003);
const PUBLIC = path.join(__dirname, "public");
const EVIDENCE = process.env.M3_BTC_EVIDENCE_FILE || "/run/lez-evidence/m3-btc-ui-evidence.json";

const TYPES = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8", ".svg": "image/svg+xml" };

function indexerCall(method, params) {
  const body = JSON.stringify({ jsonrpc: "2.0", id: "explorer", method, params });
  return fetch(INDEXER, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body,
    signal: AbortSignal.timeout(4000),
  })
    .then((r) => r.json())
    .then((j) => {
      if (j.error) throw new Error(j.error.message || JSON.stringify(j.error));
      return j.result;
    });
}

function send(res, code, body, type) {
  const buf = typeof body === "string" ? Buffer.from(body) : body;
  res.writeHead(code, { "content-type": type || "text/plain; charset=utf-8", "content-length": buf.length });
  res.end(buf);
}

function loadEvidence() {
  if (!path.isAbsolute(EVIDENCE)) throw new Error("M3 Bitcoin evidence path is not absolute");
  const info = fs.lstatSync(EVIDENCE);
  if (!info.isFile() || info.isSymbolicLink() || info.size <= 0 || info.size > 262144) {
    throw new Error("M3 Bitcoin evidence file is unavailable or unsafe");
  }
  const raw = fs.readFileSync(EVIDENCE, "utf8");
  const evidence = JSON.parse(raw);
  const ids = evidence?.effects?.map((entry) => entry.transaction_id) ?? [];
  if (evidence?.kind !== "m3_btc_ui_evidence" || evidence?.result !== "passed"
      || evidence?.pair !== "Bitcoin" || evidence?.direction !== "TakerSellsForeign"
      || evidence?.terminal?.phase !== "completed" || evidence?.terminal?.revision !== 4
      || !Array.isArray(evidence?.effects) || evidence.effects.length !== 5
      || new Set(ids).size !== 5
      || evidence.effects.filter((entry) => entry.chain === "Bitcoin").length !== 2
      || evidence.effects.filter((entry) => entry.chain === "LEZ").length !== 3
      || !evidence.effects.every((entry) => ["Confirmed", "Finalized"].includes(entry.finality))
      || evidence.private_material_disclosed !== false) {
    throw new Error("M3 Bitcoin evidence failed its public schema checks");
  }
  return evidence;
}

async function api(req, res, url) {
  const route = url.pathname.replace(/^\/api\//, "");
  try {
    if (route === "evidence") {
      send(res, 200, JSON.stringify(loadEvidence()), "application/json");
      return;
    }
    const evidenceTx = route.match(/^evidence\/tx\/([0-9a-fA-F]{64})$/);
    if (evidenceTx) {
      const evidence = loadEvidence();
      const effect = evidence.effects.find((entry) => entry.transaction_id === evidenceTx[1]);
      if (!effect) return send(res, 404, '{"error":"transaction is not in the certified M3 run"}', "application/json");
      send(res, 200, JSON.stringify({
        source: evidence.source,
        run_id: evidence.run_id,
        completed_at: evidence.completed_at,
        repository_commit: evidence.repository_commit,
        terminal: evidence.terminal,
        effect,
      }), "application/json");
      return;
    }
    if (route === "overview") {
      const schema = await indexerCall("getSchema", []);
      const health = await indexerCall("checkHealth", []).catch(() => null);
      send(res, 200, JSON.stringify({ schema, health }), "application/json");
      return;
    }
    if (route === "blocks") {
      const count = Math.min(Number(url.searchParams.get("count") || 10), 50);
      const result = await indexerCall("getBlocks", [null, count]);
      send(res, 200, JSON.stringify(result), "application/json");
      return;
    }
    const m = route.match(/^block\/id\/(.+)$/);
    if (m) { send(res, 200, JSON.stringify(await indexerCall("getBlockById", [m[1]])), "application/json"); return; }
    const h = route.match(/^block\/hash\/([0-9a-fA-F]+)$/);
    if (h) { send(res, 200, JSON.stringify(await indexerCall("getBlockByHash", [h[1]])), "application/json"); return; }
    const t = route.match(/^tx\/(.+)$/);
    if (t) { send(res, 200, JSON.stringify(await indexerCall("getTransaction", [t[1]])), "application/json"); return; }
    const a = route.match(/^account\/([1-9A-HJ-NP-Za-km-z]{20,64})$/);
    if (a) { send(res, 200, JSON.stringify(await indexerCall("getAccount", [a[1]])), "application/json"); return; }
    send(res, 404, '{"error":"unknown api route"}', "application/json");
  } catch (e) {
    send(res, 502, JSON.stringify({ error: String(e.message || e) }), "application/json");
  }
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, "http://localhost");
  if (url.pathname.startsWith("/api/")) return api(req, res, url);
  let file = url.pathname === "/" ? "/index.html" : url.pathname;
  const full = path.normalize(path.join(PUBLIC, file));
  if (!full.startsWith(PUBLIC)) return send(res, 403, "forbidden");
  fs.readFile(full, (err, data) => {
    if (err) return send(res, 404, "not found");
    send(res, 200, data, TYPES[path.extname(full)] || "application/octet-stream");
  });
});

server.listen(PORT, "0.0.0.0", () => console.log(`lez-explorer on :${PORT} -> ${INDEXER}`));
