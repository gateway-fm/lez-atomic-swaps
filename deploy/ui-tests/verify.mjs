// End-to-end UI verification against the REAL maker daemon / taker service.
// Mirrors apps/basecamp/tests/basecamp-role-product.mjs from the repo, but
// self-contained: spawns Basecamp offscreen with a fresh copy of the role
// user dir, drives it through the QML inspector, and asserts live RPC results.
//
// Run inside the basecamp-ui container:
//   docker exec lez-basecamp-ui node /ui-tests/verify.mjs [maker|taker]
import { spawn } from "node:child_process";
import net from "node:net";
import { cpSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const framework = "/opt/qt-mcp/test-framework/framework.mjs";
const { test, run } = await import(framework);

const role = process.argv[2] === "taker" ? "taker" : "maker";

const freshUserDir = mkdtempSync(join(tmpdir(), `lez-verify-${role}-`));
// both plugins in one app: the product shape (maker + taker in the sidebar)
cpSync(`/var/lez-assets/both-user`, freshUserDir, { recursive: true });
process.env.BASECAMP_USER_DIR = freshUserDir;
const appBin = "/usr/local/bin/basecamp-maker";

// spawn the app ourselves (the framework's --ci mode waits only 15s; cold
// module loading needs longer), then attach in normal mode
const appProcess = spawn(appBin, ["-platform", "offscreen"], {
  stdio: ["ignore", "ignore", "inherit"],
  env: { ...process.env, QT_QPA_PLATFORM: "offscreen", QT_FORCE_STDERR_LOGGING: "1" },
});

const inspectorPort = Number(process.env.QML_INSPECTOR_PORT || 3768);
async function waitInspector(ms) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    const ok = await new Promise((resolve) => {
      const s = net.createConnection({ host: "127.0.0.1", port: inspectorPort });
      s.once("connect", () => { s.destroy(); resolve(true); });
      s.once("error", () => resolve(false));
    });
    if (ok) return;
    await new Promise((r) => setTimeout(r, 1000));
  }
  throw new Error(`inspector did not appear on :${inspectorPort} within ${ms}ms`);
}

await waitInspector(90000);
await new Promise((r) => setTimeout(r, 2000));

async function property(app, objectName, propertyName) {
  const found = await app.findByProperty("objectName", objectName);
  if (found.error || !found.matches || found.matches.length !== 1) {
    throw new Error(`expected exactly one ${objectName}, got ${JSON.stringify(found)}`);
  }
  const response = await app.getProperties(found.matches[0].id);
  if (response.error) throw new Error(response.error);
  const value = response.properties.find((e) => e.name === propertyName);
  if (!value) throw new Error(`${objectName}.${propertyName} is unavailable`);
  return value.value;
}

async function evaluateIn(app, objectId, expression) {
  return app.inspector.send("evaluate", { objectId, expression });
}

async function outputAfterClick(app, buttonLabel, objectNameOutput) {
  const before = await property(app, objectNameOutput, "text");
  await app.click(buttonLabel);
  const deadline = Date.now() + 45000;
  for (;;) {
    await new Promise((r) => setTimeout(r, 700));
    const raw = await property(app, objectNameOutput, "text");
    if (raw !== before && !raw.startsWith("Waiting for owner-local service")) return raw;
    if (Date.now() > deadline) throw new Error(`${buttonLabel} did not complete (last: ${raw})`);
  }
}

function unwrap(raw, what) {
  const envelope = JSON.parse(raw);
  if (envelope.ok !== true) throw new Error(`${what} failed: ${raw}`);
  return envelope.result ?? {};
}

