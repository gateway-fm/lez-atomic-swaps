import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { extname, join } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import test, { after, before } from "node:test";

import puppeteer from "puppeteer";

const prototypesRoot = fileURLToPath(
  new URL("../../apps/m6-prototypes/", import.meta.url),
);
const allowedFiles = new Set([
  "/index.html",
  "/maker.html",
  "/taker.html",
  "/styles.css",
  "/prototype.js",
  "/assets/lez-orbit.svg",
  "/assets/maker-console.svg",
  "/assets/taker-route.svg",
]);
const contentTypes = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".svg", "image/svg+xml; charset=utf-8"],
]);
const csp = [
  "default-src 'none'",
  "script-src 'self'",
  "style-src 'self'",
  "img-src 'self'",
  "connect-src 'none'",
  "font-src 'none'",
  "media-src 'none'",
  "object-src 'none'",
  "frame-src 'none'",
  "worker-src 'none'",
  "base-uri 'none'",
  "form-action 'none'",
  "frame-ancestors 'none'",
].join("; ");

let browser;
let origin;
let server;
let userDataDir;
let ownedTestRoot;

before(
  async () => {
    server = createServer(async (request, response) => {
      const method = request.method ?? "";
      if (method !== "GET" && method !== "HEAD") {
        response.writeHead(405, {
          Allow: "GET, HEAD",
          "Content-Type": "text/plain; charset=utf-8",
        });
        response.end("Method not allowed\n");
        return;
      }

      const requestUrl = new URL(request.url ?? "/", origin ?? "http://127.0.0.1");
      const pathname = requestUrl.pathname === "/" ? "/index.html" : requestUrl.pathname;
      if (!allowedFiles.has(pathname)) {
        response.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
        response.end("Prototype file not found\n");
        return;
      }

      try {
        const body = await readFile(join(prototypesRoot, pathname.slice(1)));
        response.writeHead(200, {
          "Cache-Control": "no-store",
          "Content-Security-Policy": csp,
          "Content-Type":
            contentTypes.get(extname(pathname)) ?? "application/octet-stream",
          "Cross-Origin-Opener-Policy": "same-origin",
          "Cross-Origin-Resource-Policy": "same-origin",
          "Permissions-Policy": "camera=(), geolocation=(), microphone=()",
          "Referrer-Policy": "no-referrer",
          "X-Content-Type-Options": "nosniff",
        });
        response.end(method === "HEAD" ? undefined : body);
      } catch {
        response.writeHead(500, { "Content-Type": "text/plain; charset=utf-8" });
        response.end("Prototype file unavailable\n");
      }
    });

    await new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(0, "127.0.0.1", resolve);
    });
    const address = server.address();
    assert(address && typeof address === "object", "HTTP server must expose its port");
    origin = `http://127.0.0.1:${address.port}`;

    const requestedTestRoot = process.env.M6_UI_TEST_ROOT;
    const testRoot = requestedTestRoot ?? tmpdir();
    if (requestedTestRoot) {
      await mkdir(testRoot, { mode: 0o700 });
      ownedTestRoot = testRoot;
    }
    userDataDir = await mkdtemp(join(testRoot, "lez-m6-puppeteer-"));
    browser = await puppeteer.launch({
      headless: true,
      userDataDir,
    });

    const browserArguments = browser.process()?.spawnargs ?? [];
    assert.equal(
      browserArguments.some(
        (argument) => argument === "--no-sandbox" || argument === "--disable-setuid-sandbox",
      ),
      false,
      "the Chromium sandbox must remain enabled",
    );
  },
  { timeout: 30_000 },
);

after(async () => {
  await browser?.close();
  await new Promise((resolve, reject) => {
    if (!server?.listening) {
      resolve();
      return;
    }
    server.close((error) => (error ? reject(error) : resolve()));
  });
  if (userDataDir) await rm(userDataDir, { force: true, recursive: true });
  if (ownedTestRoot) await rm(ownedTestRoot, { force: true, recursive: true });
});

async function withAuditedPage(pathname, callback, viewport) {
  const page = await browser.newPage();
  const consoleErrors = [];
  const failedResponses = [];
  const pageErrors = [];
  const rejectedRequests = [];

  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  page.on("pageerror", (error) => pageErrors.push(error.stack ?? error.message));
  page.on("response", (response) => {
    if (response.status() >= 400) {
      failedResponses.push(`${response.status()} ${response.url()}`);
    }
  });

  await page.setRequestInterception(true);
  page.on("request", (request) => {
    if (request.isInterceptResolutionHandled()) return;
    let requestOrigin;
    try {
      requestOrigin = new URL(request.url()).origin;
    } catch {
      rejectedRequests.push(request.url());
      void request.abort("blockedbyclient");
      return;
    }
    if (requestOrigin !== origin) {
      rejectedRequests.push(request.url());
      void request.abort("blockedbyclient");
      return;
    }
    void request.continue();
  });

  if (viewport) await page.setViewport(viewport);

  const failures = [];
  try {
    const response = await page.goto(`${origin}${pathname}`, {
      waitUntil: "networkidle0",
    });
    assert(response, "navigation must return a response");
    assert.equal(response.status(), 200);
    assert.equal(response.headers()["content-security-policy"], csp);
    await callback(page);
  } catch (error) {
    failures.push(error);
  }

  try {
    assert.deepEqual(rejectedRequests, [], "the page attempted a non-local request");
    assert.deepEqual(failedResponses, [], "the page loaded a failing response");
    assert.deepEqual(consoleErrors, [], "the page emitted console errors");
    assert.deepEqual(pageErrors, [], "the page emitted uncaught errors");
  } catch (error) {
    failures.push(error);
  }

  await page.close();
  if (failures.length === 1) throw failures[0];
  if (failures.length > 1) throw new AggregateError(failures, "page flow and audit failed");
}

