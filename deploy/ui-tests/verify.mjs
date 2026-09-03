// End-to-end UI verification against the real Maker Node / Taker Node pair.
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
const uiDirection = process.env.M3_UI_DIRECTION || "TakerSellsForeign";
if (!["TakerSellsForeign", "TakerSellsLez"].includes(uiDirection)) {
  throw new Error("M3_UI_DIRECTION must be TakerSellsForeign or TakerSellsLez");
}
const reverseDirection = uiDirection === "TakerSellsLez";

const freshUserDir = mkdtempSync(join(tmpdir(), `lez-verify-${role}-`));
// both plugins in one app: the product shape (maker + taker in the sidebar)
cpSync(`/var/lez-assets/both-user`, freshUserDir, { recursive: true });
process.env.BASECAMP_USER_DIR = freshUserDir;
const appBin = `/usr/local/bin/lez-${role}-ui`;

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

async function outputAfterClick(
  app,
  buttonLabel,
  objectNameOutput,
  predicate = () => true,
  allowIdenticalResult = false,
) {
  const before = await property(app, objectNameOutput, "text");
  await app.click(buttonLabel);
  const deadline = Date.now() + 45000;
  for (;;) {
    await new Promise((r) => setTimeout(r, 700));
    const raw = await property(app, objectNameOutput, "text");
    if ((allowIdenticalResult || raw !== before)
        && !raw.startsWith("Waiting for owner-local Node")) {
      try {
        if (predicate(JSON.parse(raw))) return raw;
      } catch {}
    }
    if (Date.now() > deadline) throw new Error(`${buttonLabel} did not complete (last: ${raw})`);
  }
}

async function outputAfterSignal(app, objectName, objectNameOutput, predicate) {
  const before = await property(app, objectNameOutput, "text");
  const found = await app.findByProperty("objectName", objectName);
  if (found.error || found.matches?.length !== 1) {
    throw new Error(`expected exactly one ${objectName}, got ${JSON.stringify(found)}`);
  }
  await evaluateIn(app, found.matches[0].id, "clicked()");
  const deadline = Date.now() + 45000;
  for (;;) {
    await new Promise((r) => setTimeout(r, 700));
    const raw = await property(app, objectNameOutput, "text");
    if (!raw.startsWith("Waiting for owner-local Node")) {
      try {
        if (predicate(JSON.parse(raw))) return raw;
      } catch {}
    }
    if (Date.now() > deadline) throw new Error(`${objectName} did not complete (last: ${raw})`);
  }
}

async function triggerVisibleAction(app, objectName, expectedText, outputName, workingState) {
  let target = null;
  await app.waitFor(async () => {
    const found = await app.findByProperty("objectName", objectName);
    for (const match of found.matches ?? []) {
      const response = await app.getProperties(match.id);
      const values = Object.fromEntries((response.properties ?? []).map((entry) => [entry.name, entry.value]));
      if (values.visible === true && values.enabled === true && values.text === expectedText) {
        target = match.id;
        return;
      }
    }
    throw new Error(`${expectedText} is not ready`);
  }, { timeout: 600000, interval: 2000, description: `${expectedText} readiness` });
  await evaluateIn(app, target, "clicked()");
  await app.waitFor(async () => {
    const raw = await property(app, outputName, "text");
    const envelope = JSON.parse(raw);
    if (envelope.ok !== true
        || !(envelope.result?.swaps ?? []).some((swap) => swap.state === workingState)) {
      throw new Error(`${expectedText} has not entered ${workingState}`);
    }
  }, { timeout: 45000, interval: 700, description: `${expectedText} submission` });
}

function unwrap(raw, what) {
  const envelope = JSON.parse(raw);
  if (envelope.ok !== true) throw new Error(`${what} failed: ${raw}`);
  return envelope.result ?? {};
}

