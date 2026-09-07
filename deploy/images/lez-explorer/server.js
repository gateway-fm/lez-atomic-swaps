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

const EVIDENCE_DIR = process.env.LEZ_EVIDENCE_DIR || "";
let runsCache = { at: 0, byTx: new Map() };

// Every completed swap the Nodes settled is exported as one evidence file
// (deploy/scripts/export-node-evidence.py). Index all of their transaction
// ids so any copied hash resolves, not just the latest swap's.
function evidenceByTx() {
  const now = Date.now();
  if (now - runsCache.at < 15000) return runsCache.byTx;
  const byTx = new Map();
  if (EVIDENCE_DIR) {
    let files = [];
    try { files = fs.readdirSync(EVIDENCE_DIR).filter((name) => name.endsWith(".json")).sort().slice(-50); } catch {}
    for (const name of files) {
      const file = path.join(EVIDENCE_DIR, name);
      try {
        const info = fs.lstatSync(file);
        if (!info.isFile() || info.size <= 0 || info.size > 262144) continue;
        const evidence = JSON.parse(fs.readFileSync(file, "utf8"));
        if (evidence?.kind !== "m3_btc_ui_evidence" || !Array.isArray(evidence.effects)) continue;
        for (const effect of evidence.effects) {
          if (typeof effect?.transaction_id === "string" && /^[0-9a-f]{64}$/.test(effect.transaction_id)) {
            byTx.set(effect.transaction_id, {
              source: evidence.source,
              run_id: evidence.run_id,
              completed_at: evidence.completed_at,
              repository_commit: evidence.repository_commit,
              terminal: evidence.terminal,
              effect,
            });
          }
        }
      } catch {}
    }
  }
  runsCache = { at: now, byTx };
  return byTx;
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
      const wanted = evidenceTx[1].toLowerCase();
      let proof = null;
      try {
        const evidence = loadEvidence();
        const effect = evidence.effects.find((entry) => entry.transaction_id === wanted);
        if (effect) {
          proof = {
            source: evidence.source,
            run_id: evidence.run_id,
            completed_at: evidence.completed_at,
            repository_commit: evidence.repository_commit,
            terminal: evidence.terminal,
            effect,
          };
        }
      } catch {}
      if (!proof) proof = evidenceByTx().get(wanted) ?? null;
      if (!proof) return send(res, 404, '{"error":"transaction is not in any certified M3 run"}', "application/json");
      send(res, 200, JSON.stringify(proof), "application/json");
      return;
    }
    if (route === "overview") {
      const schema = await indexerCall("getSchema", []).catch(() => null);
      // The indexer's own checkHealth probes a "breakpoint" DB key this
      // deployment never writes; the chain head is the honest health signal.
      const health = await indexerCall("getBlocks", [null, 1])
        .then((blocks) => {
          const head = Array.isArray(blocks) && blocks[0] ? blocks[0] : null;
          if (!head?.header) return null;
          return {
            latest_block: head.header.block_id ?? null,
            timestamp: head.header.timestamp ?? null,
            bedrock_status: head.bedrock_status ?? null,
          };
        })
        .catch(() => null);
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
    if (m) {
      // The indexer wants an integer block id; a numeric string is rejected
      // with "Invalid params".
      const blockId = /^[0-9]+$/.test(m[1]) ? Number(m[1]) : m[1];
      send(res, 200, JSON.stringify(await indexerCall("getBlockById", [blockId])), "application/json");
      return;
    }
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
