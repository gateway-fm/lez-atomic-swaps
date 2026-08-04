#!/usr/bin/env node

import { readFileSync, statSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const basecampRoot = resolve(repositoryRoot, "apps/basecamp");
const errors = [];

const builderCommit = "92ef691ea72844134f6c68fb447d37f855fc9690";
const builderNarHash = "sha256-jm3NQ0BZ5qnUs/boE1vTil+mGbbp+Wix0ggl1HzR2gw=";

const packages = [
  {
    directory: "maker",
    name: "lez_atomic_swap_maker",
    main: "lez_atomic_swap_maker_plugin",
    icon: "icons/maker-console.svg",
    environment: "LEZ_MAKER_RPC_SOCKET",
    methods: [
      "maker_health",
      "maker_local_route_save_v1",
      "swap_history",
      "maker_actor_monitor_v1",
      "maker_actor_claim_v1",
      "maker_actor_refund_v1",
    ],
    slots: ["health", "saveRoute", "history", "monitor", "claim", "refund"],
    objects: [
      "makerConnection",
      "makerPair",
      "makerDirection",
      "makerForeignUnits",
      "makerLezUnits",
      "makerSave",
      "makerActive",
      "makerHistory",
    ],
  },
  {
    directory: "taker",
    name: "lez_atomic_swap_taker",
    main: "lez_atomic_swap_taker_plugin",
    icon: "icons/taker-route.svg",
    environment: "LEZ_TAKER_RPC_SOCKET",
    methods: [
      "taker_health",
      "taker_offer_list_v1",
      "taker_swap_initiate_v1",
      "taker_swap_list_v1",
      "taker_swap_monitor_v1",
      "taker_swap_claim_v1",
      "taker_swap_refund_v1",
    ],
    slots: ["health", "listOffers", "initiate", "listSwaps", "monitor", "claim", "refund"],
    objects: [
      "takerConnection",
      "takerOffers",
      "takerReview",
      "takerInitiate",
      "takerProgress",
      "takerClaim",
      "takerRefund",
      "takerShielding",
    ],
  },
];

function report(file, message) {
  errors.push(`${relative(repositoryRoot, file)}: ${message}`);
}

function read(file, purpose = "required file") {
  try {
    if (!statSync(file).isFile()) {
      report(file, `${purpose} is not a regular file`);
      return null;
    }
    return readFileSync(file, "utf8");
  } catch (error) {
    report(file, `${purpose} is unavailable (${error.code ?? error.message})`);
    return null;
  }
}

function parseJson(file, purpose) {
  const source = read(file, purpose);
  if (source === null) return null;
  try {
    return JSON.parse(source);
  } catch (error) {
    report(file, `${purpose} is invalid JSON (${error.message})`);
    return null;
  }
}

function requirePattern(file, source, pattern, message) {
  if (source !== null && !pattern.test(source)) report(file, message);
}

function rejectPattern(file, source, pattern, message) {
  if (source !== null && pattern.test(source)) report(file, message);
}

const rootFlake = resolve(basecampRoot, "flake.nix");
const flakeSource = read(rootFlake, "consumer flake");
requirePattern(
  rootFlake,
  flakeSource,
  /logos-module-builder\.url\s*=\s*"github:logos-co\/logos-module-builder\/0\.2\.0"/,
  "must select official module-builder tag 0.2.0",
);
for (const value of ["maker", "taker"]) {
  requirePattern(
    rootFlake,
    flakeSource,
    new RegExp(`${value}Package\\s*=.*mkLogosQmlModule`, "s"),
    `must build the ${value} package through mkLogosQmlModule`,
  );
}
requirePattern(
  rootFlake,
  flakeSource,
  /commonSource/,
  "must inject one shared local-RPC client source into both isolated package builds",
);
rejectPattern(
  rootFlake,
  flakeSource,
  /--override-input|builtins\.getFlake|github:[^"\s]+\/(?:master|main|dev)(?=["\s])/,
  "must not depend on build-time overrides or floating branches",
);

