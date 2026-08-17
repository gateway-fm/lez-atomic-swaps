const $ = (s) => document.querySelector(s);
const content = $("#content");
const status = $("#status");

async function api(route) {
  const r = await fetch(`/api/${route}`);
  const j = await r.json();
  if (!r.ok) throw new Error(j.error || r.status);
  return j;
}

function shorten(s, n = 14) {
  if (s == null) return "";
  s = String(s);
  return s.length <= n * 2 ? s : `${s.slice(0, n)}…${s.slice(-n)}`;
}

function kv(pairs) {
  const div = document.createElement("div");
  div.className = "card kv";
  for (const [k, v] of pairs) {
    const key = document.createElement("div"); key.className = "k"; key.textContent = k;
    const val = document.createElement("div"); val.textContent = v == null ? "—" : String(v);
    div.append(key, val);
  }
  return div;
}

function table(headers, rows) {
  const t = document.createElement("table");
  const thead = t.createTHead();
  const hr = thead.insertRow();
  for (const h of headers) { const th = document.createElement("th"); th.textContent = h; hr.append(th); }
  const tbody = t.createTBody();
  for (const row of rows) { const tr = tbody.insertRow(); for (const cell of row) { const td = tr.insertCell(); td.append(cell); } }
  return t;
}

function link(text, fn) {
  const a = document.createElement("a"); a.textContent = text; a.onclick = fn; return a;
}

async function overviewStatus() {
  try {
    const { health } = await api("overview");
    status.textContent = health ? `indexer healthy` : "indexer degraded";
  } catch (e) { status.textContent = `indexer unreachable (${e.message})`; }
}

async function blocksView() {
  content.replaceChildren();
  const wrap = document.createElement("div");
  wrap.className = "card dim";
  wrap.textContent = "Loading blocks…";
  content.append(wrap);
  let result;
  try { result = await api("blocks?count=25"); }
  catch (e) { wrap.className = "card error"; wrap.textContent = String(e.message || e); return; }
  wrap.remove();
  const blocks = Array.isArray(result) ? result : result?.blocks || result?.data || [];
  if (!blocks.length) {
    const empty = document.createElement("div");
    empty.className = "card dim";
    empty.textContent = "No blocks yet — the sequencer may still be producing the first slots.";
    content.append(empty);
    return;
  }
  content.append(table(["Block", "Details"], blocks.map((b) => {
    const id = b.block_id ?? b.id ?? b.blockId;
    const details = link("view", () => location.hash = `#/block/id/${id}`);
    return [String(id ?? "?"), details];
  })));
}

async function blockView(kind, value) {
  content.replaceChildren();
  const loading = document.createElement("div"); loading.className = "card dim"; loading.textContent = "Loading block…";
  content.append(loading);
  let block;
  try { block = await api(`block/${kind}/${value}`); }
  catch (e) { loading.className = "card error"; loading.textContent = String(e.message || e); return; }
  loading.remove();
  const flat = block?.block ?? block;
  content.append(kv(Object.entries(flat ?? { error: "empty response" }).map(([k, v]) => [k, typeof v === "object" ? JSON.stringify(v) : v])));
}

async function txView(id) {
  content.replaceChildren();
  const loading = document.createElement("div"); loading.className = "card dim"; loading.textContent = "Loading transaction…";
  content.append(loading);
  let tx;
  try { tx = await api(`tx/${id}`); }
  catch (e) { loading.className = "card error"; loading.textContent = String(e.message || e); return; }
  loading.remove();
  content.append(kv(Object.entries(tx ?? {}).map(([k, v]) => [k, typeof v === "object" ? JSON.stringify(v) : v])));
}

async function accountView(id) {
  content.replaceChildren();
  const loading = document.createElement("div"); loading.className = "card dim"; loading.textContent = "Loading account…";
  content.append(loading);
  let acc;
  try { acc = await api(`account/${id}`); }
  catch (e) { loading.className = "card error"; loading.textContent = String(e.message || e); return; }
  loading.remove();
  content.append(kv(Object.entries(acc ?? {}).map(([k, v]) => [k, typeof v === "object" ? JSON.stringify(v) : v])));
}

async function route() {
  const hash = location.hash || "#/blocks";
  const parts = hash.slice(2).split("/");
  document.querySelectorAll("nav button").forEach((b) => b.classList.toggle("active", parts[0] === b.dataset.view));
  if (parts[0] === "blocks") return blocksView();
  if (parts[0] === "block" && parts[1] === "id") return blockView("id", decodeURIComponent(parts[2] || ""));
  if (parts[0] === "block" && parts[1] === "hash") return blockView("hash", decodeURIComponent(parts[2] || ""));
  if (parts[0] === "tx") return txView(decodeURIComponent(parts[1] || ""));
  if (parts[0] === "account") return accountView(decodeURIComponent(parts[1] || ""));
  blocksView();
}

$("#search").addEventListener("keydown", (e) => {
  if (e.key !== "Enter") return;
  const q = e.target.value.trim();
  if (!q) return;
  if (/^[0-9]+$/.test(q)) location.hash = `#/block/id/${q}`;
  else if (/^[0-9a-fA-F]{64}$/.test(q)) location.hash = `#/block/hash/${q}`;
  else if (/^[1-9A-HJ-NP-Za-km-z]{40,64}$/.test(q)) location.hash = `#/account/${q}`;
  else location.hash = `#/tx/${q}`;
});

document.querySelectorAll("nav button").forEach((b) => b.addEventListener("click", () => (location.hash = `#/${b.dataset.view}`)));
window.addEventListener("hashchange", route);
overviewStatus();
setInterval(overviewStatus, 10000);
route();