if (role === "maker") {
  test("maker: launcher discoverable and app opens", async (app) => {
    await app.waitFor(async () => app.expectTexts(["LEZ Atomic Swap Maker"]), {
      timeout: 25000, interval: 500, description: "package discovery",
    });
    await app.click("LEZ Atomic Swap Maker");
    await app.waitFor(async () => app.expectTexts(["LEZ Atomic Swap — Maker Console", "Backend connected"]), {
      timeout: 25000, interval: 500, description: "maker view + live backend",
    });
  });

  test("maker: real daemon health", async (app) => {
    const health = unwrap(await outputAfterClick(app, "Check service", "makerOutput"), "health");
    if (health.ready !== true || health.degraded !== false) {
      throw new Error(`unexpected health: ${JSON.stringify(health)}`);
    }
    console.log(`  health: ready=true degraded=false routes=${(health.routes ?? []).length}`);
  });

  test("maker: atomic route save + history on the real database", async (app) => {
    const saved = unwrap(await outputAfterClick(app, "Save route atomically", "makerOutput"), "route save");
    const history = unwrap(await outputAfterClick(app, "Refresh swap history", "makerOutput"), "history");
    const swaps = Array.isArray(history) ? history : (history.swaps ?? []);
    console.log(`  route save ok (revisions: ${JSON.stringify(saved.pair_revision ?? saved)}) swaps=${swaps.length}`);
    for (const swap of swaps) {
      console.log(`  swap ${swap.id.slice(0, 16)}… pair=${swap.pair} phase=${swap.phase}`);
    }
  });

  test("maker: monitor the completed on-chain swap", async (app) => {
    const found = await app.findByProperty("placeholderText", "Swap ID");
    if (found.error || !found.matches || found.matches.length !== 1) {
      throw new Error(`swap id field not found: ${JSON.stringify(found).slice(0, 200)}`);
    }
    await evaluateIn(app, found.matches[0].id, `text = "${process.env.REAL_SWAP_ID ?? "a8d37797caacc1c68da23a91061de5bc31c2d991b301ae48ff864a669e510edc"}"`);
    const monitor = unwrap(await outputAfterClick(app, "Monitor", "makerOutput"), "monitor");
    const swapId = monitor.swap_id ?? monitor.swap?.id ?? monitor.id;
    if (!String(swapId ?? "").startsWith((process.env.REAL_SWAP_ID ?? "a8d37797").slice(0, 8))) {
      throw new Error(`unexpected monitor result: ${JSON.stringify(monitor).slice(0, 200)}`);
    }
    console.log(`  monitored live: actor=${monitor.actor_kind} state=${monitor.schedule_state} swap=${String(swapId).slice(0, 16)}…`);
  });
} else {
  test("taker: launcher discoverable and app opens", async (app) => {
    await app.waitFor(async () => app.expectTexts(["LEZ Atomic Swap Taker"]), {
      timeout: 25000, interval: 500, description: "package discovery",
    });
    await app.click("LEZ Atomic Swap Taker");
    await app.waitFor(async () => app.expectTexts(["LEZ Atomic Swap — Taker Route", "Backend connected"]), {
      timeout: 25000, interval: 500, description: "taker view + live service",
    });
  });

  test("taker: real service health", async (app) => {
    const health = unwrap(await outputAfterClick(app, "Service health", "takerOutput"), "health");
    if (health.ready !== true) throw new Error(`unexpected health: ${JSON.stringify(health)}`);
    console.log(`  health: ready=true delivery=${health.delivery}`);
  });

  test("taker: signed offer -> initiate -> monitor", async (app) => {
    // pick the pair carrying a live offer: ZEC when prepared swaps are armed
    // (env REAL_ZEC=1), Bitcoin otherwise
    const zecFirst = process.env.REAL_ZEC === "1";
    const namedPair = await app.findByProperty("objectName", "takerPair");
    const combos = await app.findByProperty("displayText", "Zcash");
    const combo = (namedPair.matches ?? [])[0]
      ?? (combos.matches ?? []).find((m) => String(m.type ?? "").includes("Combo"));
    if (!combo) throw new Error("pair ComboBox not found");
    await evaluateIn(app, combo.id, `currentIndex = ${zecFirst ? 0 : 1}`);
    const namedDirection = await app.findByProperty("objectName", "takerDirection");
    const dirs = await app.findByProperty("displayText", "TakerSellsLez");
    const dirCombo = (namedDirection.matches ?? [])[0]
      ?? (dirs.matches ?? []).find((m) => String(m.type ?? "").includes("Combo"));
    if (dirCombo) await evaluateIn(app, dirCombo.id, `currentIndex = ${zecFirst ? 0 : 1}`);
    const listed = unwrap(await outputAfterClick(app, "Browse authenticated offers", "takerOutput"), "offer list");
    const offers = (listed.offers ?? []).map((entry) => entry.offer ?? entry);
    const wanted = process.env.REAL_ZEC === "1" ? process.env.REAL_OFFER_ID : "offer-ui-btc-001";
    const candidates = process.env.REAL_ZEC === "1"
      ? offers.filter((o) => !wanted || o.id === wanted)
      : offers.filter((o) => o.id === wanted);
    const match = candidates.sort((a, b) =>
      Number(b.created_at_unix_seconds ?? 0) - Number(a.created_at_unix_seconds ?? 0))[0];
    if (!match) throw new Error(`live offer not listed (${wanted}): ${JSON.stringify(offers).slice(0, 200)}`);
    console.log(`  live offer ${match.id}: pair=${match.pair_configuration.route.pair} ttl=${match.pair_configuration.offer_ttl_seconds}s`);

    // fill the review form with the offer's exact facts
    const digest = (listed.offers ?? []).find((e) => (e.offer ?? e).id === match.id);
    const envelopeSha = Array.isArray(digest?.signed_envelope_sha256)
      ? digest.signed_envelope_sha256.map((b) => Number(b).toString(16).padStart(2, "0")).join("")
      : String(digest?.signed_envelope_sha256 ?? "");
    const identity = digest?.maker_identity ?? digest?.maker_public_key ?? "";
    console.log(`  review facts: identity=${identity.slice(0, 12)}… sha=${envelopeSha.slice(0, 12)}…`);
    const sets = [
      ["takerOfferId", match.id],
      ["takerMakerIdentity", identity],
      ["takerEnvelopeDigest", envelopeSha],
      ["takerForeignUnits", String(process.env.REAL_FOREIGN_UNITS ?? "10000")],
      ["takerLezUnits", String(process.env.REAL_LEZ_UNITS ?? "25000")],
    ];
    for (const [objectName, value] of sets) {
      const found = await app.findByProperty("objectName", objectName);
      if (found.matches?.length === 1) await evaluateIn(app, found.matches[0].id, `text = ${JSON.stringify(value)}`);
    }

    // REAL Maker Chat acceptance + durable actor provisioning
    const initiation = unwrap(await outputAfterClick(app, "Confirm and initiate", "takerOutput"), "initiate");
    const initiated = initiation.swap ?? initiation;
    console.log(`  initiated: state=${initiated.state ?? initiated.swap_state} replay=${initiation.was_replay} swap=${String(initiated.swap_id ?? "").slice(0, 16)}…`);

    const swaps = unwrap(await outputAfterClick(app, "List my swaps", "takerOutput"), "swap list");
    const list = Array.isArray(swaps) ? swaps : (swaps.swaps ?? []);
    if (!list.some((entry) => String(entry.swap_id ?? entry.id ?? "").startsWith(String(initiated.swap_id ?? "?").slice(0, 8)))) {
      throw new Error(`admitted swap not listed: ${JSON.stringify(list).slice(0, 200)}`);
    }
    console.log(`  swap list shows the admitted swap (${list.length} total)`);

    const swapField = await app.findByProperty("objectName", "takerSwapId");
    if (swapField.matches?.length !== 1) throw new Error("swap ID field not found");
    await evaluateIn(app, swapField.matches[0].id, `text = ${JSON.stringify(initiated.swap_id)}`);
    const monitored = unwrap(await outputAfterClick(app, "Monitor", "takerOutput"), "monitor");
    if (monitored.swap_id !== initiated.swap_id || monitored.state !== "not_activated") {
      throw new Error(`unexpected monitor result: ${JSON.stringify(monitored).slice(0, 200)}`);
    }
    console.log(`  monitor: state=${monitored.state} generation=${monitored.progress_generation}`);
  });
}

process.on("exit", () => {
  try { appProcess.kill("SIGTERM"); } catch {}
  try { rmSync(freshUserDir, { recursive: true, force: true }); } catch {}
});

await run();
