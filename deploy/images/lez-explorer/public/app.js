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
    status.textContent = health
      ? `indexer healthy · block ${health.latest_block}${health.bedrock_status ? ` · ${String(health.bedrock_status).toLowerCase()}` : ""}`
      : "indexer degraded";
  } catch (e) { status.textContent = `indexer unreachable (${e.message})`; }
}

function blockTime(ts) {
  if (ts == null) return "—";
  const d = new Date(Number(ts));
  return Number.isNaN(d.getTime()) ? String(ts) : d.toISOString().replace("T", " ").slice(0, 19) + "Z";
}

function txHash(entry) {
  if (entry == null) return null;
  const inner = entry.Public ?? entry.Private ?? entry;
  return inner?.hash ?? null;
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
  content.append(table(["Block", "Hash", "Time", "Txs", "Finality", "Details"], blocks.map((b) => {
    const header = b.header ?? b;
    const id = header.block_id ?? b.block_id ?? b.id ?? b.blockId;
    const txs = b.body?.transactions?.length ?? 0;
    const details = link("view", () => location.hash = `#/block/id/${id}`);
    return [
      String(id ?? "?"),
      link(shorten(header.hash, 10), () => location.hash = `#/block/hash/${header.hash}`),
      blockTime(header.timestamp),
      String(txs),
      String(b.bedrock_status ?? "—"),
      details,
    ];
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
  const header = flat?.header;
  if (!header) {
    content.append(kv(Object.entries(flat ?? { error: "empty response" }).map(([k, v]) => [k, typeof v === "object" ? JSON.stringify(v) : v])));
    return;
  }
  const txs = flat.body?.transactions ?? [];
  content.append(kv([
    ["Block", header.block_id],
    ["Hash", header.hash],
    ["Previous hash", header.prev_block_hash],
    ["Time", blockTime(header.timestamp)],
    ["Finality", flat.bedrock_status],
    ["Transactions", txs.length],
    ["Signature", shorten(header.signature, 20)],
  ]));
  if (txs.length) {
    content.append(table(["#", "Transaction", "Program", "Accounts"], txs.map((entry, i) => {
      const inner = entry.Public ?? entry.Private ?? entry;
      const hash = txHash(entry);
      return [
        String(i + 1),
        hash ? link(shorten(hash, 12), () => location.hash = `#/tx/${hash}`) : "—",
        shorten(inner?.message?.program_id, 10),
        String(inner?.message?.account_ids?.length ?? 0),
      ];
    })));
  }
}

function renderProof(proof) {
  const effect = proof.effect;
  content.append(kv([
    ["Source", proof.source], ["Run", proof.run_id], ["Terminal", `revision ${proof.terminal.revision} · ${proof.terminal.phase}`],
    ["Sequence", effect.sequence], ["Chain", effect.chain], ["Actor", effect.actor], ["Effect", effect.label],
    ["Transaction ID", effect.transaction_id], ["Amount", effect.amount], ["Finality", effect.finality],
    ["Confirmations", effect.confirmations], ["Block height", effect.block_height], ["Block hash", effect.block_hash],
  ]));
  const note = document.createElement("div");
  note.className = "card dim";
  note.textContent = "This transaction executed on the run's isolated chains; the certified evidence above is its public record.";
  content.append(note);
}

async function lookupView(hash) {
  content.replaceChildren();
  const loading = document.createElement("div"); loading.className = "card dim"; loading.textContent = "Resolving…";
  content.append(loading);
  try {
    const block = await api(`block/hash/${hash}`);
    if (block?.header || block?.block?.header) { location.hash = `#/block/hash/${hash}`; return; }
  } catch {}
  location.hash = `#/tx/${hash}`;
}

async function txView(id) {
  content.replaceChildren();
  const loading = document.createElement("div"); loading.className = "card dim"; loading.textContent = "Loading transaction…";
  content.append(loading);
  let tx = null;
  try { tx = await api(`tx/${id}`); } catch {}
  if (tx == null || (!tx.Public && !tx.Private && !tx.message)) {
    let proof = null;
    try { proof = await api(`evidence/tx/${id}`); } catch {}
    loading.remove();
    if (proof) { renderProof(proof); return; }
    const missing = document.createElement("div");
    missing.className = "card error";
    missing.textContent = "Not found on the live chain and not in any certified M3 run.";
    content.append(missing);
    return;
  }
  loading.remove();
  const kind = tx?.Public ? "Public" : tx?.Private ? "Private" : null;
  const inner = kind ? tx[kind] : tx;
  if (!inner?.message) {
    content.append(kv(Object.entries(tx ?? { error: "empty response" }).map(([k, v]) => [k, typeof v === "object" ? JSON.stringify(v) : v])));
    return;
  }
  const witnesses = inner.witness_set?.signatures_and_public_keys ?? [];
  content.append(kv([
    ["Transaction", inner.hash],
    ["Visibility", kind],
    ["Program", inner.message.program_id],
    ["Accounts touched", inner.message.account_ids?.length ?? 0],
    ["Nonces", (inner.message.nonces ?? []).join(", ") || "—"],
    ["Instruction data", (inner.message.instruction_data ?? []).join(", ") || "—"],
    ["Signatures", witnesses.length],
    ["Proof", inner.witness_set?.proof == null ? "none" : "attached"],
  ]));
  const accountIds = inner.message.account_ids ?? [];
  if (accountIds.length) {
    content.append(table(["#", "Account"], accountIds.map((account, i) => [
      String(i + 1),
      link(account, () => location.hash = `#/account/${account}`),
    ])));
  }
}

async function evidenceView() {
  content.replaceChildren();
  const loading = document.createElement("div"); loading.className = "card dim"; loading.textContent = "Loading certified M3 Bitcoin evidence…";
  content.append(loading);
  let evidence;
  try { evidence = await api("evidence"); }
  catch (e) { loading.className = "card error"; loading.textContent = String(e.message || e); return; }
  loading.remove();
  content.append(kv([
    ["Run", evidence.run_id], ["Result", evidence.result], ["Pair", `${evidence.pair} / ${evidence.direction}`],
    ["Terminal", `revision ${evidence.terminal.revision} · ${evidence.terminal.phase}`],
    ["Effects", `${evidence.effect_counts.bitcoin} Bitcoin + ${evidence.effect_counts.lez} LEZ`],
    ["Completed", evidence.completed_at], ["Source commit", evidence.repository_commit],
  ]));
  content.append(table(["#", "Chain / effect", "Transaction", "Finality", "Block"], evidence.effects.map((effect) => [
    String(effect.sequence),
    `${effect.chain} · ${effect.label}`,
    link(shorten(effect.transaction_id, 10), () => location.hash = `#/evidence/tx/${effect.transaction_id}`),
    effect.finality,
    effect.block_height == null ? shorten(effect.block_hash, 8) : String(effect.block_height),
  ])));
}

async function evidenceTxView(id) {
  content.replaceChildren();
  const loading = document.createElement("div"); loading.className = "card dim"; loading.textContent = "Loading transaction proof…";
  content.append(loading);
  let proof;
  try { proof = await api(`evidence/tx/${id}`); }
  catch (e) { loading.className = "card error"; loading.textContent = String(e.message || e); return; }
  loading.remove();
  const effect = proof.effect;
  content.append(kv([
    ["Source", proof.source], ["Run", proof.run_id], ["Terminal", `revision ${proof.terminal.revision} · ${proof.terminal.phase}`],
    ["Sequence", effect.sequence], ["Chain", effect.chain], ["Actor", effect.actor], ["Effect", effect.label],
    ["Transaction ID", effect.transaction_id], ["Amount", effect.amount], ["Finality", effect.finality],
    ["Confirmations", effect.confirmations], ["Block height", effect.block_height], ["Block hash", effect.block_hash],
  ]));
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
  if (parts[0] === "evidence" && parts[1] === "tx") return evidenceTxView(decodeURIComponent(parts[2] || ""));
  if (parts[0] === "evidence") return evidenceView();
  if (parts[0] === "block" && parts[1] === "id") return blockView("id", decodeURIComponent(parts[2] || ""));
  if (parts[0] === "block" && parts[1] === "hash") return blockView("hash", decodeURIComponent(parts[2] || ""));
  if (parts[0] === "lookup") return lookupView(decodeURIComponent(parts[1] || ""));
  if (parts[0] === "tx") return txView(decodeURIComponent(parts[1] || ""));
  if (parts[0] === "account") return accountView(decodeURIComponent(parts[1] || ""));
  blocksView();
}

$("#search").addEventListener("keydown", (e) => {
  if (e.key !== "Enter") return;
  const q = e.target.value.trim();
  if (!q) return;
  if (/^[0-9]+$/.test(q)) location.hash = `#/block/id/${q}`;
  else if (/^[0-9a-fA-F]{64}$/.test(q)) location.hash = `#/lookup/${q}`;
  else if (/^[1-9A-HJ-NP-Za-km-z]{40,64}$/.test(q)) location.hash = `#/account/${q}`;
  else location.hash = `#/tx/${q}`;
});

document.querySelectorAll("nav button").forEach((b) => b.addEventListener("click", () => (location.hash = `#/${b.dataset.view}`)));
window.addEventListener("hashchange", route);
overviewStatus();
setInterval(overviewStatus, 10000);
route();
