#!/usr/bin/env node

import { readFileSync, statSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const basecampRoot = resolve(repositoryRoot, "apps/basecamp");
const errors = [];

const chatCommit = "dfe8ccf3eff3e95da0ba54043577270474a216ae";
const chatNarHash = "sha256-fztN9UpWmNe1SVe4/173vrKvzh4vyHIRxEg/OoUa6Mg=";
const deliveryCommit = "3258cdb0132e37228aa2519e0c01c0e7429a20dd";
const deliveryNarHash = "sha256-6shBduxH/12ph+Y2R1Kwq65rjK6QfZCwp5vBU5h0i5Y=";

const packages = [
  {
    directory: "maker",
    name: "lez_atomic_swap_maker",
    displayName: "LEZ Atomic Swap Maker",
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
    slots: ["health", "chatStatus", "resetChat", "saveRoute", "history", "monitor", "claim", "refund"],
    objects: [
      "makerConnection",
      "makerChat",
      "makerChatStatus",
      "makerChatReset",
      "makerPair",
      "makerDirection",
      "makerForeignUnits",
      "makerLezUnits",
      "makerSave",
      "makerActive",
      "makerHistory",
      "makerOutput",
    ],
  },
  {
    directory: "taker",
    name: "lez_atomic_swap_taker",
    displayName: "LEZ Atomic Swap Taker",
    main: "lez_atomic_swap_taker_plugin",
    icon: "icons/taker-route.svg",
    environment: "LEZ_TAKER_RPC_SOCKET",
    methods: [
      "taker_health",
      "taker_swap_initiate_v1",
      "taker_swap_list_v1",
      "taker_swap_monitor_v1",
      "taker_swap_claim_v1",
      "taker_swap_refund_v1",
    ],
    slots: ["health", "chatStatus", "connectChat", "connectOffer", "resetChat", "listOffers", "initiate", "listSwaps", "monitor", "claim", "refund"],
    objects: [
      "takerConnection",
      "takerChat",
      "takerChatAddress",
      "takerChatConnect",
      "takerChatStatus",
      "takerChatReset",
      "takerOffers",
      "takerReview",
      "takerOfferId",
      "takerMakerIdentity",
      "takerEnvelopeDigest",
      "takerForeignUnits",
      "takerLezUnits",
      "takerInitiate",
      "takerProgress",
      "takerSwapId",
      "takerMonitor",
      "takerClaim",
      "takerRefund",
      "takerListSwaps",
      "takerShielding",
      "takerOutput",
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
  /logos-module-builder\.follows\s*=\s*"chat_module\/logos-module-builder"/,
  "must follow the Chat release's module-builder",
);
requirePattern(
  rootFlake,
  flakeSource,
  /chat_module\.url\s*=\s*"github:logos-co\/logos-chat-module\/v0\.2\.2"/,
  "must pin the official Chat module v0.2.2 release",
);
requirePattern(
  rootFlake,
  flakeSource,
  /logos-delivery-module\.follows\s*=\s*"chat_module\/logos-delivery-module"/,
  "must follow Chat's exact Delivery release",
);
for (const value of ["maker", "taker"]) {
  requirePattern(
    rootFlake,
    flakeSource,
    new RegExp(`${value}Package\\s*=.*mkLogosQmlModule`, "s"),
    `must build the ${value} package through mkLogosQmlModule`,
  );
  requirePattern(
    rootFlake,
    flakeSource,
    new RegExp(`lez-${value}-ui-install\\s*=.*\\.install`),
    `must expose the official ${value} developer-install tree`,
  );
  for (const suffix of ["", "-lgx", "-install", "-integration-test"]) {
    requirePattern(
      rootFlake,
      flakeSource,
      new RegExp(`lez-${value}-ui${suffix}\\s*=`),
      `must expose canonical lez-${value}-ui${suffix}`,
    );
  }
}
requirePattern(
  rootFlake,
  flakeSource,
  /commonSource/,
  "must inject shared local-RPC and Chat bridge sources into both isolated package builds",
);
for (const [pattern, message] of [
  [/withShortRuntimePath/, "must wrap both official UI checks with a short runtime path"],
  [/export TMPDIR=\/tmp\/lez-ui/, "must keep Qt local sockets below Linux's AF_UNIX path limit"],
  [/export XDG_RUNTIME_DIR="\$TMPDIR"/, "must give Qt one explicit private runtime directory"],
  [/chmod 0700 "\$TMPDIR"/, "must keep the UI-test runtime directory owner-private"],
]) {
  requirePattern(rootFlake, flakeSource, pattern, message);
}
requirePattern(rootFlake, flakeSource, /delivery_module\s*=\s*logos-delivery-module/, "must map the Delivery runtime dependency");
rejectPattern(
  rootFlake,
  flakeSource,
  /--override-input|builtins\.getFlake|github:[^"\s]+\/(?:master|main|dev)(?=["\s])/,
  "must not depend on build-time overrides or floating branches",
);

const lockFile = resolve(basecampRoot, "flake.lock");
const lock = parseJson(lockFile, "consumer flake lock");
if (lock !== null) {
  const rootNode = lock.nodes?.[lock.root];
  const chatName = rootNode?.inputs?.chat_module;
  const chat = typeof chatName === "string" ? lock.nodes?.[chatName] : null;
  const deliveryName = chat?.inputs?.["logos-delivery-module"];
  const delivery = typeof deliveryName === "string" ? lock.nodes?.[deliveryName] : null;
  if (
    chat?.locked?.owner !== "logos-co" ||
    chat?.locked?.repo !== "logos-chat-module" ||
    chat.locked.rev !== chatCommit ||
    chat.locked.narHash !== chatNarHash
  ) {
    report(lockFile, "root input must pin Chat v0.2.2 to the approved commit and NAR hash");
  }
  if (
    delivery?.locked?.owner !== "logos-co" ||
    delivery?.locked?.repo !== "logos-delivery-module" ||
    delivery.locked.rev !== deliveryCommit ||
    delivery.locked.narHash !== deliveryNarHash
  ) {
    report(lockFile, "Chat must pin Delivery v0.2.0 to the approved commit and NAR hash");
  }
  if (
    JSON.stringify(rootNode?.inputs?.["logos-module-builder"]) !==
      JSON.stringify(["chat_module", "logos-module-builder"]) ||
    JSON.stringify(rootNode?.inputs?.["logos-delivery-module"]) !==
      JSON.stringify(["chat_module", "logos-delivery-module"])
  ) {
    report(lockFile, "root builder and Delivery inputs must follow the pinned Chat release");
  }
}

const commonHeader = resolve(basecampRoot, "common/local_json_rpc_client.h");
const commonSourceFile = resolve(basecampRoot, "common/local_json_rpc_client.cpp");
const commonHeaderText = read(commonHeader, "shared local-RPC header");
const commonText = read(commonSourceFile, "shared local-RPC implementation");
const chatHeader = resolve(basecampRoot, "common/logos_chat_bridge.h");
const chatSource = resolve(basecampRoot, "common/logos_chat_bridge.cpp");
const chatHeaderText = read(chatHeader, "shared Logos Chat bridge header");
const chatText = read(chatSource, "shared Logos Chat bridge implementation");
const chatContractText = `${chatHeaderText ?? ""}\n${chatText ?? ""}`;
for (const [pattern, message] of [
  [/QLocalSocket/, "must use Qt's Unix-domain local socket client"],
  [/lstat\s*\(/, "must inspect the socket without following symlinks"],
  [/geteuid\s*\(/, "must bind socket ownership to the effective user"],
  [/S_ISSOCK/, "must require an actual Unix socket"],
  [/0?600/, "must require owner-only socket mode 0600"],
  [/Content-Length/i, "must enforce bounded HTTP Content-Length framing"],
  [/QJsonDocument/, "must parse JSON through Qt's maintained JSON implementation"],
  [/waitForConnected/, "must bound connection readiness"],
]) {
  requirePattern(commonSourceFile, commonText, pattern, message);
}
requirePattern(commonHeader, commonHeaderText, /64\s*\*\s*1024|65536/, "must bound ordinary RPC messages to 64 KiB");
for (const [pattern, message] of [
  [/delivery_state_changed/, "must wait for Chat delivery state"],
  [/conversation_created/, "must bind one direct Chat conversation"],
  [/message_received/, "must consume Chat messages through push events"],
  [/send_message/, "must send gateway frames through E2EE Chat"],
  [/logos_chat_bind_session_v1/, "must pin the owner-local gateway session"],
  [/logos_chat_outbox_peek_v1/, "must read only the fixed gateway outbox"],
  [/logos_chat_outbox_ack_v1/, "must acknowledge only an accepted Chat send"],
  [/logos_chat_outbox_defer_v1/, "must prevent one unavailable Maker conversation from blocking the others"],
  [/logos_chat_ingest_v1/, "must submit inbound frames to the fixed gateway method"],
  [/logos_chat_reset_session_v1/, "must expose an owner-local recovery from an unintended peer binding"],
  [/delivery\.subscribe\s*\(/, "must subscribe to the signed Delivery offer topic"],
  [/delivery\.send\s*\(/, "must rebroadcast signed offer announcements through Delivery"],
  [/maker_offer_announcement_snapshot_v1/, "must obtain announcements from the Maker signer and durable store"],
  [/next_after_offer_id/, "must page Maker snapshots within the owner RPC response bound"],
  [/offerBroadcastCursor_/, "must continue bounded rebroadcast sweeps across timer cycles"],
  [/QScopedValueRollback/, "must prevent overlapping synchronous Maker rebroadcast loops"],
  [/kMaximumPendingOfferIngests/, "must bound deferred inbound Delivery work"],
  [/logos_offer_ingest_v1/, "must authenticate Delivery payloads in the Taker gateway"],
  [/logos_offer_list_v1/, "must browse the bounded live Delivery index"],
  [/logos_offer_select_v1/, "must resolve the signed Chat address without transcription"],
  [/QTimer::singleShot/, "must defer inbound processing out of the Chat event callback"],
]) {
  requirePattern(chatSource, chatContractText, pattern, message);
}
requirePattern(chatHeader, chatHeaderText, /chat\.init\s*\(/, "must initialise the generated Chat client");
for (const [pattern, message] of [
  [/QTcpSocket|QNetworkAccessManager|QWebSocket/, "must not add a direct web transport"],
  [/QProcess|\bsystem\s*\(|\bpopen\s*\(|\bexec[a-z]*\s*\(/, "must not spawn commands"],
  [/qDebug|std::cerr|fprintf\s*\(\s*stderr/, "must not log Chat or RPC payloads"],
]) {
  rejectPattern(chatSource, chatText, pattern, message);
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
      display_name: pkg.displayName,
      version: "0.1.0",
      type: "ui_qml",
      category: "finance",
      interface: "universal",
      main: pkg.main,
      view: "qml/Main.qml",
      icon: pkg.icon,
    };
    for (const [key, value] of Object.entries(expected)) {
      if (metadata[key] !== value) report(metadataFile, `${key} must equal ${JSON.stringify(value)}`);
    }
    if (
      JSON.stringify(metadata.dependencies) !== JSON.stringify(["chat_module", "delivery_module"])
    ) {
      report(metadataFile, "must grant only the pinned Chat module and its Delivery runtime");
    }
    if (
      metadata.dependency_overrides?.delivery_module?.input !== "chat_module" ||
      metadata.dependency_overrides?.delivery_module?.file !== "rust-lib/deps/delivery_module.lidl"
    ) {
      report(metadataFile, "must reuse Chat's bundled Delivery API contract");
    }
    if (typeof metadata.description !== "string" || metadata.description.length < 40) {
      report(metadataFile, "must provide a meaningful user-facing description");
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
    [/LogosChatBridge|logos_chat_bridge/, "must compile the shared Chat gateway bridge"],
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
  requirePattern(backendSource, source, /modules\(\)\.chat_module/, "must use the generated Chat module wrapper");
  requirePattern(backendSource, source, /modules\(\)\.delivery_module/, "must use the generated Delivery module wrapper");
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
  if (pkg.directory === "taker") {
    requirePattern(
      backendSource,
      source,
      /logos_offer_announcement_base64/,
      "must carry the exact signed Delivery proof into Taker admission",
    );
    requirePattern(
      backendSource,
      source,
      /selectOffer\s*\(\s*makerIdentity\s*,\s*offerId\s*\)/,
      "must refresh the short-lived signed proof when the user confirms initiation",
    );
    requirePattern(
      backendSource,
      source,
      /offer_unavailable/,
      "must fail closed when the live selected proof cannot be refreshed",
    );
    rejectPattern(
      backendSource,
      source,
      /taker_offer_list_v1/,
      "must not use the filesystem offer index for Basecamp discovery",
    );
    requirePattern(
      backendSource,
      source,
      /direction\s*!=\s*QStringLiteral\("TakerSellsForeign"\)[\s\S]*direction\s*!=\s*QStringLiteral\("TakerSellsLez"\)/,
      "must accept completed BTC → LEZ and LEZ → BTC evidence",
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
  if (pkg.directory === "taker") {
    requirePattern(
      qmlFile,
      qml,
      /"taker-ui-initiate-"\s*\+\s*envelopeDigest\.text\.slice\(0,\s*32\)/,
      "must derive a stable replay id within the 64-byte RequestId bound",
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
const packageFile = resolve(repositoryRoot, "package.json");
const packageManifest = parseJson(packageFile, "root package manifest");
if (
  packageManifest?.scripts?.["test:m6:basecamp:integration"] !==
  "./scripts/test-m6-basecamp-integration.sh"
) {
  report(packageFile, "must expose the Maker and Taker Basecamp integration runner");
}
const integrationRunnerFile = resolve(repositoryRoot, "scripts/test-m6-basecamp-integration.sh");
const integrationRunner = read(integrationRunnerFile, "Basecamp integration runner");
try {
  if ((statSync(integrationRunnerFile).mode & 0o111) === 0) {
    report(integrationRunnerFile, "Basecamp integration runner must be executable");
  }
} catch {}
for (const [pattern, message] of [
  [/checks\.\$\{system\}\.lez-maker-ui/, "must build the Maker check"],
  [/checks\.\$\{system\}\.lez-taker-ui/, "must build the Taker check"],
  [/static\.crates\.io/, "must repair retired pinned crate endpoints from the immutable archive"],
  [/nix[^\n]*build[^\n]*\"\$maker\" \"\$taker\"/, "must realize both role checks in one invocation"],
]) {
  requirePattern(integrationRunnerFile, integrationRunner, pattern, message);
}
const productTestFile = resolve(basecampRoot, "tests/basecamp-role-product.mjs");
const productTest = read(productTestFile, "official Basecamp product test");
requirePattern(
  productTestFile,
  productTest,
  /gatewayRpc\(\s*"logos_offer_ingest_v1"/,
  "must seed the live Delivery index before exercising prepared Taker initiation",
);
requirePattern(
  productTestFile,
  productTest,
  /logos_offer_announcement_base64/,
  "must carry the exact short-lived signed announcement in the Taker fixture",
);
requirePattern(
  productTestFile,
  productTest,
  /expectService\s*&&\s*!takerFixture/,
  "must not silently skip the prepared Taker discovery journey",
);
requirePattern(
  productTestFile,
  productTest,
  /gatewayRpc\(\s*"logos_offer_list_v1"/,
  "must prove the exact fixture was projected by the live index",
);
requirePattern(
  productTestFile,
  productTest,
  /request\.setTimeout\(/,
  "must bound direct gateway calls made by the product harness",
);

if (errors.length > 0) {
  process.stderr.write(
    `M6 Basecamp package contract: RED (${errors.length} defect${errors.length === 1 ? "" : "s"})\n`,
  );
  for (const error of errors.sort()) process.stderr.write(`- ${error}\n`);
  process.exitCode = 1;
} else {
  process.stdout.write(
    "M6 Basecamp package contract: GREEN (2 ui_qml packages, 19 typed slots, pinned E2EE Chat plus signed Delivery discovery)\n",
  );
}
