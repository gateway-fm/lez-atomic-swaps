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
cpSync(`/var/lez-assets/${role}-user`, freshUserDir, { recursive: true });
process.env.BASECAMP_USER_DIR = freshUserDir;

// spawn the app ourselves (the framework's --ci mode waits only 15s; cold
// module loading needs longer), then attach in normal mode
const appProcess = spawn(`/usr/local/bin/basecamp-${role}`, ["-platform", "offscreen"], {
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
    console.log(`  route save ok (revisions: ${JSON.stringify(saved.pair_revision ?? saved)}) swaps=${(history.swaps ?? []).length}`);
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
    console.log(`  health: ready=true`);
  });
}

process.on("exit", () => {
  try { appProcess.kill("SIGTERM"); } catch {}
  try { rmSync(freshUserDir, { recursive: true, force: true }); } catch {}
});

await run();
