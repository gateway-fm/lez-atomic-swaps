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
const wantedDirection = reverseDirection ? "taker_sells_lez" : "taker_sells_foreign";

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

// The swap belongs to whichever Maker wallet published the offer the Taker
// took, so select the wallet whose market snapshot carries the Maker's action.
// The action becomes pending only once the runner reaches the Maker's gate,
// so keep cycling through the wallets for as long as a gate may take.
async function selectMakerWalletWithAction(app, walletId) {
  const wallets = ["maker-munich-01", "maker-basel-02"];
  const deadline = Date.now() + 600000;
  while (Date.now() < deadline) {
    for (const [index, expected] of wallets.entries()) {
      await evaluateIn(app, walletId, `currentIndex = ${index}; root.refreshBtcMarket(false)`);
      const settled = Date.now() + 15000;
      while (Date.now() < settled) {
        let envelope = null;
        try { envelope = JSON.parse(await property(app, "makerOutput", "text")); } catch { /* not yet a snapshot */ }
        if (envelope?.ok === true && envelope.result?.selected_wallet_id === expected) {
          if ((envelope.result.swaps ?? []).some((swap) => swap.action_role === "maker")) return;
          break;
        }
        await new Promise((resolve) => setTimeout(resolve, 500));
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 2000));
  }
  throw new Error("no Maker wallet holds a pending action");
}

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

async function setText(app, objectName, value, property = "text") {
  const found = await app.findByProperty("objectName", objectName);
  if (found.error || found.matches?.length !== 1) {
    throw new Error(`expected exactly one ${objectName}, got ${JSON.stringify(found)}`);
  }
  await evaluateIn(app, found.matches[0].id, `${property} = ${JSON.stringify(String(value))}`);
}

// The Maker's terms for this run, typed into the offer form: 1,000 LEZ
// units for 0.01 BTC, whole offer only, for one hour.
const makerTerms = [
  ["makerSellAmount", reverseDirection ? "0.01" : "1000", "amount"],
  ["makerReceiveAmount", reverseDirection ? "1000" : "0.01", "amount"],
  ["makerMinimumSats", ""],
  ["makerOfferTtl", "3600"],
];

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

// A market refresh from the desk's own root (the "Refresh market" action),
// so a button the Node offers later (a refund past the Maker's cutoff)
// appears without a click. Not the silent variant: only a full refresh
// rewrites the output text these checks read.
async function refreshMarket(app, walletObjectName) {
  const wallet = await app.findByProperty("objectName", walletObjectName);
  if (wallet.matches?.length === 1) await evaluateIn(app, wallet.matches[0].id, "root.refreshBtcMarket(false)");
}

// The swaps the desk renders, narrowed to the one the caller named (the Node
// names each swap in its take reply, the desk carries it as `ui_swap_id`).
function renderedSwaps(envelope, wanted) {
  return (envelope.result?.swaps ?? []).filter((swap) =>
    !wanted || swap.ui_swap_id === wanted || swap.swap_id === wanted);
}

const waitTimeoutMs = Number(process.env.INTERACTIVE_TIMEOUT_MS || 1800000);

async function triggerVisibleAction(app, objectName, expectedText, outputName, workingStates, wanted, settleMs) {
  let target = null;
  await app.waitFor(async () => {
    await refreshMarket(app, `${role}BtcWallet`);
    await new Promise((resolve) => setTimeout(resolve, 1200));
    const found = await app.findByProperty("objectName", objectName);
    for (const match of found.matches ?? []) {
      const response = await app.getProperties(match.id);
      const values = Object.fromEntries((response.properties ?? []).map((entry) => [entry.name, entry.value]));
      if (values.visible !== true || values.enabled !== true || values.text !== expectedText) continue;
      // Other swaps may show the same button; only the named swap's row counts.
      if (wanted) {
        const row = await evaluateIn(app, match.id, "String(modelData.ui_swap_id)");
        if (row.ok !== true || row.result !== wanted) continue;
      }
      target = match.id;
      return;
    }
    throw new Error(`${expectedText} is not ready${wanted ? ` on ${wanted.slice(0, 12)}` : ""}`);
  }, { timeout: waitTimeoutMs, interval: 5000, description: `${expectedText} readiness` });
  await evaluateIn(app, target, "clicked()");
  await app.waitFor(async () => {
    await refreshMarket(app, `${role}BtcWallet`);
    await new Promise((resolve) => setTimeout(resolve, 1200));
    const envelope = JSON.parse(await property(app, outputName, "text"));
    if (envelope.ok === false) throw new Error(`${expectedText} refused: ${JSON.stringify(envelope.error ?? envelope)}`);
    if (!renderedSwaps(envelope, wanted).some((swap) => workingStates.includes(swap.state))) {
      throw new Error(`${expectedText} has not entered ${workingStates.join("|")}`);
    }
  }, { timeout: settleMs, interval: 5000, description: `${expectedText} submission` });
}