async function text(page, selector) {
  await page.waitForSelector(selector);
  return page.$eval(selector, (element) => element.textContent?.trim() ?? "");
}

async function replaceInput(page, selector, value) {
  await page.$eval(
    selector,
    (input, nextValue) => {
      const setter = Object.getOwnPropertyDescriptor(
        HTMLInputElement.prototype,
        "value",
      )?.set;
      setter?.call(input, nextValue);
      input.dispatchEvent(new Event("input", { bubbles: true }));
    },
    value,
  );
}

async function visible(page, selector) {
  return page.$eval(selector, (element) => {
    const style = getComputedStyle(element);
    const bounds = element.getBoundingClientRect();
    return (
      !element.hidden &&
      style.display !== "none" &&
      style.visibility !== "hidden" &&
      bounds.width > 0 &&
      bounds.height > 0
    );
  });
}

async function openBtcProgress(page) {
  await page.waitForSelector('[data-offer="offer-btc-3bd1"]');
  await page.click('[data-offer="offer-btc-3bd1"]');
  assert.match(await text(page, "#selected-offer-detail"), /LEZ \/ BTC/);
  await page.click("#terms-confirm");
  assert.equal(await page.$eval("#initiate-swap", (button) => button.disabled), false);
  await page.click("#initiate-swap");
  assert.equal(await page.$eval("#initiate-dialog", (dialog) => dialog.open), true);
  assert.match(await text(page, "#initiate-summary"), /7,700 LEZ → 0\.005 BTC/);
  await page.click("#confirm-initiate");
  await page.waitForSelector("#taker-progress.active");
  assert.match(await text(page, "#progress-heading"), /BTC-3BD1/);
}

async function advanceToTerminalAction(page) {
  assert.equal(await visible(page, "#terminal-actions"), false);
  await page.click("#advance-progress");
  await page.click("#advance-progress");
  assert.equal(await visible(page, "#terminal-actions"), true);
  assert.equal(await text(page, "#aside-confirmations"), "2 / 2 sample");
}

test("role surfaces remain isolated", async () => {
  await withAuditedPage("/", async (page) => {
    assert.equal(await page.$$eval(".role-card", (cards) => cards.length), 2);
    assert.deepEqual(
      await page.$$eval(".role-card", (cards) => cards.map((card) => card.getAttribute("href"))),
      ["maker.html", "taker.html"],
    );

    await page.click(".maker-card");
    await page.waitForSelector('body[data-prototype="maker"]');
    assert.match(await text(page, ".role-chip"), /Maker operator/);
    assert.equal(await page.$$("#taker-browse").then((nodes) => nodes.length), 0);
    assert.equal(await page.$$('[data-offer]').then((nodes) => nodes.length), 0);

    await page.click('a[href="taker.html"]');
    await page.waitForSelector('body[data-prototype="taker"]');
    assert.match(await text(page, ".role-chip"), /Taker user/);
    assert.equal(await page.$$("#maker-config-form").then((nodes) => nodes.length), 0);
    assert.equal(await page.$$('[data-maker-action]').then((nodes) => nodes.length), 0);
  });
});

test("Maker configures, monitors, filters history, and records manual intent", async () => {
  await withAuditedPage("/maker.html", async (page) => {
    await page.click('[data-view="maker-config"]');
    await page.select("#maker-pair", "bitcoin");
    await replaceInput(page, "#foreign-units", "2");
    await replaceInput(page, "#lez-units", "1820");
    await page.waitForFunction(
      () => document.querySelector("#maker-price-preview")?.textContent === "2 BTC = 1,820 LEZ",
    );
    await page.click('#maker-config-form button[type="submit"]');
    assert.equal(await page.$eval("#maker-review-dialog", (dialog) => dialog.open), true);
    assert.match(await text(page, "#maker-review-content"), /LEZ \/ Bitcoin/);
    assert.match(await text(page, "#maker-review-content"), /2 BTC = 1,820 LEZ/);
    await page.click("#confirm-maker-config");
    await page.waitForFunction(
      () => document.querySelector("#toast")?.textContent?.includes("revision 9 confirmed"),
    );

    await page.click('[data-view="maker-active"]');
    assert.match(await text(page, "#maker-swap-detail"), /1 \/ 2 confirmations/);
    await page.click("#maker-refresh");
    await page.waitForFunction(
      () => document.querySelector("#maker-swap-detail")?.textContent?.includes("Claim available"),
    );
    assert.match(await text(page, "#maker-swap-detail"), /2 \/ 2 confirmations/);
    await page.click('[data-maker-action="claim"]');
    await page.waitForFunction(
      () => document.querySelector("#toast")?.textContent?.includes("claim intent recorded"),
    );
    await page.click('[data-maker-action="refund"]');
    await page.waitForFunction(
      () => document.querySelector("#toast")?.textContent?.includes("refund intent recorded"),
    );

    await page.click('[data-view="maker-history"]');
    await page.select("#history-filter", "refunded");
    assert.match(await text(page, "#history-body"), /BTC-19C4/);
    assert.doesNotMatch(await text(page, "#history-body"), /BTC-DA82/);
    await page.select("#history-filter", "completed");
    assert.match(await text(page, "#history-body"), /BTC-DA82/);
    assert.doesNotMatch(await text(page, "#history-body"), /BTC-19C4/);
  });
});

