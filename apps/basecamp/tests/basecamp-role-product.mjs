import { resolve } from "node:path";

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

const { test, run } = await import(resolve(framework, "test-framework/framework.mjs"));

async function property(app, objectName, propertyName) {
  const found = await app.findByProperty("objectName", objectName);
  if (found.error || !found.matches || found.matches.length !== 1) {
    throw new Error(`expected exactly one ${objectName}, got ${JSON.stringify(found)}`);
  }
  const response = await app.getProperties(found.matches[0].id);
  if (response.error) throw new Error(response.error);
  const value = response.properties.find((entry) => entry.name === propertyName);
  if (!value) throw new Error(`${objectName}.${propertyName} is unavailable`);
  return value.value;
}

async function expectSuccessfulOutput(app, action) {
  await app.waitFor(async () => {
    const raw = await property(app, expected.output, "text");
    let envelope;
    try {
      envelope = JSON.parse(raw);
    } catch {
      throw new Error(`${action} did not return JSON: ${raw}`);
    }
    if (envelope.ok !== true) throw new Error(`${action} failed: ${raw}`);
  }, { timeout: 15000, interval: 300, description: `${role} ${action} success` });
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
    await app.click(expected.health);
    await expectSuccessfulOutput(app, "health");
    if (role === "maker") {
      await app.click("Save route atomically");
      await expectSuccessfulOutput(app, "atomic route save");
      await app.click("Refresh swap history");
      await expectSuccessfulOutput(app, "history");
    } else {
      await app.click("Browse authenticated offers");
      await expectSuccessfulOutput(app, "offer browsing");
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