// Watching one swap reach a desk state; the Node acts, the desk shows it.
async function waitDeskState(app, outputName, wanted, states, label) {
  await app.waitFor(async () => {
    await refreshMarket(app, `${role}BtcWallet`);
    await new Promise((resolve) => setTimeout(resolve, 1500));
    const envelope = JSON.parse(await property(app, outputName, "text"));
    if (envelope.ok !== true || !renderedSwaps(envelope, wanted).some((swap) => states.includes(swap.state))) {
      throw new Error(`no ${role} swap ${wanted ? wanted.slice(0, 12) + " " : ""}has reached ${states.join("|")}`);
    }
  }, { timeout: waitTimeoutMs, interval: 15000, description: label });
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
  for (const [field, value, property] of makerTerms) await setText(app, field, value, property);
  await new Promise((r) => setTimeout(r, 300));
  const rate = await property(app, "makerRate", "text");
  if (rate !== "1 BTC = 100,000 LEZ") throw new Error(`offer form quotes ${rate} for the typed terms`);
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
    // Signal-invoke by objectName: text-targeted clicks do not reliably reach
    // controls under the offscreen platform once the inventory has rendered.
    const check = await app.findByProperty("objectName", "makerHealth");
    if (check.error || check.matches?.length !== 1) throw new Error("Check Node button is unavailable");
    await evaluateIn(app, check.matches[0].id, "clicked()");
    await app.waitFor(async () => app.expectTexts(["Node ready"]), {
      timeout: 15000, interval: 300, description: "Maker health status",
    });
    console.log("  health: Node ready");
  });

  test("maker: Node-indexed BTC offer inventory", async (app) => {
    if (reverseDirection) {
      const sellLeg = await app.findByProperty("objectName", "makerSellAmount");
      if (sellLeg.error || sellLeg.matches?.length !== 1) {
        throw new Error("Maker offer form was not found");
      }
      await evaluateIn(app, sellLeg.matches[0].id, 'root.sellSide = "btc"');
    }
    const wallet = await app.findByProperty("objectName", "makerBtcWallet");
    if (wallet.matches?.length !== 1) {
      throw new Error("Maker wallet selector was not found");
    }
    await evaluateIn(app, wallet.matches[0].id, "currentIndex = 0");
    let munich = unwrap(await outputAfterClick(
      app, "Refresh market", "makerOutput",
      (envelope) => envelope.ok === true
        && envelope.result?.selected_wallet_id === "maker-munich-01", true,
    ), "Munich inventory");
    // Offers of the other direction may be open too; only this run's count.
    const pendingHere = (inventory) => (inventory ?? [])
      .filter((offer) => offer.state === "pending" && offer.direction === wantedDirection).length;
    let pending = pendingHere(munich.inventory);
    // The Node publishes one offer per click; two open offers prove the
    // inventory is indexed to this Node's identity and survives a refresh.
    while (pending < 2) {
      const target = pending + 1;
      munich = unwrap(await publishOfferOnce(
        app,
        (envelope) => envelope.ok === true && pendingHere(envelope.result?.inventory) >= target,
      ), "Munich offers");
      pending = pendingHere(munich.inventory);
    }
    if (munich.selected_wallet_id !== "maker-munich-01" || munich.runner_ready !== true
        || Number(munich.summary?.pending_offers ?? 0) < 2) {
      throw new Error(`Node-indexed offer totals are wrong: ${JSON.stringify(munich).slice(0, 500)}`);
    }
    console.log(`  inventory: Munich Vault 01 (Maker Node) open offers=${pending} · market=${munich.summary.pending_offers}`);
  });

  // The Maker Node's supervisor funds LEZ and claims Bitcoin itself; the desk
  // step is to watch the swap reach that state.
  // `INTERACTIVE_ACTION=wait INTERACTIVE_STATE=<desk state>` names the state
  // to wait for; the older action names map to the state each ends in.
  const makerWaits = reverseDirection
    ? { lock_btc: ["awaiting_taker_claim", "Bitcoin locked"], claim_lez: ["completed", "LEZ claimed"] }
    : { fund_lez: ["awaiting_taker_claim", "LEZ escrow funded"], claim_btc: ["completed", "Bitcoin claimed"] };
  if (process.env.INTERACTIVE_ACTION === "wait" && process.env.INTERACTIVE_STATE) {
    makerWaits.wait = [process.env.INTERACTIVE_STATE, process.env.INTERACTIVE_STATE];
  }
  if (Object.hasOwn(makerWaits, process.env.INTERACTIVE_ACTION)) {
    const action = process.env.INTERACTIVE_ACTION;
    const [state, label] = makerWaits[action];
    test(`maker: ${action} ${state} reached by the Node`, async (app) => {
      // Earlier swaps may already sit in the target state: when the caller
      // names the swap this run created, only that swap counts.
      const wanted = process.env.INTERACTIVE_SWAP_ID || "";
      await waitDeskState(app, "makerOutput", wanted, state.split("|"), `${label} by the Maker Node`);
      console.log(`  Node-owned swap: Maker reached ${state} (${label})`);
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
  });

  test("taker: wallet-indexed BTC order book is ready", async (app) => {
    await app.expectTexts(["ACCOUNT", "My orders", "Available orders", "Zurich Wallet 01 · Taker Node"]);
    // The order book arrives with the first market snapshot after the view
    // opens; wait for the rendered rows instead of racing that request.
    await app.waitFor(async () => app.expectTexts(["0.01000000 BTC", "1,000 LEZ"]), {
      timeout: 15000, interval: 500, description: "first market snapshot rendered",
    });
    await app.click("Refresh market");
    await app.waitFor(async () => app.expectTexts(["Munich Vault 01"]), {
      timeout: 15000, interval: 500, description: "Maker Node order book",
    });
    console.log("  order book: the Maker Node's offers are visible to the Taker Node's identity");
  });

  test("taker: real Node health", async (app) => {
    // Signal-invoke by objectName: text-targeted clicks do not reliably reach
    // controls under the offscreen platform once the order book has grown.
    const check = await app.findByProperty("objectName", "takerHealth");
    if (check.error || check.matches?.length !== 1) throw new Error("Check Node button is unavailable");
    await evaluateIn(app, check.matches[0].id, "clicked()");
    await app.waitFor(async () => app.expectTexts(["Node ready"]), {
      timeout: 15000, interval: 300, description: "Taker health status",
    });
    console.log("  health: Node ready");
  });

  if (process.env.PREPARE_INTERACTIVE_BTC === "1") {
    test("taker: taking one offer prepares the real Taker BTC action", async (app) => {
      // Rows of both directions may be open; take one of this run's, read
      // from the row's own model behind each Take button.
      const buttons = await app.findByProperty("objectName", "takerTakeOffer");
      let target = null;
      const seen = [];
      for (const match of buttons.matches ?? []) {
        const direction = await evaluateIn(app, match.id, "String(modelData.direction)");
        seen.push(direction.result);
        if (direction.ok === true && direction.result === wantedDirection) { target = match.id; break; }
      }
      if (target === null) {
        throw new Error(`no takeable ${wantedDirection} order-book row (rows: ${seen.join(", ") || "none"})`);
      }
      await evaluateIn(app, target, "clicked()");
      const firstAction = reverseDirection ? "lock_lez" : "lock_btc";
      // Older swaps may already show the same lock button: only the swap this
      // take created counts, and the Node names it in its reply. A rejected
      // take fails at once instead of waiting out the deadline.
      const deadline = Date.now() + 600000;
      let taken;
      for (;;) {
        let envelope = null;
        try { envelope = JSON.parse(await property(app, "takerOutput", "text")); } catch { /* not yet a reply */ }
        if (envelope?.ok === false) {
          throw new Error(`take rejected by the Taker Node: ${JSON.stringify(envelope.error ?? envelope)}`);
        }
        const swapId = envelope?.result?.taken?.swap?.swap_id;
        if (swapId) {
          const row = (envelope.result.swaps ?? []).find((swap) => swap.ui_swap_id === swapId);
          if (row?.action_required === firstAction) { taken = swapId; break; }
          if (Date.now() > deadline) {
            throw new Error(`swap ${swapId.slice(0, 12)} is ${row?.state ?? "missing"}, not ready to ${firstAction}`);
          }
        } else if (Date.now() > deadline) {
          throw new Error("the Taker Node did not answer the take in time");
        }
        await new Promise((resolve) => setTimeout(resolve, 2000));
      }
      console.log(`  Node-owned swap ${taken.slice(0, 12)}: offer taken · Taker lock action ready`);
    });
  }

  // A refund is admitted at once and driven by the Node until the chain
  // clocks allow it, so its row settles into refunding/refunded much later.
  const takerActions = reverseDirection
    ? { lock_lez: ["Lock 1,000 LEZ", ["locking_lez"], 45000],
        claim_btc: ["Claim 0.01000000 BTC", ["claiming_btc"], 45000],
        refund_lez: ["Refund 1,000 LEZ", ["refunding", "refunded"], waitTimeoutMs] }
    : { lock_btc: ["Lock 0.01000000 BTC", ["locking_btc"], 45000],
        claim_lez: ["Claim 1,000 LEZ", ["claiming_lez"], 45000],
        refund_btc: ["Refund 0.01000000 BTC", ["refunding", "refunded"], waitTimeoutMs] };
  if (Object.hasOwn(takerActions, process.env.INTERACTIVE_ACTION)) {
    const action = process.env.INTERACTIVE_ACTION;
    const [label, working, settleMs] = takerActions[action];
    test(`taker: perform ${action}`, async (app) => {
      await triggerVisibleAction(app, "takerSwapAction", label, "takerOutput", working,
                                 process.env.INTERACTIVE_SWAP_ID || "", settleMs);
      console.log(`  Node-owned swap: Taker ${action} submitted`);
    });
  }
  if (process.env.INTERACTIVE_ACTION === "wait" && process.env.INTERACTIVE_STATE) {
    const states = process.env.INTERACTIVE_STATE.split("|");
    test(`taker: swap reaches ${states.join("|")}`, async (app) => {
      await waitDeskState(app, "takerOutput", process.env.INTERACTIVE_SWAP_ID || "", states, `${states.join("|")} on the Taker desk`);
      console.log(`  Node-owned swap: Taker desk shows ${states.join("|")}`);
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
