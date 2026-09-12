pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// The Taker desk. Components come from the shared kit in
// apps/basecamp/common/qml; the skeleton is generated for both desks.
Item {
    id: root

    readonly property var backend: logos.module("lez_atomic_swap_taker")
    property bool ready: false
    property bool busy: false
    property string output: "No request sent yet"
    property bool rawVisible: false
    property string statusMode: "neutral"
    property string statusTitle: "Connecting"
    property string statusDetail: "Opening the owner-local Node channel"
    property string chatState: "not initialised"
    property string chatAddress: ""
    property var btcMarket: ({ order_book: [], inventory: [], swaps: [], routes: [],
        summary: ({pending_offers: 0, accepted_swaps: 0, completed_swaps: 0}) })
    property bool btcMarketReady: false
    property bool btcMarketBusy: false
    // Market reads in flight. A read costs one Node call per swap, so the
    // periodic silent refresh yields while one is running (reads started every
    // tick regardless queued behind each other and the desk fell ever further
    // behind the Node); an explicit "Refresh market" always goes out.
    property int btcMarketReads: 0
    // Swap filters are on/off toggles; the list is one list, rows that need
    // this desk first, then running, then done under a divider.
    property bool showAttention: true
    property bool showRunning: true
    property bool showDone: true
    // Unix seconds, ticked once a second for every countdown on the desk.
    property real now: Date.now() / 1000
    // The swap whose on-chain details are open, or null.
    property var detailSwap: null
    // ---- The activity log: what the desk asked and what the Node answered,
    // plus every change the background poll notices. Newest last.
    property var activity: []
    function note(kind, text) {
        var stamp = new Date().toLocaleTimeString(Qt.locale("en_US"), "HH:mm:ss")
        var next = root.activity.slice(Math.max(0, root.activity.length - 299))
        next.push({time: stamp, kind: kind, text: String(text)})
        root.activity = next
    }

    function copyText(value) {
        clipboardHelper.text = String(value)
        clipboardHelper.selectAll()
        clipboardHelper.copy()
    }
    function swapBucket(swap) {
        if (swap.state === "completed" || swap.state === "refunded" || swap.state === "failed") return "done"
        if (swap.can_act === true) return "attention"
        return "running"
    }
    function filteredSwaps() {
        var rank = { attention: 0, running: 1, done: 2 }
        var shown = { attention: root.showAttention, running: root.showRunning, done: root.showDone }
        var rows = (root.btcMarket.swaps ?? []).filter(function(swap) { return shown[root.swapBucket(swap)] })
        rows.sort(function(a, b) { return rank[root.swapBucket(a)] - rank[root.swapBucket(b)] })
        return rows
    }
    // The first done row carries the divider.
    function firstDone(swap) {
        var rows = root.filteredSwaps()
        for (var i = 0; i < rows.length; ++i)
            if (root.swapBucket(rows[i]) === "done") return rows[i].ui_swap_id === swap.ui_swap_id
        return false
    }
    function swapCountFor(tab) {
        return (root.btcMarket.swaps ?? []).filter(function(swap) { return root.swapBucket(swap) === tab }).length
    }
    function btcAmount(sats) {
        return (Number(sats ?? 0) / 100000000).toFixed(8)
    }
    function lezAmount(units) {
        return Number(units ?? 0).toLocaleString(Qt.locale("en_US"), "f", 0)
    }
    function formatBtcSats(value) {
        return root.btcAmount(value) + " BTC"
    }
    function formatLez(value) {
        return root.lezAmount(value) + " LEZ"
    }
    function decode(raw) {
        var envelope = JSON.parse(String(raw))
        if (envelope.ok !== true)
            throw new Error(envelope.message || envelope.code || "The Node rejected this request")
        return envelope.result ?? {}
    }
    // Every desk request goes through here: it owns the status strip, the raw
    // reply and the activity log.
    function run(operation, pendingTitle, onSuccess) {
        if (!root.ready) {
            root.output = "Node backend is not ready"
            root.statusMode = "error"
            root.statusTitle = "Node unavailable"
            root.statusDetail = "Wait for the owner-local connection and try again"
            root.note("error", pendingTitle + " · backend not ready")
            return
        }
        root.busy = true
        root.output = "Waiting for owner-local Node..."
        root.statusMode = "working"
        root.statusTitle = pendingTitle
        root.statusDetail = "Sent over the owner-only channel"
        root.note("request", pendingTitle)
        logos.watch(operation,
            function(value) {
                root.busy = false
                root.btcMarketBusy = false
                root.output = String(value)
                try {
                    onSuccess(root.decode(value))
                    root.note("reply", pendingTitle + " · " + root.statusTitle)
                } catch (error) {
                    root.statusMode = "error"
                    root.statusTitle = "Request could not be completed"
                    root.statusDetail = String(error)
                    root.note("error", pendingTitle + " · " + String(error))
                }
            },
            function(error) {
                root.busy = false
                root.btcMarketBusy = false
                root.output = "Backend failure: " + String(error)
                root.statusMode = "error"
                root.statusTitle = "Node backend error"
                root.statusDetail = String(error)
                root.note("error", pendingTitle + " · " + String(error))
            })
    }
    // Snapshot changes the background poll notices, so nothing happens silently.
    function noteMarketChanges(before, after) {
        var previous = {}
        for (var i = 0; i < (before.swaps ?? []).length; ++i) previous[before.swaps[i].ui_swap_id] = before.swaps[i]
        var swaps = after.swaps ?? []
        for (var j = 0; j < swaps.length; ++j) {
            var swap = swaps[j], old = previous[swap.ui_swap_id], id = String(swap.ui_swap_id).slice(0, 12)
            if (!old) root.note("swap", id + " · " + swap.state_label)
            else if (old.state !== swap.state) root.note("swap", id + " · " + old.state + " → " + swap.state)
            if ((!old || old.can_act !== true) && swap.can_act === true) root.note("action", id + " · " + swap.action_label + " is yours to run")
        }
        var openBefore = Number((before.summary ?? {}).pending_offers ?? 0), openAfter = Number((after.summary ?? {}).pending_offers ?? 0)
        if (openBefore !== openAfter) root.note("offers", openAfter + " open offer" + (openAfter === 1 ? "" : "s"))
    }
    function applyBtcMarket(result) {
        var first = !root.btcMarketReady
        if (first) root.note("market", (result.swaps ?? []).length + " swaps · " + Number((result.summary ?? {}).pending_offers ?? 0) + " open offers")
        else root.noteMarketChanges(root.btcMarket, result)
        root.btcMarket = result
        root.btcMarketReady = true
        
    }
    function refreshBtcMarket(silent) {
        if (!root.ready || root.busy || root.btcMarketBusy) return
        if (silent && root.btcMarketReads > 0) return
        root.btcMarketReads += 1
        if (!silent) root.note("request", "Refresh market")
        logos.watch(root.backend.btcMarket(root.walletId()),
            function(value) {
                root.btcMarketReads = Math.max(0, root.btcMarketReads - 1)
                try {
                    if (!silent) root.output = String(value)
                    root.applyBtcMarket(root.decode(value))
                    if (!silent) {
                        root.statusMode = "success"
                        root.statusTitle = "Market refreshed"
                        root.statusDetail = (root.btcMarket.swaps ?? []).length + " swaps · " + Number((root.btcMarket.summary ?? {}).pending_offers ?? 0) + " open offers"
                        root.note("reply", "Refresh market · " + root.statusDetail)
                    }
                } catch (error) {
                    if (!silent) {
                        root.output = String(value)
                        root.statusMode = "error"
                        root.statusTitle = "Market unavailable"
                        root.statusDetail = String(error)
                    }
                    root.note("error", "Market · " + String(error))
                }
            },
            function(error) {
                root.btcMarketReads = Math.max(0, root.btcMarketReads - 1)
                if (!silent) {
                    root.output = "Backend failure: " + String(error)
                    root.statusMode = "error"
                    root.statusTitle = "Market unavailable"
                    root.statusDetail = String(error)
                }
                root.note("error", "Market · " + String(error))
            })
    }
    function health() {
        root.run(root.backend.health(), "Check Node", function(result) {
            var ok = result.ready === true && result.degraded !== true
            root.statusMode = ok ? "success" : "error"
            root.statusTitle = ok ? "Node ready" : "Node needs attention"
            root.statusDetail = "Offer delivery: " + String(result.delivery ?? "unknown")
        })
    }
    function chatStatus() {
        root.run(root.backend.chatStatus(), "Chat status", function(result) {
            root.chatState = String(result.state ?? "unknown")
            root.chatAddress = String(result.address ?? "")
            root.statusMode = result.online === true ? "success" : "working"
            root.statusTitle = result.session_bound === true ? "Chat connected" : result.online === true ? "Chat online" : "Chat starting"
            root.statusDetail = result.session_bound === true ? "Direct conversation bound for this app session" : "Paste the Maker's current session address to connect"
        })
    }
    function resetChat() {
        root.run(root.backend.resetChat(), "Reset Chat", function(result) {
            root.chatState = String(result.state ?? "online")
            root.chatAddress = String(result.address ?? "")
            takerChatAddress.text = ""
            root.statusMode = "working"
            root.statusTitle = "Chat session reset"
            root.statusDetail = "No previous peer binding remains"
        })
    }
    function walletId() {
        return "taker-zurich-01"
    }

    property string selectedOffer: ""
    property string selectedAnnouncementBase64: ""
    property string selectedExpiry: ""
    property string currentSwap: ""
    property string currentState: ""
    function digestHex(value) {
        if (!Array.isArray(value)) return String(value ?? "")
        var result = ""
        for (var index = 0; index < value.length; ++index)
            result += Number(value[index]).toString(16).padStart(2, "0")
        return result
    }
    // The Node's exact quote for `sats` at an offer's lot price, or NaN when
    // the amount is outside the bounds or would need fractional LEZ units.
    function quoteOffer(offer, sats) {
        var amount = Number(sats)
        var lezLot = Number(offer.lez_units_per_lot), satsLot = Number(offer.foreign_units_per_lot)
        if (!/^[0-9]{1,16}$/.test(String(sats)) || !Number.isSafeInteger(amount)
            || amount < Number(offer.minimum_foreign_units) || amount > Number(offer.maximum_foreign_units)
            || !(lezLot > 0) || !(satsLot > 0)) return NaN
        var numerator = amount * lezLot
        return Number.isSafeInteger(numerator) && numerator % satsLot === 0 ? numerator / satsLot : NaN
    }
    // "1 BTC = 100,000 LEZ" for an offer's exact lot price.
    function offerRate(offer) {
        return 100000000 * Number(offer.lez_units_per_lot) / Number(offer.foreign_units_per_lot)
    }
    function rateDisplay(perBitcoin) {
        return "1 BTC = " + perBitcoin.toLocaleString(Qt.locale("en_US"), "f", Number.isInteger(perBitcoin) ? 0 : 2) + " LEZ"
    }
    // The best rate on the book for `direction`: most LEZ per BTC when you
    // pay BTC, fewest when you sell LEZ.
    function bestRate(direction) {
        var best = NaN
        var book = root.btcMarket.order_book ?? []
        for (var i = 0; i < book.length; ++i) {
            if (book[i].direction !== direction) continue
            var rate = root.offerRate(book[i])
            if (!Number.isFinite(best) || (direction === "taker_sells_foreign" ? rate > best : rate < best)) best = rate
        }
        return best
    }
    // How much worse than the best rate an offer is, in percent; 0 is best.
    function behindBest(offer) {
        var best = root.bestRate(offer.direction), rate = root.offerRate(offer)
        return offer.direction === "taker_sells_foreign" ? (best / rate - 1) * 100 : (rate / best - 1) * 100
    }
    // The gap between the two sides of the book, when both are quoted.
    readonly property string bookSpread: {
        var buy = root.bestRate("taker_sells_foreign"), sell = root.bestRate("taker_sells_lez")
        if (!Number.isFinite(buy) || !Number.isFinite(sell)) return ""
        return "spread " + ((sell / buy - 1) * 100).toFixed(2) + "%"
    }
    function takeBtcOffer(offer, sats) {
        if (root.btcMarketBusy || !Number.isSafeInteger(root.quoteOffer(offer, sats))) return
        root.btcMarketBusy = true
        var requestId = "ui-taker-take-offer-" + String(Date.now())
        root.run(root.backend.btcTakeOffer(requestId, root.walletId(), String(offer.offer_id), String(sats)),
            "Take " + String(offer.offer_id) + " for " + root.formatBtcSats(sats), function(result) {
                root.btcMarketBusy = false
                root.applyBtcMarket(result)
                root.statusMode = "success"
                root.statusTitle = "Offer taken"
                root.statusDetail = "The swap is queued; only the owning actors advance it"
            })
    }
    function runTakerAction(swap) {
        if (root.btcMarketBusy || swap.can_act !== true) return
        root.btcMarketBusy = true
        var requestId = "ui-taker-swap-action-" + String(Date.now())
        root.run(root.backend.btcSwapAction(requestId, root.walletId(), String(swap.ui_swap_id), String(swap.action_required)),
            String(swap.action_label), function(result) {
                root.btcMarketBusy = false
                root.applyBtcMarket(result)
                root.statusMode = "working"
                root.statusTitle = "Action submitted"
                root.statusDetail = "Waiting for finalized chain evidence before the next actor turn"
            })
    }
    function connectChat() {
        var address = takerChatAddress.text.trim()
        if (address.length === 0) return
        root.run(root.backend.connectChat(address), "Connect Chat", function(result) {
            root.chatState = String(result.state ?? "online")
            root.statusMode = result.session_bound === true ? "success" : "working"
            root.statusTitle = result.session_bound === true ? "Chat connected" : "Conversation starting"
            root.statusDetail = "The direct session lasts only while both Basecamp apps remain open"
        })
    }
    function chooseNewest(result) {
        var entries = result.offers ?? []
        if (entries.length === 0) {
            root.selectedOffer = ""
            root.selectedAnnouncementBase64 = ""
            var unavailable = Number(result.unavailable_offers ?? 0) + Number(result.locally_contended_offers ?? 0)
            root.statusMode = unavailable > 0 ? "working" : "neutral"
            root.statusTitle = unavailable > 0 ? "Offer is already negotiating or taken" : "No offers available"
            root.statusDetail = unavailable > 0 ? String(unavailable) + " stale offer state(s) suppressed" : "Wait for a Maker rebroadcast"
            return
        }
        var selected = entries[0]
        for (var index = 1; index < entries.length; ++index) {
            var candidate = entries[index].offer ?? entries[index]
            var current = selected.offer ?? selected
            if (Number(candidate.created_at_unix_seconds ?? 0) > Number(current.created_at_unix_seconds ?? 0))
                selected = entries[index]
        }
        var offer = selected.offer ?? selected
        root.selectedAnnouncementBase64 = String(selected.announcement_base64 ?? "")
        var configuration = offer.pair_configuration ?? {}
        var price = offer.price ?? {}
        offerId.text = String(offer.id ?? "")
        makerIdentity.text = String(selected.maker_identity ?? selected.maker_public_key ?? "")
        envelopeDigest.text = root.digestHex(selected.signed_envelope_sha256)
        foreignUnits.text = String(configuration.minimum_foreign_units ?? "")
        var numerator = Number(configuration.minimum_foreign_units) * Number(price.lez_units_per_lot)
        var divisor = Number(price.foreign_units_per_lot)
        lezUnits.text = Number.isSafeInteger(numerator) && Number.isSafeInteger(divisor)
            && divisor > 0 && numerator % divisor === 0 && Number.isSafeInteger(numerator / divisor)
            ? String(numerator / divisor) : ""
        root.selectedOffer = offerId.text
        root.selectedExpiry = Number(offer.expires_at_unix_seconds ?? 0) > 0
            ? new Date(Number(offer.expires_at_unix_seconds) * 1000).toLocaleTimeString(Qt.locale(), "HH:mm") : "—"
        root.statusMode = "success"
        root.statusTitle = "Newest offer selected"
        root.statusDetail = "Signature and live Chat address verified · connecting"
        root.run(root.backend.connectOffer(makerIdentity.text, offerId.text), "Connect the selected Maker", function(connection) {
            root.chatState = String(connection.state ?? "online")
            root.statusMode = connection.session_bound === true ? "success" : "working"
            root.statusTitle = connection.session_bound === true ? "Offer selected · Chat connected" : "Offer selected · conversation starting"
            root.statusDetail = "The signed Maker address was resolved from the Delivery broadcast"
        })
    }
    function browse() {
        root.run(root.backend.listOffers(pair.currentText, direction.currentText), "Browse authenticated offers", function(result) { root.chooseNewest(result) })
    }
    function initiate() {
        root.run(root.backend.initiate("taker-ui-initiate-" + envelopeDigest.text.slice(0, 32),
            offerId.text, pair.currentText, direction.currentText, makerIdentity.text, envelopeDigest.text,
            foreignUnits.text, lezUnits.text, root.selectedAnnouncementBase64),
            "Confirm and initiate", function(result) {
                var swap = result.swap ?? result
                root.currentSwap = String(swap.swap_id ?? swap.id ?? "")
                root.currentState = String(swap.state ?? swap.swap_state ?? "not_activated")
                swapId.text = root.currentSwap
                generation.text = String(swap.progress_generation ?? 0)
                root.statusMode = "success"
                root.statusTitle = result.was_replay === true ? "Acceptance recovered" : "Swap agreement secured"
                root.statusDetail = result.was_replay === true ? "The identical request returned its durable result" : "Both actor bundles are provisioned"
            })
    }
    function listSwaps() {
        root.run(root.backend.listSwaps(), "Refresh registry", function(result) {
            var swaps = Array.isArray(result) ? result : (result.swaps ?? [])
            root.statusMode = "success"
            root.statusTitle = swaps.length === 1 ? "1 swap in the registry" : swaps.length + " swaps in the registry"
            root.statusDetail = "Select a swap ID to inspect its latest state"
        })
    }
    function monitor() {
        root.run(root.backend.monitor(swapId.text), "Refresh state", function(result) {
            root.currentSwap = String(result.swap_id ?? swapId.text)
            root.currentState = String(result.state ?? result.schedule_state ?? "unknown")
            generation.text = String(result.progress_generation ?? result.generation ?? generation.text)
            root.statusMode = "success"
            root.statusTitle = "Swap state refreshed"
            root.statusDetail = root.currentState.split("_").join(" ") + " · generation " + generation.text
        })
    }
    function terminal(action) {
        var requestId = ["taker-ui", action, swapId.text, generation.text].join("-")
        var operation = action === "claim"
            ? root.backend.claim(requestId, swapId.text, generation.text)
            : root.backend.refund(requestId, swapId.text, generation.text)
        root.run(operation, action === "claim" ? "Claim" : "Refund", function(result) {
            root.statusMode = "success"
            root.statusTitle = action === "claim" ? "Claim accepted" : "Refund accepted"
            root.statusDetail = "The generation fence was verified by the actor"
        })
    }


    TextEdit { id: clipboardHelper; visible: false }

    Timer {
        id: btcMarketBootstrapTimer
        interval: 450
        repeat: false
        onTriggered: root.refreshBtcMarket(false)
    }
    Timer {
        interval: 2000
        repeat: true
        running: root.ready
        onTriggered: root.refreshBtcMarket(true)
    }
    Timer {
        interval: 1000
        repeat: true
        running: true
        onTriggered: root.now = Date.now() / 1000
    }

    function connected() {
        root.statusMode = "success"
        root.statusTitle = "Node connected"
        root.statusDetail = "Loading the wallet market"
        root.note("node", "Backend connected")
        btcMarketBootstrapTimer.restart()
    }
    Connections {
        target: logos
        function onViewModuleReadyChanged(moduleName, isReady) {
            if (moduleName !== "lez_atomic_swap_taker") return
            root.ready = isReady && root.backend !== null
            if (root.ready) root.connected()
        }
    }
    Component.onCompleted: {
        root.ready = root.backend !== null && logos.isViewModuleReady("lez_atomic_swap_taker")
        if (root.ready) root.connected()
    }

    Rectangle {
        anchors.fill: parent
        color: "#0A0C11"

        ScrollView {
            id: scroll
            anchors.fill: parent
            anchors.margins: 20
            contentWidth: availableWidth
            clip: true
            ScrollBar.horizontal.policy: ScrollBar.AlwaysOff

            ColumnLayout {
                id: body
                width: scroll.availableWidth
                spacing: 14

                // ----- Header: who this desk is, and whether its Node answers.
                RowLayout {
                    Layout.fillWidth: true; spacing: 16
                    Label {
                        text: "LEZ / BTC — Taker Desk"
                        color: "#F7F8FA"; font.pixelSize: 24; font.weight: Font.Bold; font.letterSpacing: -0.5
                        Layout.fillWidth: true
                    }
                    Label { text: "ACCOUNT"; color: "#6F7A8B"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 1.3 }
                    LuxeCombo {
                        id: takerWallet
                        objectName: "takerBtcWallet"
                        model: ["Zurich Wallet 01 · Taker Node"]
                        implicitWidth: 240
                        onActivated: root.refreshBtcMarket(false)
                    }
                    // What this Node's own wallets hold, from its balance method.
                    ColumnLayout {
                        spacing: 1
                        readonly property var wallet: (root.btcMarket.wallets ?? [])[0] ?? ({})
                        Label {
                            objectName: "takerBtcBalance"
                            text: String(parent.wallet.btc_display ?? "… BTC")
                            color: "#B997FF"; font.pixelSize: 11; font.weight: Font.DemiBold; font.family: "DejaVu Sans Mono"
                        }
                        Label {
                            objectName: "takerLezBalance"
                            text: String(parent.wallet.lez_display ?? "… LEZ")
                            color: "#7EE100"; font.pixelSize: 11; font.weight: Font.DemiBold; font.family: "DejaVu Sans Mono"
                        }
                    }
                    Rectangle {
                        implicitWidth: connectionRow.implicitWidth + 20; implicitHeight: 30; radius: 15
                        color: root.ready ? "#11271F" : "#292318"
                        border.width: 1; border.color: root.ready ? "#497621" : "#62438B"
                        RowLayout {
                            id: connectionRow; anchors.centerIn: parent; spacing: 8
                            Rectangle { implicitWidth: 7; implicitHeight: 7; radius: 4; color: root.ready ? "#7EE100" : "#8950FA" }
                            Label {
                                objectName: "takerConnection"
                                text: root.ready ? "Backend connected" : "Connecting"
                                color: root.ready ? "#B8F57C" : "#C6AAFF"
                                font.pixelSize: 11; font.weight: Font.DemiBold
                            }
                        }
                    }
                }

                StatusStrip {
                    Layout.fillWidth: true
                    mode: root.statusMode
                    title: root.statusTitle
                    detail: root.statusDetail
                    LuxeButton {
                        objectName: "takerHealth"
                        text: "Check Node"; quiet: true
                        enabled: root.ready && !root.busy
                        onClicked: root.health()
                    }
                    LuxeButton {
                        objectName: "takerMarketRefresh"
                        text: "Refresh market"; quiet: true
                        enabled: root.ready && !root.busy && !root.btcMarketBusy
                        onClicked: root.refreshBtcMarket(false)
                    }
                }

                // ----- The desk (left) beside the activity log (right) on wide
                // views; stacked on narrow ones.
                Item {
                    id: deskArea
                    property bool deskWide: scroll.availableWidth >= 1180
                    Layout.fillWidth: true
                    implicitHeight: deskWide
                        ? Math.max(deskColumn.implicitHeight, activityLog.implicitHeight)
                        : deskColumn.implicitHeight + 14 + activityLog.implicitHeight

                    ColumnLayout {
                        id: deskColumn
                        anchors.left: parent.left
                        anchors.right: deskArea.deskWide ? activityLog.left : parent.right
                        anchors.rightMargin: deskArea.deskWide ? 14 : 0
                        anchors.top: parent.top
                        spacing: 14

                        // ----- Swaps this Node is party to.
                        Panel {
                            objectName: "takerBtcMarket"
                            Layout.fillWidth: true
                            SectionTitle { text: "My orders" }
                            RowLayout {
                                spacing: 6
                                FilterTab { objectName: "takerSwapTabAttention"; label: "NEEDS YOU"; count: root.swapCountFor("attention"); alert: root.swapCountFor("attention") > 0; active: root.showAttention; onPicked: root.showAttention = !root.showAttention }
                                FilterTab { label: "RUNNING"; count: root.swapCountFor("running"); active: root.showRunning; onPicked: root.showRunning = !root.showRunning }
                                FilterTab { label: "DONE"; count: root.swapCountFor("done"); active: root.showDone; onPicked: root.showDone = !root.showDone }
                            }
                            Label {
                                visible: root.filteredSwaps().length === 0
                                text: (root.btcMarket.swaps ?? []).length === 0 ? "This wallet has not taken an offer yet." : "Every filter that matches is off."
                                color: "#7F8A9B"; font.pixelSize: 12
                            }
                            Repeater {
                                model: root.filteredSwaps()
                                delegate: SwapRow {
                                    Layout.fillWidth: true
                                    role: "taker"; counterpartyLabel: "MAKER"; actionObjectName: "takerSwapAction"
                                    actionEnabled: root.ready && !root.btcMarketBusy
                                    divider: root.firstDone(modelData) ? "DONE" : ""
                                    now: root.now
                                    detailsObjectName: "takerShowDetails"
                                    onDetails: root.detailSwap = modelData
                                    onAct: root.runTakerAction(modelData)
                                }
                            }
                        }

                        // ----- Available orders: the Maker's open offers.
                        Panel {
                            Layout.fillWidth: true
                            RowLayout {
                                Layout.fillWidth: true
                                SectionTitle { text: "Available orders"; Layout.fillWidth: true }
                                Label { visible: root.bookSpread !== ""; text: root.bookSpread; color: "#9AA6B8"; font.pixelSize: 10; font.family: "DejaVu Sans Mono" }
                                Label { text: String((root.btcMarket.order_book ?? []).length); color: "#B997FF"; font.pixelSize: 12; font.weight: Font.Bold; font.family: "DejaVu Sans Mono" }
                            }
                            Label {
                                visible: (root.btcMarket.order_book ?? []).length === 0
                                text: root.btcMarketReady ? "No open offers." : "Loading the wallet market…"
                                color: "#7F8A9B"; font.pixelSize: 12
                            }
                            Repeater {
                                model: root.btcMarket.order_book ?? []
                                delegate: Rectangle {
                                    id: takerOfferRow
                                    required property var modelData
                                    // The amount this Taker takes, in satoshis: any value
                                    // inside the offer's bounds that quotes to whole LEZ.
                                    readonly property bool ranged: Number(modelData.minimum_foreign_units) !== Number(modelData.maximum_foreign_units)
                                    readonly property real quotedLez: root.quoteOffer(modelData, takeSats.text)
                                    readonly property real behind: root.behindBest(modelData)
                                    Layout.fillWidth: true; implicitHeight: offerColumn.implicitHeight + 24; radius: 9
                                    color: "#0D141E"; border.width: 1; border.color: "#28364A"
                                    ColumnLayout {
                                        id: offerColumn
                                        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                                        anchors.margins: 12; spacing: 8
                                        RowLayout {
                                            Layout.fillWidth: true; spacing: 12
                                            ColumnLayout {
                                                Layout.fillWidth: true; spacing: 2
                                                Label { text: String(takerOfferRow.modelData.maker_wallet_label); color: "#F1F3F6"; font.pixelSize: 12; font.weight: Font.DemiBold }
                                                Label { text: String(takerOfferRow.modelData.offer_id); color: "#68768A"; font.pixelSize: 9; font.family: "DejaVu Sans Mono"; elide: Text.ElideMiddle; Layout.fillWidth: true }
                                            }
                                            Label { text: String(takerOfferRow.modelData.taker_pays_display); color: "#B997FF"; font.pixelSize: 12; font.weight: Font.DemiBold }
                                            Label { text: "→"; color: "#687486"; font.pixelSize: 13 }
                                            Label { text: String(takerOfferRow.modelData.taker_receives_display); color: "#7EE100"; font.pixelSize: 12; font.weight: Font.DemiBold }
                                            LuxeButton {
                                                objectName: "takerTakeOffer"
                                                text: "Take offer"; primary: true
                                                enabled: root.ready && !root.btcMarketBusy && Number.isSafeInteger(takerOfferRow.quotedLez)
                                                onClicked: root.takeBtcOffer(takerOfferRow.modelData, takeSats.text)
                                            }
                                        }
                                        // The exact lot rate, its distance from the best on the
                                        // book, and when the Maker's signature stops being valid.
                                        RowLayout {
                                            Layout.fillWidth: true; spacing: 14
                                            Label { text: root.rateDisplay(root.offerRate(takerOfferRow.modelData)); color: "#D9E2F2"; font.pixelSize: 10; font.family: "DejaVu Sans Mono" }
                                            Label {
                                                text: takerOfferRow.behind > 0.005 ? takerOfferRow.behind.toFixed(2) + "% behind the best" : "best rate"
                                                color: takerOfferRow.behind > 0.005 ? "#FFB8EC" : "#7EE100"; font.pixelSize: 10
                                            }
                                            Item { Layout.fillWidth: true }
                                            Timeline {
                                                moments: [{label: "Expires", at_unix_seconds: takerOfferRow.modelData.expires_at_unix_seconds}]
                                                now: root.now
                                            }
                                        }
                                        // How much of the offer to take: the slider walks the
                                        // lots, the field takes an exact number of satoshis.
                                        RowLayout {
                                            visible: takerOfferRow.ranged
                                            Layout.fillWidth: true; spacing: 10
                                            LuxeSlider {
                                                id: takeShare
                                                objectName: "takerTakeShare"
                                                Layout.fillWidth: true
                                                from: Number(takerOfferRow.modelData.minimum_foreign_units)
                                                to: Number(takerOfferRow.modelData.maximum_foreign_units)
                                                stepSize: Number(takerOfferRow.modelData.foreign_units_per_lot)
                                                value: Number(takerOfferRow.modelData.maximum_foreign_units)
                                                onMoved: takeSats.text = String(Math.round(value))
                                            }
                                            Label {
                                                text: Number.isSafeInteger(takerOfferRow.quotedLez)
                                                    ? Math.round(100 * Number(takeSats.text) / Number(takerOfferRow.modelData.maximum_foreign_units)) + "% of the offer"
                                                    : "outside the terms"
                                                color: Number.isSafeInteger(takerOfferRow.quotedLez) ? "#9AA6B8" : "#FF9FAF"
                                                font.pixelSize: 10; Layout.preferredWidth: 110
                                            }
                                            LuxeField {
                                                id: takeSats
                                                objectName: "takerTakeSats"
                                                Layout.preferredWidth: 150
                                                placeholderText: String(takerOfferRow.modelData.minimum_foreign_units) + "–" + String(takerOfferRow.modelData.maximum_foreign_units)
                                                text: String(takerOfferRow.modelData.maximum_foreign_units ?? "")
                                                onTextChanged: if (!takeShare.pressed && Number.isFinite(Number(text))) takeShare.value = Number(text)
                                            }
                                            Label {
                                                text: Number.isSafeInteger(takerOfferRow.quotedLez) ? "→ " + root.formatLez(takerOfferRow.quotedLez) : ""
                                                color: "#7EE100"; font.pixelSize: 10; Layout.preferredWidth: 120
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // ----- Authenticated offer discovery over Logos Chat,
                        // for pairs negotiated outside the Bitcoin desk.
                        Panel {
                            Layout.fillWidth: true
                            SectionTitle { text: "Authenticated offers" }
                            GridLayout {
                                Layout.fillWidth: true; columns: 2; columnSpacing: 10; rowSpacing: 6
                                FieldLabel { text: "ASSET YOU RECEIVE" }
                                FieldLabel { text: "YOUR SIDE" }
                                LuxeCombo { id: pair; objectName: "takerPair"; model: ["Bitcoin"]; Layout.fillWidth: true }
                                LuxeCombo { id: direction; objectName: "takerDirection"; model: ["TakerSellsForeign", "TakerSellsLez"]; Layout.fillWidth: true }
                            }
                            RowLayout {
                                Layout.fillWidth: true; spacing: 10
                                LuxeButton {
                                    objectName: "takerOffers"
                                    text: "Browse authenticated offers"
                                    enabled: root.ready && !root.busy
                                    onClicked: root.browse()
                                }
                                Label {
                                    text: root.selectedOffer === "" ? "No offer selected" : root.selectedOffer + " · valid until " + root.selectedExpiry
                                    color: root.selectedOffer === "" ? "#7F8A9B" : "#D9E2F2"; font.pixelSize: 11
                                    elide: Text.ElideMiddle; Layout.fillWidth: true
                                }
                            }
                        }

                        Panel {
                            objectName: "takerReview"
                            visible: pair.currentText !== "Bitcoin"
                            Layout.fillWidth: true
                            SectionTitle { text: "Review exact terms" }
                            FieldLabel { text: "OFFER ID" }
                            LuxeField { id: offerId; objectName: "takerOfferId"; placeholderText: "Selected automatically after browsing"; Layout.fillWidth: true; font.family: "DejaVu Sans Mono" }
                            FieldLabel { text: "MAKER PUBLIC IDENTITY" }
                            LuxeField { id: makerIdentity; objectName: "takerMakerIdentity"; placeholderText: "Verified compressed public key"; Layout.fillWidth: true; font.family: "DejaVu Sans Mono" }
                            FieldLabel { text: "SIGNED-ENVELOPE SHA-256" }
                            LuxeField { id: envelopeDigest; objectName: "takerEnvelopeDigest"; placeholderText: "Verified envelope fingerprint"; Layout.fillWidth: true; font.family: "DejaVu Sans Mono" }
                            GridLayout {
                                Layout.fillWidth: true; columns: 2; columnSpacing: 10; rowSpacing: 6
                                FieldLabel { text: direction.currentIndex === 0 ? "YOU RECEIVE · FOREIGN UNITS" : "YOU SEND · FOREIGN UNITS" }
                                FieldLabel { text: direction.currentIndex === 0 ? "YOU SEND · LEZ UNITS" : "YOU RECEIVE · LEZ UNITS" }
                                LuxeField { id: foreignUnits; objectName: "takerForeignUnits"; placeholderText: "Exact foreign units"; Layout.fillWidth: true }
                                LuxeField { id: lezUnits; objectName: "takerLezUnits"; placeholderText: "Exact LEZ units"; Layout.fillWidth: true }
                            }
                            LuxeButton {
                                objectName: "takerInitiate"
                                text: "Confirm and initiate"
                                primary: true
                                enabled: root.ready && !root.busy && offerId.text.length > 0 && makerIdentity.text.length > 0 && envelopeDigest.text.length === 64
                                Layout.fillWidth: true
                                onClicked: root.initiate()
                            }
                        }

                        Panel {
                            objectName: "takerProgress"
                            visible: pair.currentText !== "Bitcoin"
                            Layout.fillWidth: true
                            SectionTitle { text: "Track settlement" }
                            GridLayout {
                                Layout.fillWidth: true; columns: 5; columnSpacing: 10; rowSpacing: 6
                                FieldLabel { text: "SWAP ID"; Layout.columnSpan: 3 }
                                FieldLabel { text: "GENERATION"; Layout.columnSpan: 2 }
                                LuxeField { id: swapId; objectName: "takerSwapId"; placeholderText: "Swap ID"; text: root.currentSwap; Layout.fillWidth: true; Layout.columnSpan: 3; font.family: "DejaVu Sans Mono" }
                                LuxeField { id: generation; placeholderText: "Generation"; text: "0"; Layout.fillWidth: true; Layout.columnSpan: 2 }
                            }
                            RowLayout {
                                Layout.fillWidth: true; spacing: 10
                                LuxeButton { objectName: "takerMonitor"; text: "Refresh state"; primary: true; enabled: root.ready && !root.busy && swapId.text.length > 0; onClicked: root.monitor() }
                                LuxeButton { objectName: "takerListSwaps"; text: "Refresh registry"; enabled: root.ready && !root.busy; onClicked: root.listSwaps() }
                                Label { text: root.currentState.split("_").join(" ").toUpperCase(); color: "#C5A9FF"; font.pixelSize: 9; font.weight: Font.Bold; Layout.fillWidth: true }
                                LuxeButton { objectName: "takerClaim"; text: "Claim"; enabled: root.ready && !root.busy && swapId.text.length > 0; onClicked: root.terminal("claim") }
                                LuxeButton { objectName: "takerRefund"; text: "Refund"; destructive: true; enabled: root.ready && !root.busy && swapId.text.length > 0; onClicked: root.terminal("refund") }
                            }
                            Label {
                                objectName: "takerShielding"
                                text: "Bitcoin amounts and transaction linkage are public; use fresh wallet addresses."
                                color: "#A8AFBB"; font.pixelSize: 11; Layout.fillWidth: true; wrapMode: Text.WordWrap
                            }
                        }


                        // ----- Chat: the private negotiation channel.
                        Panel {
                            objectName: "takerChat"
                            Layout.fillWidth: true
                            RowLayout {
                                Layout.fillWidth: true; spacing: 10
                                SectionTitle { text: "Private negotiation Chat"; Layout.fillWidth: true }
                                Label { text: root.chatState.toUpperCase(); color: "#B997FF"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.8 }
                                LuxeButton {
                                    objectName: "takerChatStatus"
                                    text: "Status"; quiet: true
                                    enabled: root.ready && !root.busy
                                    onClicked: root.chatStatus()
                                }
                                LuxeButton {
                                    objectName: "takerChatReset"
                                    text: "Reset"; quiet: true
                                    enabled: root.ready && !root.busy
                                    onClicked: root.resetChat()
                                }
                            }
                            RowLayout {
                                Layout.fillWidth: true; spacing: 10
                                LuxeField {
                                    id: takerChatAddress
                                    objectName: "takerChatAddress"
                                    placeholderText: "Maker Logos Chat session address"
                                    Layout.fillWidth: true
                                    font.family: "DejaVu Sans Mono"
                                }
                                LuxeButton {
                                    objectName: "takerChatConnect"
                                    text: "Connect"
                                    primary: true
                                    enabled: root.ready && !root.busy && takerChatAddress.text.trim().length > 0
                                    onClicked: root.connectChat()
                                }
                            }
                        }
                    }

                    ActivityLog {
                        id: activityLog
                        objectName: "takerActivity"
                        anchors.top: deskArea.deskWide ? parent.top : deskColumn.bottom
                        anchors.topMargin: deskArea.deskWide ? 0 : 14
                        anchors.right: parent.right
                        width: deskArea.deskWide ? 380 : parent.width
                        entries: root.activity
                        onCopyRequested: function(text) { root.copyText(text) }
                        onClearRequested: root.activity = []
                        RowLayout {
                            Layout.fillWidth: true
                            Label { text: "Raw last reply"; color: "#8B96A8"; font.pixelSize: 11; Layout.fillWidth: true }
                            LuxeButton { text: root.rawVisible ? "Hide" : "Show"; quiet: true; onClicked: root.rawVisible = !root.rawVisible }
                        }
                        TextArea {
                            objectName: "takerOutput"
                            text: root.output
                            visible: root.rawVisible
                            readOnly: true
                            wrapMode: Text.WrapAnywhere
                            selectByMouse: true
                            Layout.fillWidth: true
                            Layout.preferredHeight: root.rawVisible ? 160 : 0
                            color: "#BAC4D3"
                            selectionColor: "#8950FA"
                            selectedTextColor: "#FFFFFF"
                            font.family: "DejaVu Sans Mono"
                            font.pixelSize: 10
                            leftPadding: 10; rightPadding: 10; topPadding: 8; bottomPadding: 8
                            background: Rectangle { color: "#080C12"; radius: 8; border.width: 1; border.color: "#253043" }
                        }
                    }
                }

                Item { Layout.fillWidth: true; implicitHeight: 4 }
            }
        }
    }
    Rectangle {
        // The swap's on-chain transactions, as the Node's actor recorded them.
        id: swapDetailsOverlay
        anchors.fill: parent
        visible: root.detailSwap !== null
        z: 1001
        color: "#D0060A12"
        MouseArea { anchors.fill: parent; onClicked: root.detailSwap = null }
        Panel {
            objectName: "takerSwapDetails"
            anchors.centerIn: parent
            width: 640
            border.color: "#8950FA"
            MouseArea { anchors.fill: parent; z: -1 }
            RowLayout {
                Layout.fillWidth: true; spacing: 10
                SectionTitle { text: "On-chain details"; Layout.fillWidth: true }
                LuxeButton { text: "Close"; quiet: true; onClicked: root.detailSwap = null }
            }
            Label {
                text: root.detailSwap ? String(root.detailSwap.state_label) + " · " + String(root.detailSwap.direction_display) + "  ·  " + String(root.detailSwap.amounts_display ?? "") : ""
                color: "#D9E2F2"; font.pixelSize: 12; font.weight: Font.DemiBold
                elide: Text.ElideRight; Layout.fillWidth: true
            }
            Label {
                text: root.detailSwap ? String(root.detailSwap.ui_swap_id) : ""
                color: "#68768A"; font.pixelSize: 9; font.family: "DejaVu Sans Mono"
                elide: Text.ElideMiddle; Layout.fillWidth: true
            }
            Repeater {
                model: root.detailSwap ? (root.detailSwap.effects ?? []) : []
                delegate: Rectangle {
                    id: effectRow
                    required property var modelData
                    Layout.fillWidth: true; implicitHeight: effectColumn.implicitHeight + 20; radius: 8
                    color: "#0D141E"; border.width: 1; border.color: "#28364A"
                    ColumnLayout {
                        id: effectColumn
                        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                        anchors.margins: 10; spacing: 3
                        RowLayout {
                            Layout.fillWidth: true; spacing: 10
                            Label { text: String(effectRow.modelData.kind_display).toUpperCase(); color: "#D8C6FF"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.8 }
                            Label { text: String(effectRow.modelData.chain).toUpperCase(); color: effectRow.modelData.chain === "Bitcoin" ? "#B997FF" : "#7EE100"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.8 }
                            Label { visible: Number(effectRow.modelData.confirmations) > 0; text: String(effectRow.modelData.confirmations) + " conf"; color: "#9AA6B8"; font.pixelSize: 9; font.family: "DejaVu Sans Mono" }
                            Item { Layout.fillWidth: true }
                            LuxeButton { text: "Copy id"; quiet: true; onClicked: root.copyText(effectRow.modelData.transaction_id) }
                            LuxeButton { text: "Copy link"; quiet: true; onClicked: root.copyText(effectRow.modelData.explorer_url) }
                        }
                        Label { text: String(effectRow.modelData.transaction_id); color: "#F1F3F6"; font.pixelSize: 10; font.family: "DejaVu Sans Mono"; elide: Text.ElideMiddle; Layout.fillWidth: true }
                        Label { text: String(effectRow.modelData.explorer_url); color: "#68768A"; font.pixelSize: 9; font.family: "DejaVu Sans Mono"; elide: Text.ElideMiddle; Layout.fillWidth: true }
                    }
                }
            }
        }
    }

}
