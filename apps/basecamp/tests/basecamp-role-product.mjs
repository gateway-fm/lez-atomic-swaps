import { resolve } from "node:path";
import { readFileSync } from "node:fs";

const framework = process.env.LOGOS_QT_MCP;
if (!framework) throw new Error("LOGOS_QT_MCP must select the pinned official test framework");

const roles = {
  maker: {
    launcher: "LEZ Atomic Swap Maker",
    heading: "LEZ Atomic Swap — Maker Console",
    health: "Check service",
    output: "makerOutput",
  },
  taker: {
    launcher: "LEZ Atomic Swap Taker",
    heading: "LEZ Atomic Swap — Taker Route",
    health: "Service health",
    output: "takerOutput",
  },
};

const role = process.env.M6_BASECAMP_ROLE;
const expected = roles[role];
if (!expected) throw new Error("M6_BASECAMP_ROLE must be maker or taker");
const expectService = process.env.M6_BASECAMP_EXPECT_SERVICE === "1";
const takerFixture = role === "taker" && process.env.M6_TAKER_FIXTURE_JSON
  ? JSON.parse(readFileSync(process.env.M6_TAKER_FIXTURE_JSON, "utf8"))
  : null;

const { test, run } = await import(resolve(framework, "test-framework/framework.mjs"));

async function property(app, objectName, propertyName) {
  const found = await app.findByProperty("objectName", objectName);
  if (found.error || !found.matches || found.matches.length !== 1) {
    const named = await app.findByProperty("objectName");
    const available = (named.matches || [])
      .map((entry) => entry.value)
      .filter((value) => typeof value === "string" && value.length > 0);
    throw new Error(
      `expected exactly one ${objectName}, got ${JSON.stringify(found)}; `
      + `available object names: ${JSON.stringify(available)}`,
    );
  }
  const response = await app.getProperties(found.matches[0].id);
  if (response.error) throw new Error(response.error);
  const value = response.properties.find((entry) => entry.name === propertyName);
  if (!value) throw new Error(`${objectName}.${propertyName} is unavailable`);
  return value.value;
}

async function invokeSuccessfully(app, button, action, predicate = () => true) {
  const before = await property(app, expected.output, "text");
  await app.click(button);
  let envelope;
  await app.waitFor(async () => {
    const raw = await property(app, expected.output, "text");
    if (raw === before || raw === "Waiting for owner-local service...") {
      throw new Error(`${action} has not completed`);
    }
    envelope = JSON.parse(raw);
    if (envelope.ok !== true || !predicate(envelope.result)) {
      throw new Error(`${action} returned an unexpected result: ${raw}`);
    }
  }, { timeout: 15000, interval: 300, description: `${role} ${action} completion` });
  return envelope;
}

async function evaluateIn(app, objectName, expression) {
  const found = await app.findByProperty("objectName", objectName);
  if (found.error || !found.matches || found.matches.length !== 1) {
    throw new Error(`expected exactly one ${objectName}, got ${JSON.stringify(found)}`);
  }
  const response = await app.inspector.send("evaluate", {
    objectId: found.matches[0].id,
    expression,
  });
  if (response.error) throw new Error(`evaluate in ${objectName}: ${response.error}`);
}

test(`${role}: pinned Basecamp discovers and loads the role package`, async (app) => {
  await app.waitFor(async () => app.expectTexts([expected.launcher]), {
    timeout: 20000,
    interval: 500,
    description: `${role} package discovery`,
  });
  await app.click(expected.launcher);
  await app.waitFor(async () => app.expectTexts([expected.heading, "Backend connected"]), {
    timeout: 20000,
    interval: 500,
    description: `${role} package and process backend load`,
  });
});

if (expectService) {
  test(`${role}: Basecamp calls the real owner-local role service`, async (app) => {
    await invokeSuccessfully(app, expected.health, "health");
    if (role === "maker") {
      await invokeSuccessfully(app, "Save route atomically", "atomic route save");
      await invokeSuccessfully(app, "Refresh swap history", "history");
    } else {
      await invokeSuccessfully(app, "Browse authenticated offers", "offer browsing");
      if (takerFixture) {
        await evaluateIn(app, "takerReview", [
          `offerId.text = ${JSON.stringify(String(takerFixture.offer_id))}`,
          `makerIdentity.text = ${JSON.stringify(String(takerFixture.maker_identity))}`,
          `envelopeDigest.text = ${JSON.stringify(String(takerFixture.signed_envelope_sha256))}`,
          `foreignUnits.text = ${JSON.stringify(String(takerFixture.foreign_units))}`,
          `lezUnits.text = ${JSON.stringify(String(takerFixture.expected_lez_units))}`,
          "true",
        ].join("; "));
        await invokeSuccessfully(app, "Confirm and initiate", "prepared initiation",
          (result) => result.was_replay === false);
        await invokeSuccessfully(app, "Confirm and initiate", "exact initiation replay",
          (result) => result.was_replay === true);
        await invokeSuccessfully(app, "List my swaps", "swap list");
        await evaluateIn(app, "takerProgress",
          `swapId.text = ${JSON.stringify(String(takerFixture.swap_id))}; true`);
        await invokeSuccessfully(app, "Monitor", "swap monitor",
          (result) => result.swap_id === takerFixture.swap_id);
      }
    }
  });
} else {
  test(`${role}: missing owner-local service fails closed without crashing the UI`, async (app) => {
    await app.click(expected.health);
    await app.waitFor(async () => app.expectTexts([
      '{"code":"endpoint_unavailable","message":"Owner-local service endpoint is unavailable","ok":false}',
    ]), {
      timeout: 10000,
      interval: 300,
      description: `${role} fail-closed local endpoint result`,
    });
  });
}

run();