// Offer creation lives behind the "New offer" dialog. Signal-invoke the
// buttons by objectName — text-targeted synthetic clicks don't reliably
// reach controls under the offscreen platform.
async function publishOfferOnce(app, predicate) {
  const opener = await app.findByProperty("objectName", "makerNewOffer");
  if (opener.matches?.length !== 1) throw new Error("New offer button not found");
  await evaluateIn(app, opener.matches[0].id, "clicked()");
  await new Promise((r) => setTimeout(r, 600));
  const before = await property(app, "makerOutput", "text");
  const publish = await app.findByProperty("objectName", "makerCreateOffers");
  if (publish.matches?.length !== 1) throw new Error("Publish offer button not found");
  await evaluateIn(app, publish.matches[0].id, "clicked()");
  const deadline = Date.now() + 45000;
  for (;;) {
    await new Promise((r) => setTimeout(r, 700));
    const raw = await property(app, "makerOutput", "text");
    if (raw !== before && !raw.startsWith("Waiting for owner-local Node")) {
      try { if (predicate(JSON.parse(raw))) return raw; } catch {}
    }
    if (Date.now() > deadline) throw new Error(`publish did not complete (last: ${raw.slice(0, 200)})`);
  }
}

if (role === "maker") {
  test("maker: launcher discoverable and app opens", async (app) => {
    await app.waitFor(async () => app.expectTexts(["LEZ / BTC Maker"]), {
      timeout: 25000, interval: 500, description: "package discovery",
    });
    await app.click("LEZ / BTC Maker");
    await app.waitFor(async () => app.expectTexts(["LEZ / BTC — Maker Desk", "Backend connected"]), {
      timeout: 25000, interval: 500, description: "maker view + live backend",
    });
  });

  test("maker: real Node health", async (app) => {
    await app.click("Check Node");
    await app.waitFor(async () => app.expectTexts(["Maker systems ready"]), {
      timeout: 15000, interval: 300, description: "Maker health status",
    });
    console.log("  health: Maker systems ready");
  });

  test("maker: wallet-indexed BTC offer inventory", async (app) => {
    if (reverseDirection) {
      const sellLeg = await app.findByProperty("objectName", "makerSellLegLez");
      if (sellLeg.error || sellLeg.matches?.length !== 1) {
        throw new Error("Maker direction composer was not found");
      }
      await evaluateIn(app, sellLeg.matches[0].id, 'root.sellSide = "btc"');
    }
    const wallet = await app.findByProperty("objectName", "makerBtcWallet");
    if (wallet.matches?.length !== 1) {
      throw new Error("Maker wallet selector was not found");
    }
    await evaluateIn(app, wallet.matches[0].id, "currentIndex = 0");
    let munich = unwrap(await outputAfterClick(
      app, "Refresh wallet inventory", "makerOutput",
      (envelope) => envelope.ok === true
        && envelope.result?.selected_wallet_id === "maker-munich-01", true,
    ), "Munich inventory");
    let munichPending = (munich.inventory ?? []).filter((offer) => offer.state === "pending").length;
    while (munichPending < 3) {
      const target = munichPending + 1;
      munich = unwrap(await publishOfferOnce(
        app,
        (envelope) => envelope.ok === true
          && (envelope.result?.inventory ?? []).filter((offer) => offer.state === "pending").length >= target,
      ), "Munich offers");
      munichPending = (munich.inventory ?? []).filter((offer) => offer.state === "pending").length;
    }
    if (munichPending < 3) {
      throw new Error(`Munich wallet did not retain three pending offers: ${JSON.stringify(munich).slice(0, 500)}`);
    }
    await evaluateIn(app, wallet.matches[0].id, "currentIndex = 1");
    let basel = unwrap(await outputAfterClick(
      app, "Refresh wallet inventory", "makerOutput",
      (envelope) => envelope.ok === true
        && envelope.result?.selected_wallet_id === "maker-basel-02",
      true,
    ), "Basel inventory");
    let baselPending = (basel.inventory ?? []).filter((offer) => offer.state === "pending").length;
    while (baselPending < 2) {
      const target = baselPending + 1;
      basel = unwrap(await publishOfferOnce(
        app,
        (envelope) => envelope.ok === true
          && (envelope.result?.inventory ?? []).filter((offer) => offer.state === "pending").length >= target,
      ), "Basel offers");
      baselPending = (basel.inventory ?? []).filter((offer) => offer.state === "pending").length;
    }
    if (baselPending < 2
        || Number(basel.summary?.pending_offers ?? 0) < 5) {
      throw new Error(`wallet-indexed offer totals are wrong: ${JSON.stringify(basel).slice(0, 500)}`);
    }
    console.log(`  wallet inventory: Munich >=3 · Basel >=2 · market=${basel.summary.pending_offers}`);
  });

  const makerActions = reverseDirection
    ? { lock_btc: ["Lock 0.01000000 BTC", "locking_btc"], claim_lez: ["Claim 1,000 LEZ", "claiming_lez"] }
    : { fund_lez: ["Fund 1,000 LEZ", "funding_lez"], claim_btc: ["Claim Bitcoin", "claiming_btc"] };
  if (Object.hasOwn(makerActions, process.env.INTERACTIVE_ACTION)) {
    const action = process.env.INTERACTIVE_ACTION;
    const [label, working] = makerActions[action];
    test(`maker: perform ${action}`, async (app) => {
      const wallet = await app.findByProperty("objectName", "makerBtcWallet");
      if (wallet.error || wallet.matches?.length !== 1) throw new Error("Maker wallet selector is unavailable");
      await evaluateIn(app, wallet.matches[0].id, "currentIndex = 0; root.refreshBtcMarket(false)");
      await triggerVisibleAction(app, "makerSwapAction", label, "makerOutput", working);
      console.log(`  interactive M3: Maker ${action} submitted`);
    });
  }
} else {
  test("taker: launcher discoverable and app opens", async (app) => {
    await app.waitFor(async () => app.expectTexts(["LEZ / BTC Taker"]), {
      timeout: 25000, interval: 500, description: "package discovery",
    });
    await app.click("LEZ / BTC Taker");
    await app.waitFor(async () => app.expectTexts(["LEZ / BTC — Taker Desk", "Backend connected"]), {
      timeout: 25000, interval: 500, description: "taker view + live Node",
    });
    // Let the intentional one-shot evidence preload settle before a later
    // button assertion observes the shared diagnostic output field.
    await app.waitFor(async () => app.expectTexts(["REV 4 · COMPLETED"]), {
      timeout: 25000, interval: 500, description: "completed BTC evidence preload",
    });
  });

  test("taker: wallet-indexed BTC order book is ready", async (app) => {
    await app.expectTexts(["ACCOUNT", "My orders", "Available orders", "Zurich Wallet 01 · Taker"]);
    // The order book arrives with the first market snapshot after the view
    // opens; wait for the rendered rows instead of racing that request.
    await app.waitFor(async () => app.expectTexts(["0.01000000 BTC", "1,000 LEZ"]), {
      timeout: 15000, interval: 500, description: "first market snapshot rendered",
    });
    await app.click("Refresh wallet market");
    await app.waitFor(async () => app.expectTexts(["Munich Vault 01", "Basel Vault 02"]), {
      timeout: 15000, interval: 500, description: "multi-Maker order book",
    });
    console.log("  order book: both Maker wallets visible to the selected Taker wallet");
  });

  test("taker: real Node health", async (app) => {
    // Signal-invoke by objectName: text-targeted clicks do not reliably reach
    // controls under the offscreen platform once the order book has grown.
    const check = await app.findByProperty("objectName", "takerHealth");
    if (check.error || check.matches?.length !== 1) throw new Error("Check Node button is unavailable");
    await evaluateIn(app, check.matches[0].id, "clicked()");
    await app.waitFor(async () => app.expectTexts(["All systems ready"]), {
      timeout: 15000, interval: 300, description: "Taker health status",
    });
    console.log("  health: All systems ready");
  });

  test("taker: completed M3 BTC evidence is public, unique, and final", async (app) => {
    const evidence = unwrap(
      await outputAfterSignal(
        app, "takerRefreshProof", "takerOutput",
        (envelope) => envelope.ok === true
          && envelope.result?.kind === "m3_btc_ui_evidence",
      ),
      "BTC evidence",
    );
    const ids = evidence.effects.map((effect) => effect.transaction_id);
    const bitcoin = evidence.effects.filter((effect) => effect.chain === "Bitcoin");
    const lez = evidence.effects.filter((effect) => effect.chain === "LEZ");
    if (evidence.pair !== "Bitcoin" || evidence.direction !== uiDirection
        || evidence.terminal?.phase !== "completed" || evidence.terminal?.revision !== 4
        || evidence.private_material_disclosed !== false || evidence.replay_resubmission_count !== 0
        || ids.length !== 5 || new Set(ids).size !== 5 || bitcoin.length !== 2 || lez.length !== 3
        || !evidence.effects.every((effect) => ["Confirmed", "Finalized"].includes(effect.finality))) {
      throw new Error(`invalid M3 BTC evidence: ${JSON.stringify(evidence).slice(0, 500)}`);
    }
    console.log(`  M3 BTC: ${evidence.run_id} rev=${evidence.terminal.revision} effects=${bitcoin.length}+${lez.length}`);
    for (const effect of evidence.effects) {
      console.log(`  ${effect.sequence}. ${effect.chain} ${effect.kind}: ${effect.transaction_id.slice(0, 16)}… ${effect.finality}`);
    }
    if (evidence.wallet_balance_changes) {
      const wallets = evidence.wallet_balance_changes.wallets ?? [];
      if (wallets.length !== 2 || evidence.wallet_balance_changes.reconciliation?.lez_conserved !== true) {
        throw new Error(`invalid wallet balance reconciliation: ${JSON.stringify(evidence.wallet_balance_changes)}`);
      }
      console.log("  wallet ledger: opening/closing BTC + LEZ balances reconciled");
    }
  });

  if (process.env.PREPARE_INTERACTIVE_BTC === "1") {
    test("taker: taking one offer prepares the real Taker BTC action", async (app) => {
      const buttons = await app.findByProperty("objectName", "takerTakeOffer");
      if (buttons.error || !buttons.matches?.length) {
        throw new Error(`no takeable order-book row: ${JSON.stringify(buttons)}`);
      }
      await evaluateIn(app, buttons.matches[0].id, "clicked()");
      const firstAction = reverseDirection ? "Lock 1,000 LEZ" : "Lock 0.01000000 BTC";
      await app.waitFor(async () => app.expectTexts([firstAction]), {
        timeout: 600000,
        interval: 2000,
        description: "real M3 runner preparation and Taker lock gate",
      });
      console.log("  interactive M3: offer accepted · Taker BTC lock action ready");
    });
  }

  const takerActions = reverseDirection
    ? { lock_lez: ["Lock 1,000 LEZ", "locking_lez"], claim_btc: ["Claim Bitcoin", "claiming_btc"] }
    : { lock_btc: ["Lock 0.01000000 BTC", "locking_btc"], claim_lez: ["Claim 1,000 LEZ", "claiming_lez"] };
  if (Object.hasOwn(takerActions, process.env.INTERACTIVE_ACTION)) {
    const action = process.env.INTERACTIVE_ACTION;
    const [label, working] = takerActions[action];
    test(`taker: perform ${action}`, async (app) => {
      await triggerVisibleAction(app, "takerSwapAction", label, "takerOutput", working);
      console.log(`  interactive M3: Taker ${action} submitted`);
    });
  }

  if (process.env.REAL_ZEC === "1") {
  test("taker: signed offer -> initiate -> monitor", async (app) => {
    // pick the pair carrying a live offer: ZEC when prepared swaps are armed
    // (env REAL_ZEC=1), Bitcoin otherwise
    const namedPair = await app.findByProperty("objectName", "takerPair");
    const combos = await app.findByProperty("displayText", "Zcash");
    const combo = (namedPair.matches ?? [])[0]
      ?? (combos.matches ?? []).find((m) => String(m.type ?? "").includes("Combo"));
    if (!combo) throw new Error("pair ComboBox not found");
    await evaluateIn(app, combo.id, "currentIndex = 1");
    const namedDirection = await app.findByProperty("objectName", "takerDirection");
    const dirs = await app.findByProperty("displayText", "TakerSellsLez");
    const dirCombo = (namedDirection.matches ?? [])[0]
      ?? (dirs.matches ?? []).find((m) => String(m.type ?? "").includes("Combo"));
    if (dirCombo) await evaluateIn(app, dirCombo.id, "currentIndex = 1");
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
}

process.on("exit", () => {
  try { appProcess.kill("SIGTERM"); } catch {}
  try { rmSync(freshUserDir, { recursive: true, force: true }); } catch {}
});

await run();