test("Taker completes the BTC claim journey", async () => {
  await withAuditedPage("/taker.html", async (page) => {
    await openBtcProgress(page);
    await advanceToTerminalAction(page);

    await page.click("#claim-action");
    assert.equal(await page.$eval("#terminal-dialog", (dialog) => dialog.open), true);
    assert.match(await text(page, "#terminal-dialog-copy"), /generation-fenced claim intent/);
    await page.click("#confirm-terminal");
    await page.waitForSelector("#taker-terminal.active");
    assert.equal(await text(page, "#terminal-heading"), "Swap completed");
    assert.match(await text(page, "#terminal-summary"), /sample claim path/);
  });
});

test("Taker can choose the mutually exclusive refund branch", async () => {
  await withAuditedPage("/taker.html", async (page) => {
    await openBtcProgress(page);
    await advanceToTerminalAction(page);

    await page.click("#refund-action");
    assert.match(await text(page, "#terminal-dialog-title"), /Refund sample funds/);
    assert.match(await text(page, "#terminal-dialog-copy"), /generation-fenced refund intent/);
    await page.click("#confirm-terminal");
    await page.waitForSelector("#taker-terminal.active");
    assert.equal(await text(page, "#terminal-heading"), "Swap refunded");
    assert.match(await text(page, "#terminal-summary"), /sample refund path/);
  });
});

test("Taker can select both BTC directions without offer substitution", async () => {
  await withAuditedPage("/taker.html", async (page) => {
    await page.waitForSelector('[data-offer="offer-btc-3bd1"]');
    await page.click('[data-offer="offer-btc-3bd1"]');
    assert.match(await text(page, "#selected-offer-detail"), /LEZ \/ BTC/);
    assert.match(await text(page, "#selected-offer-detail"), /offer-btc-3bd1/);
    assert.doesNotMatch(await text(page, "#selected-offer-detail"), /offer-lez/);

    await page.click('[data-view-jump="taker-browse"]');
    await page.click('[data-direction="receive-lez"]');
    await page.click('[data-offer="offer-lez-51af"]');
    assert.match(await text(page, "#selected-offer-detail"), /BTC \/ LEZ/);
    assert.match(await text(page, "#selected-offer-detail"), /offer-lez-51af/);
    assert.doesNotMatch(await text(page, "#selected-offer-detail"), /offer-btc/);
  });
});

test("mobile role journeys do not overflow horizontally", async () => {
  await withAuditedPage(
    "/maker.html",
    async (page) => {
      for (const view of ["maker-config", "maker-active", "maker-history"]) {
        await page.click(`[data-view="${view}"]`);
        const dimensions = await page.evaluate(() => ({
          body: document.body.scrollWidth,
          viewport: document.documentElement.clientWidth,
          root: document.documentElement.scrollWidth,
        }));
        assert.ok(
          Math.max(dimensions.body, dimensions.root) <= dimensions.viewport + 1,
          `${view} overflows: ${JSON.stringify(dimensions)}`,
        );
      }
    },
    { height: 812, isMobile: true, width: 375 },
  );

  await withAuditedPage(
    "/taker.html",
    async (page) => {
      const assertNoOverflow = async (view) => {
        const dimensions = await page.evaluate(() => ({
          body: document.body.scrollWidth,
          viewport: document.documentElement.clientWidth,
          root: document.documentElement.scrollWidth,
        }));
        assert.ok(
          Math.max(dimensions.body, dimensions.root) <= dimensions.viewport + 1,
          `${view} overflows: ${JSON.stringify(dimensions)}`,
        );
      };

      await assertNoOverflow("taker-browse");
      await page.click('[data-offer="offer-btc-3bd1"]');
      await assertNoOverflow("taker-review");
      await page.click("#terms-confirm");
      await page.click("#initiate-swap");
      await page.click("#confirm-initiate");
      await page.waitForSelector("#taker-progress.active");
      await assertNoOverflow("taker-progress");
    },
    { height: 812, isMobile: true, width: 375 },
  );
});