const lockFile = resolve(basecampRoot, "flake.lock");
const lock = parseJson(lockFile, "consumer flake lock");
if (lock !== null) {
  const lockedBuilders = Object.values(lock.nodes ?? {}).filter(
    (node) =>
      node?.locked?.owner === "logos-co" &&
      node?.locked?.repo === "logos-module-builder",
  );
  if (
    lockedBuilders.length !== 1 ||
    lockedBuilders[0].locked.rev !== builderCommit ||
    lockedBuilders[0].locked.narHash !== builderNarHash
  ) {
    report(
      lockFile,
      "must pin exactly one module-builder node to the approved commit and NAR hash",
    );
  }
}

const commonHeader = resolve(basecampRoot, "common/local_json_rpc_client.h");
const commonSourceFile = resolve(basecampRoot, "common/local_json_rpc_client.cpp");
const commonHeaderText = read(commonHeader, "shared local-RPC header");
const commonText = read(commonSourceFile, "shared local-RPC implementation");
for (const [pattern, message] of [
  [/QLocalSocket/, "must use Qt's Unix-domain local socket client"],
  [/lstat\s*\(/, "must inspect the socket without following symlinks"],
  [/geteuid\s*\(/, "must bind socket ownership to the effective user"],
  [/S_ISSOCK/, "must require an actual Unix socket"],
  [/0?600/, "must require owner-only socket mode 0600"],
  [/64\s*\*\s*1024|65536/, "must bound RPC messages to 64 KiB"],
  [/Content-Length/i, "must enforce bounded HTTP Content-Length framing"],
  [/QJsonDocument/, "must parse JSON through Qt's maintained JSON implementation"],
  [/waitForConnected/, "must bound connection readiness"],
]) {
  requirePattern(commonSourceFile, commonText, pattern, message);
}
requirePattern(
  commonHeader,
  commonHeaderText,
  /class\s+LocalJsonRpcClient/,
  "must expose one shared, non-QML local RPC client",
);
for (const [pattern, message] of [
  [/QTcpSocket|QNetworkAccessManager|QWebSocket/, "must not open TCP or web transports"],
  [/QProcess|\bsystem\s*\(|\bpopen\s*\(|\bexec[a-z]*\s*\(/, "must not spawn commands"],
  [/qDebug|std::cerr|fprintf\s*\(\s*stderr/, "must not log request or response payloads"],
]) {
  rejectPattern(commonSourceFile, commonText, pattern, message);
}

for (const pkg of packages) {
  const root = resolve(basecampRoot, pkg.directory);
  const metadataFile = resolve(root, "metadata.json");
  const metadata = parseJson(metadataFile, "package metadata");
  if (metadata !== null) {
    const expected = {
      name: pkg.name,
      version: "0.1.0",
      type: "ui_qml",
      interface: "universal",
      main: pkg.main,
      view: "qml/Main.qml",
      icon: pkg.icon,
    };
    for (const [key, value] of Object.entries(expected)) {
      if (metadata[key] !== value) report(metadataFile, `${key} must equal ${JSON.stringify(value)}`);
    }
    if (!Array.isArray(metadata.dependencies) || metadata.dependencies.length !== 0) {
      report(metadataFile, "must not grant a generic Logos module dependency");
    }
    if (metadata.codegen?.rep !== `src/${pkg.name}.rep`) {
      report(metadataFile, "must select the role-specific typed QtRO interface");
    }
  }

  const cmakeFile = resolve(root, "CMakeLists.txt");
  const cmake = read(cmakeFile, "package CMake definition");
  for (const [pattern, message] of [
    [/logos_module\s*\(/, "must use the official logos_module builder"],
    [/REP_FILE/, "must generate a typed QtRO boundary"],
    [/LocalJsonRpcClient|local_json_rpc_client/, "must compile the shared local-RPC client"],
    [/Qt\$\{QT_VERSION_MAJOR\}::Network|Qt6::Network/, "must explicitly link Qt Network for QLocalSocket"],
  ]) {
    requirePattern(cmakeFile, cmake, pattern, message);
  }

  const repFile = resolve(root, `src/${pkg.name}.rep`);
  const rep = read(repFile, "typed QtRO interface");
  for (const slot of pkg.slots) {
    requirePattern(
      repFile,
      rep,
      new RegExp(`\\bSLOT\\s*\\([^\\n]*\\b${slot}\\s*\\(`),
      `missing typed ${slot} slot`,
    );
  }
  rejectPattern(
    repFile,
    rep,
    /\b(?:call|invoke|request|execute|run)(?:Json|Rpc|Method|Command)?\s*\(\s*QString\s+(?:method|command|path)/i,
    "must not expose a generic method, command, or path",
  );
  rejectPattern(repFile, rep, /(?:private|secret|seed|mnemonic|keyPath|receiptPath)/i, "must not expose private custody");

  const backendHeader = resolve(root, `src/${pkg.name}_backend.h`);
  const backendSource = resolve(root, `src/${pkg.name}_backend.cpp`);
  const header = read(backendHeader, "role backend header");
  const source = read(backendSource, "role backend implementation");
  requirePattern(
    backendHeader,
    header,
    /SimpleSource[\s\S]*LogosUiPluginContext/,
    "must inherit the generated QtRO source and LogosUiPluginContext",
  );
  requirePattern(
    backendSource,
    source,
    new RegExp(`qEnvironmentVariable\\s*\\(\\s*"${pkg.environment}"\\s*\\)`),
    `must read only the fixed ${pkg.environment} endpoint`,
  );
  for (const method of pkg.methods) {
    requirePattern(
      backendSource,
      source,
      new RegExp(`["']${method}["']`),
      `must delegate through fixed RPC method ${method}`,
    );
  }
  for (const [pattern, message] of [
    [/QProcess|\bsystem\s*\(|\bpopen\s*\(|\bexec[a-z]*\s*\(/, "must not spawn commands"],
    [/QTcpSocket|QNetworkAccessManager|QWebSocket/, "must not open non-local transports"],
    [/qDebug|std::cerr|fprintf\s*\(\s*stderr/, "must not log UI or RPC payloads"],
  ]) {
    rejectPattern(backendSource, source, pattern, message);
  }

  const qmlFile = resolve(root, "src/qml/Main.qml");
  const qml = read(qmlFile, "QML view");
  requirePattern(
    qmlFile,
    qml,
    new RegExp(`logos\\.module\\(\\s*"${pkg.name}"\\s*\\)`),
    "must connect to its own typed process-isolated backend",
  );
  requirePattern(qmlFile, qml, /logos\.watch\s*\(/, "must observe typed QtRO slot completion");
  for (const objectName of pkg.objects) {
    requirePattern(
      qmlFile,
      qml,
      new RegExp(`objectName\\s*:\\s*"${objectName}"`),
      `missing actor-test objectName ${objectName}`,
    );
  }
  for (const [pattern, message] of [
    [/XMLHttpRequest|WebSocket|QtWebSockets|QtNetwork/, "must not open transport from QML"],
    [/LocalStorage|Settings\s*\{|Qt\.labs\.settings/, "must not persist authority in QML"],
    [/FileDialog|FolderDialog|QProcess|private[_ -]?key|seed phrase|mnemonic/i, "must not expose custody or arbitrary paths"],
  ]) {
    rejectPattern(qmlFile, qml, pattern, message);
  }

  read(resolve(root, pkg.icon), "local package icon");
  const uiTests = read(resolve(root, "tests/ui-tests.mjs"), "official Qt MCP UI tests");
  requirePattern(
    resolve(root, "tests/ui-tests.mjs"),
    uiTests,
    /test\s*\(/,
    "must ship at least one official-framework UI test",
  );
}

read(resolve(basecampRoot, "README.md"), "Basecamp package build and operator guide");

if (errors.length > 0) {
  process.stderr.write(
    `M6 Basecamp package contract: RED (${errors.length} defect${errors.length === 1 ? "" : "s"})\n`,
  );
  for (const error of errors.sort()) process.stderr.write(`- ${error}\n`);
  process.exitCode = 1;
} else {
  process.stdout.write(
    "M6 Basecamp package contract: GREEN (2 ui_qml packages, 13 typed slots, one owner-local transport)\n",
  );
}
