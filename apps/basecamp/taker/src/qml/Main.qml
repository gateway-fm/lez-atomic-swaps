pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {
    id: root

    readonly property var backend: logos.module("lez_atomic_swap_taker")
    property bool ready: false
    property bool busy: false
    property bool technicalVisible: false
    property string output: "Browse authenticated offers to begin"
    property string statusMode: "neutral"
    property string statusTitle: "Connecting securely"
    property string statusDetail: "Establishing the owner-local Node channel"
    property string selectedOffer: ""
    property string selectedAnnouncementBase64: ""
    property string selectedExpiry: ""
    property int availableOffers: 0
    property string currentState: ""
    property string currentSwap: ""
    property bool replayed: false
    property var btcEvidence: ({})
    property bool btcEvidenceReady: false
    property var btcMarket: ({
        order_book: [], swaps: [], wallets: [],
        summary: ({pending_offers: 0, accepted_swaps: 0, completed_swaps: 0}),
        runner_ready: false, runner_busy: false,
        runner_detail: "Checking the Taker Node"
    })
    property bool btcMarketReady: false
    property bool btcMarketBusy: false
    property string swapTab: "attention"
    property var expandedSwaps: ({})
    property string chatState: "not initialised"

    function toggleSwapHashes(uiSwapId) {
        var next = {}
        for (var key in root.expandedSwaps) next[key] = root.expandedSwaps[key]
        next[uiSwapId] = !next[uiSwapId]
        root.expandedSwaps = next
    }
    function copyText(value) {
        clipboardHelper.text = String(value)
        clipboardHelper.selectAll()
        clipboardHelper.copy()
    }

    function swapBucket(swap) {
        if (swap.state === "completed" || swap.state === "failed") return "done"
        if (swap.can_act === true) return "attention"
        return "running"
    }
    function filteredSwaps() {
        var all = root.btcMarket.swaps ?? []
        if (root.swapTab === "all") return all
        return all.filter(function(swap) { return root.swapBucket(swap) === root.swapTab })
    }
    // Another account of this role may hold the open gate. Without this the
    // desk shows an empty NEEDS YOU and no way to discover which wallet waits.
    function otherWalletNeedingAction() {
        var wallets = root.btcMarket.wallets ?? []
        for (var i = 0; i < wallets.length; i++) {
            if (wallets[i].id !== root.btcMarket.selected_wallet_id
                && Number(wallets[i].needs_action ?? 0) > 0) {
                return {index: i, label: wallets[i].label,
                        count: Number(wallets[i].needs_action)}
            }
        }
        return null
    }
    function swapCountFor(tab) {
        return (root.btcMarket.swaps ?? []).filter(function(swap) {
            return root.swapBucket(swap) === tab
        }).length
    }
    property string lastPublishedMarketRun: ""
    // Newest completed market run the desk already asked the evidence file about,
    // so a run that is not published yet is probed once, not on every poll.
    property string lastProbedMarketRun: ""

    component LuxeButton: Button {
        id: control
        property bool primary: false
        property bool quiet: false
        property bool destructive: false
        hoverEnabled: true
        implicitHeight: 44
        leftPadding: 18
        rightPadding: 18
        font.pixelSize: 13
        font.weight: Font.DemiBold
        contentItem: Label {
            text: control.text
            color: !control.enabled ? "#6F7787"
                : control.primary ? "#FFFFFF"
                : control.destructive ? "#FF9BE0" : "#F3F5F8"
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
            font: control.font
        }
        background: Rectangle {
            radius: 10
            color: !control.enabled ? "#181E29"
                : control.primary ? (control.down ? "#7139DA" : control.hovered ? "#9D72FF" : "#8950FA")
                : control.destructive ? (control.hovered ? "#3B1C34" : "#251621")
                : control.quiet ? (control.hovered ? "#1B2432" : "transparent")
                : (control.down ? "#202938" : control.hovered ? "#252F40" : "#1B2330")
            border.width: control.primary || control.quiet ? 0 : 1
            border.color: control.destructive ? "#8F3A77" : "#344052"
            Behavior on color { ColorAnimation { duration: 120 } }
        }
    }

    component LuxeField: TextField {
        id: control
        implicitHeight: 44
        color: "#F5F7FA"
        placeholderTextColor: "#707A8B"
        selectionColor: "#8950FA"
        selectedTextColor: "#0C1017"
        font.pixelSize: 13
        leftPadding: 14
        rightPadding: 14
        selectByMouse: true
        background: Rectangle {
            radius: 9
            color: control.readOnly ? "#0E141E" : "#111925"
            border.width: 1
            border.color: control.activeFocus ? "#8950FA" : control.hovered ? "#46536A" : "#2A3547"
            Behavior on border.color { ColorAnimation { duration: 120 } }
        }
    }

    component LuxeCombo: ComboBox {
        id: control
        implicitHeight: 44
        hoverEnabled: true
        font.pixelSize: 13
        font.weight: Font.Medium
        contentItem: Label {
            text: control.displayText
            color: "#F5F7FA"
            verticalAlignment: Text.AlignVCenter
            leftPadding: 14
            rightPadding: 36
            font: control.font
        }
        indicator: Label {
            x: control.width - width - 14
            y: (control.height - height) / 2 - 1
            text: "⌄"
            color: "#7EE100"
            font.pixelSize: 18
        }
        background: Rectangle {
            radius: 9
            color: control.hovered ? "#182130" : "#111925"
            border.width: 1
            border.color: control.activeFocus ? "#8950FA" : "#2A3547"
        }
        delegate: ItemDelegate {
            id: option
            required property int index
            required property var modelData
            width: control.width
            height: 40
            contentItem: Label {
                text: option.modelData
                color: option.highlighted ? "#B997FF" : "#E8ECF2"
                verticalAlignment: Text.AlignVCenter
                leftPadding: 10
                font.pixelSize: 13
            }
            background: Rectangle { color: option.highlighted ? "#222D3D" : "transparent"; radius: 7 }
            highlighted: control.highlightedIndex === option.index
        }
        popup: Popup {
            y: control.height + 5
            width: control.width
            implicitHeight: contentItem.implicitHeight + 8
            padding: 4
            contentItem: ListView {
                clip: true
                implicitHeight: contentHeight
                model: control.popup.visible ? control.delegateModel : null
                currentIndex: control.highlightedIndex
            }
            background: Rectangle {
                color: "#111925"
                radius: 10
                border.width: 1
                border.color: "#344052"
            }
        }
    }

    TextEdit { id: clipboardHelper; visible: false }

    component FieldLabel: Label {
        color: "#AAB3C2"
        font.pixelSize: 11
        font.weight: Font.DemiBold
        font.letterSpacing: 0.4
    }

    component StepBadge: Rectangle {
        property string number: "01"
        property color accent: "#8950FA"
        implicitWidth: 46
        implicitHeight: 26
        radius: 2
        color: "#151820"
        border.width: 1
        border.color: accent
        Label {
            anchors.centerIn: parent
            text: parent.number
            color: parent.accent
            font.pixelSize: 11
            font.weight: Font.Bold
            font.letterSpacing: 1.4
        }
    }

    component FilterTab: Rectangle {
        id: filterTab
        property string label: ""
        property int count: 0
        property bool active: false
        property bool showCount: true
        signal picked()
        implicitHeight: 28
        implicitWidth: filterTabRow.implicitWidth + 24
        radius: 2
        color: filterTab.active ? "#1D2739" : filterTabArea.containsMouse ? "#151D2A" : "transparent"
        border.width: 1
        border.color: filterTab.active ? "#8950FA" : "#2A3547"
        RowLayout {
            id: filterTabRow
            anchors.centerIn: parent
            spacing: 6
            Label {
                text: filterTab.label
                color: filterTab.active ? "#EDEFF4" : "#7F8A9B"
                font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 1.1
            }
            Label {
                visible: filterTab.showCount
                text: String(filterTab.count)
                color: filterTab.active ? "#B997FF" : "#5F6B7D"
                font.pixelSize: 9; font.weight: Font.Bold; font.family: "DejaVu Sans Mono"
            }
        }
        MouseArea {
            id: filterTabArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: filterTab.picked()
        }
    }

    component EvidenceCard: Rectangle {
        id: evidenceCard
        required property var effect
        property color accent: effect.chain === "Bitcoin" ? "#8950FA" : "#7EE100"
        Layout.fillWidth: true
        implicitHeight: 218
        radius: 12
        color: "#0D141E"
        border.width: 1
        border.color: effect.chain === "Bitcoin" ? "#6846A2" : "#3D671E"

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: 14
            spacing: 8
            RowLayout {
                Layout.fillWidth: true
                Rectangle {
                    implicitWidth: 30; implicitHeight: 24; radius: 2
                    color: "#151820"; border.width: 1; border.color: evidenceCard.accent
                    Label {
                        anchors.centerIn: parent
                        text: String(evidenceCard.effect.sequence).padStart(2, "0")
                        color: evidenceCard.accent; font.pixelSize: 9; font.weight: Font.Bold
                    }
                }
                ColumnLayout {
                    Layout.fillWidth: true; spacing: 1
                    Label {
                        text: evidenceCard.effect.label
                        color: "#F4F6F8"; font.pixelSize: 12; font.weight: Font.DemiBold
                        elide: Text.ElideRight; Layout.fillWidth: true
                    }
                    Label {
                        text: evidenceCard.effect.chain.toUpperCase() + " · " + evidenceCard.effect.actor.toUpperCase()
                        color: evidenceCard.accent; font.pixelSize: 8; font.weight: Font.Bold; font.letterSpacing: 0.8
                    }
                }
            }
            Label {
                text: evidenceCard.effect.amount
                color: "#C8D0DB"; font.pixelSize: 11; font.weight: Font.DemiBold
            }
            FieldLabel { text: "TRANSACTION ID" }
            TextArea {
                text: evidenceCard.effect.transaction_id
                readOnly: true; selectByMouse: true; wrapMode: Text.WrapAnywhere
                Layout.fillWidth: true; Layout.preferredHeight: 56
                color: "#D8DEE8"; selectionColor: evidenceCard.accent; selectedTextColor: "#080A0E"
                font.family: "DejaVu Sans Mono"; font.pixelSize: 9
                leftPadding: 9; rightPadding: 9; topPadding: 7; bottomPadding: 7
                background: Rectangle { color: "#080C12"; radius: 7; border.width: 1; border.color: "#243043" }
            }
            RowLayout {
                Layout.fillWidth: true
                Label {
                    text: evidenceCard.effect.finality.toUpperCase()
                    color: evidenceCard.accent; font.pixelSize: 8; font.weight: Font.Bold; font.letterSpacing: 0.7
                }
                Item { Layout.fillWidth: true }
                Label {
                    text: evidenceCard.effect.block_height === null
                        ? "BLOCK PROOF ATTACHED" : "BLOCK " + evidenceCard.effect.block_height
                    color: "#7D899A"; font.pixelSize: 8; font.weight: Font.DemiBold
                }
            }
            LuxeButton {
                text: "Open local proof"
                quiet: true; implicitHeight: 30; Layout.fillWidth: true
                onClicked: Qt.openUrlExternally(String(evidenceCard.effect.explorer_url))
            }
        }
    }

    Timer {
        id: btcEvidenceTimer
        interval: 250
        repeat: false
        onTriggered: root.loadBtcEvidence()
    }

    Timer {
        id: btcMarketBootstrapTimer
        interval: 450
        repeat: false
        onTriggered: root.refreshBtcMarket(false)
    }

    Timer {
        id: btcMarketPollTimer
        interval: 2000
        repeat: true
        running: root.ready
        onTriggered: root.refreshBtcMarket(true)
    }

    Connections {
        target: logos
        function onViewModuleReadyChanged(moduleName, isReady) {
            if (moduleName !== "lez_atomic_swap_taker") return
            root.ready = isReady && root.backend !== null
            if (root.ready) {
                root.statusMode = "success"
                root.statusTitle = "Taker Node connected"
                root.statusDetail = "Loading certified LEZ / Bitcoin settlement evidence"
                btcEvidenceTimer.restart()
                btcMarketBootstrapTimer.restart()
            }
        }
    }

    Component.onCompleted: {
        root.ready = root.backend !== null && logos.isViewModuleReady("lez_atomic_swap_taker")
        if (root.ready) {
            root.statusMode = "success"
            root.statusTitle = "Taker Node connected"
            root.statusDetail = "Loading certified LEZ / Bitcoin settlement evidence"
            btcEvidenceTimer.restart()
            btcMarketBootstrapTimer.restart()
        }
    }

    function digestHex(value) {
        if (!Array.isArray(value)) return String(value ?? "")
        var result = ""
        for (var index = 0; index < value.length; ++index)
            result += Number(value[index]).toString(16).padStart(2, "0")
        return result
    }

    function decode(raw) {
        var envelope = JSON.parse(String(raw))
        if (envelope.ok !== true)
            throw new Error(envelope.message || envelope.code || "The Taker Node rejected this request")
        return envelope.result ?? {}
    }

    function run(operation, pendingTitle, onSuccess) {
        if (!root.ready) {
            root.output = "Taker Node backend is not ready"
            root.statusMode = "error"
            root.statusTitle = "Taker Node unavailable"
            root.statusDetail = "Wait for the secure local connection and try again"
            return
        }
        root.busy = true
        root.output = "Waiting for owner-local Node..."
        root.statusMode = "working"
        root.statusTitle = pendingTitle
        root.statusDetail = "Verifying the request over the owner-only channel"
        logos.watch(operation,
            function(value) {
                root.busy = false
                root.btcMarketBusy = false
                root.output = String(value)
                try {
                    onSuccess(root.decode(value))
                } catch (error) {
                    root.statusMode = "error"
                    root.statusTitle = "Request could not be completed"
                    root.statusDetail = String(error)
                    root.technicalVisible = true
                }
            },
            function(error) {
                root.busy = false
                root.btcMarketBusy = false
                root.output = "Backend failure: " + String(error)
                root.statusMode = "error"
                root.statusTitle = "Secure Taker Node error"
                root.statusDetail = String(error)
                root.technicalVisible = true
            })
    }

    function chooseNewest(result) {
        var entries = result.offers ?? []
        root.availableOffers = entries.length
        if (entries.length === 0) {
            root.selectedOffer = ""
            root.selectedAnnouncementBase64 = ""
            var unavailable = Number(result.unavailable_offers ?? 0)
                + Number(result.locally_contended_offers ?? 0)
            root.statusMode = unavailable > 0 ? "working" : "neutral"
            root.statusTitle = unavailable > 0
                ? "Offer is already negotiating or taken"
                : "No offers available"
            root.statusDetail = unavailable > 0
                ? String(unavailable) + " signed offer state update(s) suppressed stale availability"
                : "Wait for a Maker rebroadcast or choose another market"
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
        var numerator = Number(configuration.minimum_foreign_units)
            * Number(price.lez_units_per_lot)
        var divisor = Number(price.foreign_units_per_lot)
        lezUnits.text = Number.isSafeInteger(numerator) && Number.isSafeInteger(divisor)
            && divisor > 0 && numerator % divisor === 0
            && Number.isSafeInteger(numerator / divisor)
            ? String(numerator / divisor) : ""
        root.selectedOffer = offerId.text
        root.selectedExpiry = Number(offer.expires_at_unix_seconds ?? 0) > 0
            ? new Date(Number(offer.expires_at_unix_seconds) * 1000).toLocaleTimeString(Qt.locale(), "HH:mm") : "—"
        root.statusMode = "success"
        root.statusTitle = "Newest offer selected"
        root.statusDetail = "Signature and live Chat address verified · connecting automatically"
        root.run(root.backend.connectOffer(makerIdentity.text, offerId.text),
            "Connecting the selected Maker", function(connection) {
                root.chatState = String(connection.state ?? "online")
                root.statusMode = connection.session_bound === true ? "success" : "working"
                root.statusTitle = connection.session_bound === true
                    ? "Offer selected · private Chat connected"
                    : "Offer selected · conversation starting"
                root.statusDetail = "The signed Maker address was resolved from the Delivery broadcast"
            })
    }

    function browse() {
        root.run(root.backend.listOffers(pair.currentText, direction.currentText),
            "Scanning authenticated offers", function(result) { root.chooseNewest(result) })
    }

    function health() {
        root.run(root.backend.health(), "Checking Taker Node health", function(result) {
            root.statusMode = result.ready === true ? "success" : "error"
            root.statusTitle = result.ready === true ? "All systems ready" : "Taker Node needs attention"
            root.statusDetail = "Offer delivery: " + String(result.delivery ?? "unknown")
        })
    }

    function chatStatus() {
        root.run(root.backend.chatStatus(), "Reading Logos Chat session", function(result) {
            root.chatState = String(result.state ?? "unknown")
            root.statusMode = result.online === true ? "success" : "working"
            root.statusTitle = result.session_bound === true ? "Private Chat connected" : "Private Chat is ready"
            root.statusDetail = result.session_bound === true
                ? "Direct Maker conversation bound for this app session"
                : "Paste the Maker's current session address to connect"
        })
    }

    function connectChat() {
        var address = takerChatAddress.text.trim()
        if (address.length === 0) return
        root.run(root.backend.connectChat(address), "Connecting private Logos Chat", function(result) {
            root.chatState = String(result.state ?? "online")
            root.statusMode = result.session_bound === true ? "success" : "working"
            root.statusTitle = result.session_bound === true ? "Private Chat connected" : "Conversation is starting"
            root.statusDetail = "The direct session lasts only while both Basecamp apps remain open"
        })
    }

    function resetChat() {
        root.run(root.backend.resetChat(), "Resetting private Chat session", function(result) {
            takerChatAddress.text = ""
            root.chatState = String(result.state ?? "online")
            root.statusMode = "working"
            root.statusTitle = "Private Chat session reset"
            root.statusDetail = "Paste the intended Maker's current address to bind a new peer"
        })
    }

    function applyBtcEvidence(result) {
        root.btcEvidence = result
        root.btcEvidenceReady = true
        root.lastPublishedMarketRun = String(result.run_id ?? "")
    }

    // The visible load: the operator asked for the proof, so it owns the status.
    function loadBtcEvidence() {
        if (root.busy) return
        root.run(root.backend.btcEvidence(), "Loading Bitcoin settlement proof", function(result) {
            root.applyBtcEvidence(result)
            root.statusMode = "success"
            root.statusTitle = "BTC settlement verified · revision " + String(result.terminal.revision)
            root.statusDetail = "2 Bitcoin + 3 LEZ effects · completed without replay resubmission"
        })
    }

    // The background load driven by the market poll: refreshes the proof data
    // without touching the status or the diagnostic output the operator is
    // reading, so a "Check Node" or "Refresh wallet market" result stays put.
    function refreshBtcEvidenceSilently() {
        if (root.busy) return
        logos.watch(root.backend.btcEvidence(),
            function(value) {
                try { root.applyBtcEvidence(root.decode(value)) } catch (error) {}
            },
            function(error) {})
    }

    // The Taker Node settles as one identity; the desk shows it as its wallet.
    function selectedTakerWallet() {
        return "taker-zurich-01"
    }

    function walletBalance(role) {
        var ledger = root.btcEvidence.wallet_balance_changes ?? {}
        var wallets = ledger.wallets ?? []
        for (var index = 0; index < wallets.length; ++index)
            if (wallets[index].role === role) return wallets[index]
        return null
    }

    function formatBtcSats(value) {
        return (Number(value ?? 0) / 100000000).toFixed(8) + " BTC"
    }

    function formatSignedBtc(value) {
        var amount = Number(value ?? 0)
        return (amount >= 0 ? "+" : "−") + root.formatBtcSats(Math.abs(amount))
    }

    function formatLez(value) {
        return Number(value ?? 0).toLocaleString(Qt.locale(), "f", 0) + " LEZ"
    }

    function formatSignedLez(value) {
        var amount = Number(value ?? 0)
        return (amount >= 0 ? "+" : "−") + root.formatLez(Math.abs(amount))
    }

    // The newest completed swap decides whether the published proof is stale.
    // Older completed runs stay in the market forever; comparing every one of
    // them against the proof would reload it on every poll.
    function newestCompletedRun(swaps) {
        var newest = null
        for (var index = 0; index < swaps.length; ++index) {
            var swap = swaps[index]
            if (swap.state !== "completed" || String(swap.run_id ?? "") === "") continue
            if (newest === null
                    || String(swap.completed_at ?? "") > String(newest.completed_at ?? "")
                    || (String(swap.completed_at ?? "") === String(newest.completed_at ?? "")
                        && String(swap.run_id) > String(newest.run_id)))
                newest = swap
        }
        return newest === null ? "" : String(newest.run_id)
    }

    function applyBtcMarket(result) {
        root.btcMarket = result
        root.btcMarketReady = true
        var newest = root.newestCompletedRun(result.swaps ?? [])
        if (newest === "" || newest === root.lastProbedMarketRun) return
        if (root.btcEvidenceReady && String(root.btcEvidence.run_id) === newest) return
        root.lastProbedMarketRun = newest
        root.refreshBtcEvidenceSilently()
    }

    function refreshBtcMarket(silent) {
        if (!root.ready || root.busy || root.btcMarketBusy) return
        logos.watch(root.backend.btcMarket(root.selectedTakerWallet()),
            function(value) {
                try {
                    if (!silent) root.output = String(value)
                    root.applyBtcMarket(root.decode(value))
                } catch (error) {
                    if (!silent) {
                        root.statusMode = "error"
                        root.statusTitle = "BTC market unavailable"
                        root.statusDetail = String(error)
                    }
                }
            },
            function(error) {
                if (!silent) root.output = "Backend failure: " + String(error)
                if (!silent) {
                    root.statusMode = "error"
                    root.statusTitle = "BTC market unavailable"
                    root.statusDetail = String(error)
                }
            })
    }

    function takeBtcOffer(offer) {
        if (root.btcMarketBusy) return
        root.btcMarketBusy = true
        var requestId = "ui-taker-take-offer-" + String(Date.now())
        root.run(root.backend.btcTakeOffer(
            requestId, root.selectedTakerWallet(), String(offer.offer_id)),
            "Accepting Maker offer", function(result) {
                root.btcMarketBusy = false
                root.applyBtcMarket(result)
                root.statusMode = "success"
                root.statusTitle = "Offer accepted by " + takerWallet.currentText
                root.statusDetail = "The swap is queued; only the owning actors can advance it"
            })
    }

    function runTakerAction(swap) {
        if (root.btcMarketBusy || swap.can_act !== true) return
        root.btcMarketBusy = true
        var requestId = "ui-taker-swap-action-" + String(Date.now())
        root.run(root.backend.btcSwapAction(requestId, root.selectedTakerWallet(),
            String(swap.ui_swap_id), String(swap.action_required)),
            String(swap.action_label), function(result) {
                root.btcMarketBusy = false
                root.applyBtcMarket(result)
                root.statusMode = "working"
                root.statusTitle = "Taker action submitted"
                root.statusDetail = "Waiting for finalized chain evidence before the next actor turn"
            })
    }

    function initiate() {
        root.run(root.backend.initiate(
            "taker-ui-initiate-" + envelopeDigest.text.slice(0, 32),
            offerId.text, pair.currentText,
            direction.currentText, makerIdentity.text, envelopeDigest.text,
            foreignUnits.text, lezUnits.text, root.selectedAnnouncementBase64),
            "Securing the swap agreement", function(result) {
                var swap = result.swap ?? result
                root.currentSwap = String(swap.swap_id ?? swap.id ?? "")
                root.currentState = String(swap.state ?? swap.swap_state ?? "not_activated")
                root.replayed = result.was_replay === true
                swapId.text = root.currentSwap
                generation.text = String(swap.progress_generation ?? 0)
                root.statusMode = "success"
                root.statusTitle = root.replayed ? "Acceptance recovered safely" : "Swap agreement secured"
                root.statusDetail = root.replayed
                    ? "The identical request returned its durable result"
                    : "Both actor bundles are provisioned and ready"
            })
    }

    function listSwaps() {
        root.run(root.backend.listSwaps(), "Loading your swaps", function(result) {
            var swaps = Array.isArray(result) ? result : (result.swaps ?? [])
            root.statusMode = "success"
            root.statusTitle = swaps.length === 1 ? "1 swap in your registry" : swaps.length + " swaps in your registry"
            root.statusDetail = "Select a swap ID to inspect its latest state"
        })
    }

    function monitor() {
        root.run(root.backend.monitor(swapId.text), "Reading actor progress", function(result) {
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
        root.run(operation, action === "claim" ? "Submitting claim" : "Submitting refund", function(result) {
            root.statusMode = "success"
            root.statusTitle = action === "claim" ? "Claim accepted" : "Refund accepted"
            root.statusDetail = "The generation fence was verified by the actor"
        })
    }

    Rectangle {
        anchors.fill: parent
        color: "#08090D"
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#10111A" }
            GradientStop { position: 0.55; color: "#090A0F" }
            GradientStop { position: 1.0; color: "#07080B" }
        }

        Rectangle {
            width: 520; height: 520; radius: 260
            anchors.top: parent.top; anchors.right: parent.right
            anchors.topMargin: -310; anchors.rightMargin: -150
            color: "#8950FA"; opacity: 0.13
        }

        ScrollView {
            id: scroll
            anchors.fill: parent
            anchors.margins: 26
            contentWidth: availableWidth
            clip: true
            ScrollBar.horizontal.policy: ScrollBar.AlwaysOff

            ColumnLayout {
                id: body
                width: scroll.availableWidth
                spacing: 16

                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: 154
                    radius: 18
                    color: "#111824"
                    border.width: 1
                    border.color: "#273246"
                    clip: true

                    Rectangle {
                        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                        height: 4
                        Row {
                            anchors.fill: parent
                            Rectangle { width: parent.width * 0.46; height: parent.height; color: "#8950FA" }
                            Rectangle { width: parent.width * 0.28; height: parent.height; color: "#7EE100" }
                            Rectangle { width: parent.width * 0.26; height: parent.height; color: "#FA50C1" }
                        }
                    }

                    RowLayout {
                        anchors.fill: parent
                        anchors.margins: 24
                        spacing: 24

                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 7
                            Label {
                                text: "M3 · LIVE LOCAL SWAP · BITCOIN / LEZ"
                                color: "#7EE100"
                                font.pixelSize: 10
                                font.weight: Font.Bold
                                font.letterSpacing: 1.8
                            }
                            Label {
                                text: "LEZ / BTC — Taker Desk"
                                color: "#F7F8FA"
                                font.pixelSize: 30
                                font.weight: Font.Bold
                                font.letterSpacing: -0.7
                            }
                            Label {
                                text: "Choose your wallet, take Maker offers, and authorize only the Taker-owned chain actions."
                                color: "#9FA9B9"
                                font.pixelSize: 13
                            }
                        }

                        ColumnLayout {
                            Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
                            spacing: 8
                            RowLayout {
                                Layout.alignment: Qt.AlignRight; spacing: 9
                                Label {
                                    text: "ACCOUNT"
                                    color: "#6F7A8B"; font.pixelSize: 9
                                    font.weight: Font.Bold; font.letterSpacing: 1.3
                                }
                                Rectangle {
                                    implicitWidth: 9; implicitHeight: 9; radius: 5
                                    color: takerWallet.currentIndex === 0 ? "#7EE100" : "#4FC3F7"
                                }
                                LuxeCombo {
                                    id: takerWallet
                                    objectName: "takerBtcWallet"
                                    model: ["Zurich Wallet 01 · Taker Node"]
                                    implicitWidth: 240
                                    onActivated: root.refreshBtcMarket(false)
                                }
                            }
                            Rectangle {
                                Layout.alignment: Qt.AlignRight
                                implicitWidth: connectionRow.implicitWidth + 22
                                implicitHeight: 32
                                radius: 16
                                color: root.ready ? "#11271F" : "#292318"
                                border.width: 1
                                border.color: root.ready ? "#497621" : "#62438B"
                                RowLayout {
                                    id: connectionRow
                                    anchors.centerIn: parent
                                    spacing: 8
                                    Rectangle { implicitWidth: 7; implicitHeight: 7; radius: 4; color: root.ready ? "#7EE100" : "#8950FA" }
                                    Label {
                                        objectName: "takerConnection"
                                        text: root.ready ? "Backend connected" : "Connecting securely"
                                        color: root.ready ? "#B8F57C" : "#C6AAFF"
                                        font.pixelSize: 11
                                        font.weight: Font.DemiBold
                                    }
                                }
                            }
                            Label {
                                Layout.alignment: Qt.AlignRight
                                text: "Bitcoin Core 31.1 · LEZ v0.2 · no public funds"
                                color: "#6F7A8B"
                                font.pixelSize: 10
                            }
                        }
                    }
                }

                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: 76
                    radius: 14
                    color: root.statusMode === "error" ? "#21151A"
                        : root.statusMode === "success" ? "#102019"
                        : root.statusMode === "working" ? "#1C1628" : "#11131A"
                    border.width: 1
                    border.color: root.statusMode === "error" ? "#61323E"
                        : root.statusMode === "success" ? "#285540"
                        : root.statusMode === "working" ? "#6742A0" : "#2A2E3A"
                    RowLayout {
                        anchors.fill: parent; anchors.margins: 16; spacing: 14
                        Rectangle {
                            implicitWidth: 34; implicitHeight: 34; radius: 17
                            color: root.statusMode === "error" ? "#49232D"
                                : root.statusMode === "success" ? "#1C4A37"
                                : root.statusMode === "working" ? "#392358" : "#202433"
                            Label {
                                anchors.centerIn: parent
                                text: root.statusMode === "error" ? "!" : root.statusMode === "success" ? "✓" : root.statusMode === "working" ? "···" : "i"
                                color: root.statusMode === "error" ? "#FF9FAF"
                                    : root.statusMode === "success" ? "#7EE100"
                                    : root.statusMode === "working" ? "#B997FF" : "#A8B5C7"
                                font.pixelSize: 14; font.weight: Font.Bold
                            }
                        }
                        ColumnLayout {
                            Layout.fillWidth: true; spacing: 2
                            Label { text: root.statusTitle; color: "#F2F4F7"; font.pixelSize: 13; font.weight: Font.DemiBold }
                            Label { text: root.statusDetail; color: "#929DAD"; font.pixelSize: 11; elide: Text.ElideRight; Layout.fillWidth: true }
                        }
                        LuxeButton {
                            text: "Monitor"
                            visible: false
                            enabled: root.ready && !root.busy
                            implicitHeight: 38
                            primary: true
                            onClicked: root.monitor()
                        }
                        LuxeButton {
                            objectName: "takerHealth"
                            text: "Check Node"
                            quiet: true
                            enabled: root.ready && !root.busy
                            implicitHeight: 38
                            onClicked: root.health()
                        }
                        LuxeButton {
                            text: "Refresh wallet market"
                            enabled: root.ready && !root.busy && !root.btcMarketBusy
                            implicitHeight: 38
                            onClicked: root.refreshBtcMarket(false)
                        }
                        Label {
                            visible: pair.currentText !== "Bitcoin" && root.selectedOffer !== ""
                            text: root.availableOffers + (root.availableOffers === 1 ? " VERIFIED OFFER" : " VERIFIED OFFERS")
                            color: "#7EE100"; font.pixelSize: 10; font.weight: Font.Bold; font.letterSpacing: 0.8
                        }
                    }
                }

                Rectangle {
                    id: takerMarketPanel
                    objectName: "takerBtcMarket"
                    Layout.fillWidth: true
                    implicitHeight: takerMarketColumn.implicitHeight + 44
                    radius: 16
                    color: "#101722"
                    border.width: 1
                    border.color: root.btcMarketReady ? "#38465A" : "#5C3341"

                    ColumnLayout {
                        id: takerMarketColumn
                        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                        anchors.margins: 22
                        spacing: 16

                        RowLayout {
                            Layout.fillWidth: true; spacing: 12
                            Rectangle {
                                Layout.fillWidth: true; implicitHeight: 44; radius: 9
                                color: "#0D141E"; border.width: 1; border.color: "#263144"
                                RowLayout {
                                    anchors.fill: parent; anchors.margins: 12
                                    Label { text: "TRADE"; color: "#7EE100"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 1 }
                                    Label { text: "BTC"; color: "#B997FF"; font.pixelSize: 13; font.weight: Font.DemiBold }
                                    Label { text: "↔"; color: "#6F7A8B"; font.pixelSize: 12; font.weight: Font.Bold }
                                    Label { text: "LEZ"; color: "#7EE100"; font.pixelSize: 13; font.weight: Font.DemiBold }
                                    Item { Layout.fillWidth: true }
                                    Label { text: "REGTEST / PRIVATE LOCAL"; color: "#657184"; font.pixelSize: 8; font.weight: Font.Bold; font.letterSpacing: 0.8 }
                                }
                            }
                            LuxeButton {
                                objectName: "takerMarketRefresh"
                                text: "Refresh market"; quiet: true
                                enabled: root.ready && !root.btcMarketBusy
                                onClicked: root.refreshBtcMarket(false)
                            }
                        }

                        RowLayout {
                            Layout.fillWidth: true; spacing: 12
                            StepBadge { number: "01"; accent: "#8950FA" }
                            ColumnLayout {
                                Layout.fillWidth: true; spacing: 2
                                Label { text: "My orders"; color: "#F5F6F8"; font.pixelSize: 17; font.weight: Font.DemiBold }
                                Label { text: "Offers this wallet has taken. Direction-specific Taker actions appear here; Maker actions appear on the other dashboard."; color: "#7F8A9B"; font.pixelSize: 11 }
                            }
                        }

                        RowLayout {
                            spacing: 7
                            FilterTab {
                                objectName: "takerSwapTabAttention"
                                label: "NEEDS YOU"; count: root.swapCountFor("attention")
                                active: root.swapTab === "attention"; onPicked: root.swapTab = "attention"
                            }
                            FilterTab {
                                label: "RUNNING"; count: root.swapCountFor("running")
                                active: root.swapTab === "running"; onPicked: root.swapTab = "running"
                            }
                            FilterTab {
                                label: "DONE"; count: root.swapCountFor("done")
                                active: root.swapTab === "done"; onPicked: root.swapTab = "done"
                            }
                            FilterTab {
                                label: "ALL"; count: (root.btcMarket.swaps ?? []).length
                                active: root.swapTab === "all"; onPicked: root.swapTab = "all"
                            }
                        }

                        Rectangle {
                            id: takerAttentionBanner
                            property var target: root.otherWalletNeedingAction()
                            visible: takerAttentionBanner.target !== null
                            Layout.fillWidth: true
                            implicitHeight: visible ? 42 : 0
                            radius: 10
                            color: takerAttentionArea.containsMouse ? "#2A1930" : "#1F1526"
                            border.width: 1; border.color: "#FA50C1"
                            RowLayout {
                                anchors.fill: parent; anchors.margins: 12; spacing: 10
                                Label {
                                    text: "ANOTHER ACCOUNT"
                                    color: "#FA50C1"; font.pixelSize: 8
                                    font.weight: Font.Bold; font.letterSpacing: 0.9
                                }
                                Label {
                                    Layout.fillWidth: true
                                    text: takerAttentionBanner.target
                                        ? takerAttentionBanner.target.label + " has "
                                          + takerAttentionBanner.target.count
                                          + (Number(takerAttentionBanner.target.count) === 1
                                             ? " action waiting" : " actions waiting")
                                        : ""
                                    color: "#F1F3F6"; font.pixelSize: 12; font.weight: Font.DemiBold
                                    elide: Text.ElideRight
                                }
                                Label {
                                    text: "SWITCH ACCOUNT →"
                                    color: "#FFB8EC"; font.pixelSize: 9
                                    font.weight: Font.Bold; font.letterSpacing: 0.8
                                }
                            }
                            MouseArea {
                                id: takerAttentionArea
                                anchors.fill: parent
                                hoverEnabled: true
                                cursorShape: Qt.PointingHandCursor
                                onClicked: {
                                    if (!takerAttentionBanner.target) return
                                    takerWallet.currentIndex = takerAttentionBanner.target.index
                                    root.refreshBtcMarket(false)
                                }
                            }
                        }

                        Label {
                            visible: root.filteredSwaps().length === 0
                            text: (root.btcMarket.swaps ?? []).length === 0
                                ? "This wallet has not taken an offer yet."
                                : root.swapTab === "attention" ? "Nothing needs you right now — check RUNNING."
                                : "Nothing under this tab yet."
                            color: "#7F8A9B"; font.pixelSize: 12
                        }

                        Repeater {
                            model: root.filteredSwaps()
                            delegate: Rectangle {
                                id: takerSwapRow
                                required property var modelData
                                readonly property var effects: takerSwapRow.modelData.effects ?? []
                                readonly property bool hashesShown:
                                    root.expandedSwaps[takerSwapRow.modelData.ui_swap_id] === true
                                        && takerSwapRow.effects.length > 0
                                Layout.fillWidth: true; radius: 10
                                implicitHeight: (takerSwapRow.modelData.progress_detail ? 104 : 86)
                                    + (takerSwapRow.hashesShown ? takerSwapRow.effects.length * 21 + 30 : 0)
                                color: takerSwapRow.modelData.can_act === true ? "#17152A" : "#0D141E"
                                border.width: 1
                                border.color: takerSwapRow.modelData.can_act === true ? "#8950FA" : "#28364A"
                                RowLayout {
                                    anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                                    anchors.margins: 13; spacing: 14
                                    ColumnLayout {
                                        Layout.fillWidth: true; spacing: 3
                                        Label {
                                            text: String(takerSwapRow.modelData.maker_wallet_label) + " · " + String(takerSwapRow.modelData.state_label)
                                            color: "#F1F3F6"; font.pixelSize: 12; font.weight: Font.DemiBold
                                            elide: Text.ElideRight; Layout.fillWidth: true
                                        }
                                        Label {
                                            text: String(takerSwapRow.modelData.ui_swap_id) + "  /  " + String(takerSwapRow.modelData.offer_id)
                                            color: "#68768A"; font.pixelSize: 9; font.family: "DejaVu Sans Mono"
                                            elide: Text.ElideMiddle; Layout.fillWidth: true
                                        }
                                        RowLayout {
                                            Layout.fillWidth: true; spacing: 8
                                            Rectangle {
                                                Layout.fillWidth: true; implicitHeight: 4; radius: 2; color: "#252E3C"
                                                Rectangle {
                                                    id: takerSwapFill
                                                    property bool settled: false
                                                    width: parent.width * Number(takerSwapRow.modelData.progress_percent ?? 0) / 100
                                                    height: parent.height; radius: 2
                                                    color: takerSwapRow.modelData.state === "completed" ? "#7EE100" : "#8950FA"
                                                    Timer { interval: 400; running: true; onTriggered: takerSwapFill.settled = true }
                                                    Behavior on width {
                                                        enabled: takerSwapFill.settled
                                                        NumberAnimation { duration: 600; easing.type: Easing.OutCubic }
                                                    }
                                                }
                                            }
                                            Label {
                                                text: String(takerSwapRow.modelData.progress_percent ?? 0) + "%"
                                                color: "#9AA6B8"; font.pixelSize: 9; font.family: "DejaVu Sans Mono"
                                            }
                                        }
                                        Label {
                                            visible: !!takerSwapRow.modelData.progress_detail
                                            text: String(takerSwapRow.modelData.progress_detail ?? "")
                                                + (takerSwapRow.modelData.eta_display ? "  ·  " + String(takerSwapRow.modelData.eta_display) : "")
                                            color: "#8E7BC6"; font.pixelSize: 10
                                            elide: Text.ElideRight; Layout.fillWidth: true
                                        }
                                        ColumnLayout {
                                            visible: takerSwapRow.hashesShown
                                            Layout.fillWidth: true; Layout.topMargin: 4; spacing: 4
                                            Repeater {
                                                model: takerSwapRow.hashesShown ? takerSwapRow.effects : []
                                                delegate: RowLayout {
                                                    id: takerFxRow
                                                    required property var modelData
                                                    property bool copied: false
                                                    Layout.fillWidth: true; spacing: 8
                                                    Label {
                                                        text: String(takerFxRow.modelData.chain ?? "").toUpperCase()
                                                        color: takerFxRow.modelData.chain === "Bitcoin" ? "#B997FF" : "#7EE100"
                                                        font.pixelSize: 8; font.weight: Font.Bold; font.letterSpacing: 0.6
                                                        Layout.preferredWidth: 52
                                                    }
                                                    Label {
                                                        text: String(takerFxRow.modelData.label ?? "")
                                                        color: "#8E99AA"; font.pixelSize: 9
                                                        Layout.preferredWidth: 148; elide: Text.ElideRight
                                                    }
                                                    Label {
                                                        text: String(takerFxRow.modelData.transaction_id ?? "")
                                                        color: "#C7CED9"; font.pixelSize: 9; font.family: "DejaVu Sans Mono"
                                                        elide: Text.ElideMiddle; Layout.fillWidth: true
                                                    }
                                                    Label {
                                                        text: takerFxRow.copied ? "COPIED ✓" : "COPY"
                                                        color: takerFxRow.copied ? "#7EE100" : "#B997FF"
                                                        font.pixelSize: 8; font.weight: Font.Bold; font.letterSpacing: 0.8
                                                        MouseArea {
                                                            anchors.fill: parent; anchors.margins: -4
                                                            cursorShape: Qt.PointingHandCursor
                                                            onClicked: {
                                                                root.copyText(takerFxRow.modelData.transaction_id)
                                                                takerFxRow.copied = true
                                                                takerFxCopyReset.restart()
                                                            }
                                                        }
                                                    }
                                                    Timer { id: takerFxCopyReset; interval: 1600; onTriggered: takerFxRow.copied = false }
                                                }
                                            }
                                            Label {
                                                text: "Verify any hash — paste it into the proof explorer search at 127.0.0.1:3003"
                                                color: "#5F6B7D"; font.pixelSize: 8; font.letterSpacing: 0.4
                                            }
                                        }
                                    }
                                    Label {
                                        visible: takerSwapRow.modelData.can_act !== true
                                        text: takerSwapRow.modelData.action_role === "maker" ? "WAITING FOR MAKER" : String(takerSwapRow.modelData.state).toUpperCase()
                                        color: "#7B8798"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.7
                                    }
                                    LuxeButton {
                                        objectName: "takerSwapHashes"
                                        visible: takerSwapRow.effects.length > 0
                                        text: takerSwapRow.hashesShown ? "Hide hashes" : "Tx hashes"
                                        quiet: true
                                        onClicked: root.toggleSwapHashes(takerSwapRow.modelData.ui_swap_id)
                                    }
                                    LuxeButton {
                                        objectName: "takerSwapAction"
                                        visible: takerSwapRow.modelData.can_act === true
                                        text: String(takerSwapRow.modelData.action_label ?? "Continue")
                                        primary: true
                                        enabled: root.ready && !root.btcMarketBusy
                                        onClicked: root.runTakerAction(takerSwapRow.modelData)
                                    }
                                }
                            }
                        }

                        RowLayout {
                            Layout.fillWidth: true; spacing: 12; Layout.topMargin: 5
                            StepBadge { number: "02"; accent: "#FA50C1" }
                            ColumnLayout {
                                Layout.fillWidth: true; spacing: 2
                                Label { text: "Available orders"; color: "#F5F6F8"; font.pixelSize: 17; font.weight: Font.DemiBold }
                                Label {
                                    text: Number((root.btcMarket.summary ?? {}).pending_offers ?? 0) + " open across both Maker wallets"
                                    color: "#7F8A9B"; font.pixelSize: 11
                                }
                            }
                            Rectangle {
                                implicitWidth: takerRunnerLabel.implicitWidth + 24; implicitHeight: 31; radius: 2
                                color: root.btcMarket.runner_ready === true ? "#142A20" : "#2A1820"
                                border.width: 1
                                border.color: root.btcMarket.runner_ready === true ? "#416F4F" : "#724051"
                                Label {
                                    id: takerRunnerLabel
                                    anchors.centerIn: parent
                                    text: root.btcMarket.runner_busy === true ? "NODES ACTIVE"
                                        : root.btcMarket.runner_ready === true ? "NODES READY" : "NODES OFFLINE"
                                    color: root.btcMarket.runner_ready === true ? "#7EE100" : "#FF9FAF"
                                    font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.9
                                }
                            }
                        }

                        Label {
                            visible: (root.btcMarket.order_book ?? []).length === 0
                            text: root.btcMarketReady ? "No pending offers. Ask a Maker wallet to publish inventory." : "Loading wallet-indexed offers…"
                            color: "#7F8A9B"; font.pixelSize: 12
                        }

                        Repeater {
                            model: root.btcMarket.order_book ?? []
                            delegate: Rectangle {
                                id: takerOfferRow
                                required property var modelData
                                Layout.fillWidth: true; implicitHeight: 70; radius: 10
                                color: "#0D141E"; border.width: 1; border.color: "#28364A"
                                RowLayout {
                                    anchors.fill: parent; anchors.margins: 13; spacing: 14
                                    Rectangle {
                                        implicitWidth: 38; implicitHeight: 38; radius: 2
                                        color: "#201830"; border.width: 1; border.color: "#8950FA"
                                        Label { anchors.centerIn: parent; text: "M"; color: "#B997FF"; font.pixelSize: 13; font.weight: Font.Bold }
                                    }
                                    ColumnLayout {
                                        Layout.fillWidth: true; spacing: 2
                                        Label { text: String(takerOfferRow.modelData.maker_wallet_label); color: "#F1F3F6"; font.pixelSize: 12; font.weight: Font.DemiBold }
                                        Label {
                                            text: String(takerOfferRow.modelData.offer_id)
                                            color: "#6F7B8E"; font.pixelSize: 9; font.family: "DejaVu Sans Mono"
                                            elide: Text.ElideMiddle; Layout.fillWidth: true
                                        }
                                    }
                                    Label { text: String(takerOfferRow.modelData.taker_pays_display ?? "0.01000000 BTC"); color: "#B997FF"; font.pixelSize: 12; font.weight: Font.Bold }
                                    Label { text: "→"; color: "#687486"; font.pixelSize: 15 }
                                    Label { text: String(takerOfferRow.modelData.taker_receives_display ?? "1,000 LEZ"); color: "#7EE100"; font.pixelSize: 12; font.weight: Font.Bold }
                                    LuxeButton {
                                        objectName: "takerTakeOffer"
                                        text: "Take offer"; primary: true
                                        enabled: root.ready && !root.btcMarketBusy
                                        onClicked: root.takeBtcOffer(takerOfferRow.modelData)
                                    }
                                }
                            }
                        }
                    }
                }


                Rectangle {
                    id: btcEvidencePanel
                    objectName: "takerBtcEvidence"
                    Layout.fillWidth: true
                    implicitHeight: btcEvidenceColumn.implicitHeight + 44
                    radius: 16
                    color: "#101722"
                    border.width: 1
                    border.color: root.btcEvidenceReady ? "#4A6940" : "#34304A"

                    ColumnLayout {
                        id: btcEvidenceColumn
                        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                        anchors.margins: 22
                        spacing: 16

                        RowLayout {
                            Layout.fillWidth: true; spacing: 12
                            StepBadge { number: "03"; accent: "#8950FA" }
                            ColumnLayout {
                                Layout.fillWidth: true; spacing: 2
                                Label {
                                    text: "Five effects. Two chains. One completed swap."
                                    color: "#F5F6F8"; font.pixelSize: 19; font.weight: Font.DemiBold
                                }
                                Label {
                                    text: root.btcEvidenceReady
                                        ? "Run " + root.btcEvidence.run_id + " · " + root.btcEvidence.completed_at
                                        : "Loading the public, secret-free certification snapshot"
                                    color: "#7F8A9B"; font.pixelSize: 11
                                }
                            }
                            Rectangle {
                                implicitWidth: completedLabel.implicitWidth + 24; implicitHeight: 32; radius: 16
                                color: "#142A20"; border.width: 1; border.color: "#416F4F"
                                Label {
                                    id: completedLabel; anchors.centerIn: parent
                                    text: root.btcEvidenceReady ? "REV 4 · COMPLETED" : "VERIFYING"
                                    color: "#7EE100"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.8
                                }
                            }
                            LuxeButton {
                                objectName: "takerRefreshProof"
                                text: "Refresh proof"
                                quiet: true
                                enabled: root.ready && !root.busy
                                onClicked: root.loadBtcEvidence()
                            }
                        }

                        GridLayout {
                            Layout.fillWidth: true
                            columns: 4
                            columnSpacing: 10
                            Repeater {
                                model: [
                                    ["PAIR", "LEZ ↔ BTC", "#8950FA"],
                                    ["BITCOIN EFFECTS", root.btcEvidenceReady ? String(root.btcEvidence.effect_counts.bitcoin) : "—", "#8950FA"],
                                    ["LEZ EFFECTS", root.btcEvidenceReady ? String(root.btcEvidence.effect_counts.lez) : "—", "#7EE100"],
                                    ["REPLAY SUBMISSIONS", root.btcEvidenceReady ? String(root.btcEvidence.replay_resubmission_count) : "—", "#FA50C1"]
                                ]
                                delegate: Rectangle {
                                    id: metricCard
                                    required property var modelData
                                    Layout.fillWidth: true; implicitHeight: 64; radius: 9
                                    color: "#0D141E"; border.width: 1; border.color: "#253143"
                                    ColumnLayout {
                                        anchors.fill: parent; anchors.margins: 11; spacing: 2
                                        Label { text: metricCard.modelData[0]; color: "#758195"; font.pixelSize: 8; font.weight: Font.Bold; font.letterSpacing: 0.7 }
                                        Label { text: metricCard.modelData[1]; color: metricCard.modelData[2]; font.pixelSize: 16; font.weight: Font.Bold }
                                    }
                                }
                            }
                        }

                        Rectangle {
                            id: balanceLedger
                            objectName: "takerBalanceLedger"
                            Layout.fillWidth: true
                            implicitHeight: balanceLedgerColumn.implicitHeight + 30
                            radius: 11
                            color: "#0B111A"
                            border.width: 1
                            border.color: root.btcEvidence.wallet_balance_changes ? "#3D671E" : "#283244"

                            ColumnLayout {
                                id: balanceLedgerColumn
                                anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                                anchors.margins: 15
                                spacing: 12
                                RowLayout {
                                    Layout.fillWidth: true
                                    ColumnLayout {
                                        Layout.fillWidth: true; spacing: 2
                                        Label { text: "Wallet balance proof"; color: "#F2F4F7"; font.pixelSize: 14; font.weight: Font.DemiBold }
                                        Label {
                                            text: root.btcEvidence.wallet_balance_changes
                                                ? "Opening → closing balances reconciled from finalized Bitcoin and LEZ state"
                                                : "Balance ledger appears after a completed interactive Maker / Taker run"
                                            color: "#778396"; font.pixelSize: 10
                                        }
                                    }
                                    Label {
                                        visible: root.btcEvidence.wallet_balance_changes !== undefined
                                        text: "PRINCIPAL + FEES RECONCILED"
                                        color: "#7EE100"; font.pixelSize: 8; font.weight: Font.Bold; font.letterSpacing: 0.8
                                    }
                                }
                                GridLayout {
                                    Layout.fillWidth: true
                                    columns: width > 780 ? 2 : 1
                                    columnSpacing: 10; rowSpacing: 10
                                    Repeater {
                                        model: root.btcEvidence.wallet_balance_changes
                                            ? root.btcEvidence.wallet_balance_changes.wallets : []
                                        delegate: Rectangle {
                                            id: walletProof
                                            required property var modelData
                                            Layout.fillWidth: true; implicitHeight: 128; radius: 9
                                            color: "#101722"; border.width: 1
                                            border.color: walletProof.modelData.role === "taker" ? "#5A3D8B" : "#3E4A5D"
                                            ColumnLayout {
                                                anchors.fill: parent; anchors.margins: 12; spacing: 7
                                                RowLayout {
                                                    Layout.fillWidth: true
                                                    Label {
                                                        text: walletProof.modelData.role.toUpperCase() + " · " + walletProof.modelData.wallet_id
                                                        color: walletProof.modelData.role === "taker" ? "#B997FF" : "#D4DBE5"
                                                        font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.6
                                                    }
                                                    Item { Layout.fillWidth: true }
                                                    Label { text: walletProof.modelData.role === "taker" ? "YOU" : "COUNTERPARTY"; color: "#687486"; font.pixelSize: 8; font.weight: Font.Bold }
                                                }
                                                RowLayout {
                                                    Layout.fillWidth: true
                                                    Label { text: "BTC"; color: "#8950FA"; font.pixelSize: 10; font.weight: Font.Bold; Layout.preferredWidth: 38 }
                                                    Label { text: root.formatBtcSats(walletProof.modelData.balances.bitcoin.opening); color: "#9CA7B7"; font.pixelSize: 10 }
                                                    Label { text: "→"; color: "#5F6B7D"; font.pixelSize: 11 }
                                                    Label { text: root.formatBtcSats(walletProof.modelData.balances.bitcoin.closing); color: "#F0F3F7"; font.pixelSize: 10; font.weight: Font.DemiBold }
                                                    Item { Layout.fillWidth: true }
                                                    Label { text: root.formatSignedBtc(walletProof.modelData.balances.bitcoin.net_change); color: Number(walletProof.modelData.balances.bitcoin.net_change) >= 0 ? "#7EE100" : "#FA50C1"; font.pixelSize: 10; font.weight: Font.Bold }
                                                }
                                                RowLayout {
                                                    Layout.fillWidth: true
                                                    Label { text: "LEZ"; color: "#7EE100"; font.pixelSize: 10; font.weight: Font.Bold; Layout.preferredWidth: 38 }
                                                    Label { text: root.formatLez(walletProof.modelData.balances.lez.opening); color: "#9CA7B7"; font.pixelSize: 10 }
                                                    Label { text: "→"; color: "#5F6B7D"; font.pixelSize: 11 }
                                                    Label { text: root.formatLez(walletProof.modelData.balances.lez.closing); color: "#F0F3F7"; font.pixelSize: 10; font.weight: Font.DemiBold }
                                                    Item { Layout.fillWidth: true }
                                                    Label { text: root.formatSignedLez(walletProof.modelData.balances.lez.net_change); color: Number(walletProof.modelData.balances.lez.net_change) >= 0 ? "#7EE100" : "#FA50C1"; font.pixelSize: 10; font.weight: Font.Bold }
                                                }
                                                Label {
                                                    text: "BTC fee " + root.formatBtcSats(walletProof.modelData.balances.bitcoin.fee)
                                                    color: "#667386"; font.pixelSize: 8
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        GridLayout {
                            id: evidenceGrid
                            Layout.fillWidth: true
                            columns: width > 1240 ? 5 : width > 760 ? 3 : 1
                            columnSpacing: 10
                            rowSpacing: 10
                            Repeater {
                                model: root.btcEvidenceReady ? root.btcEvidence.effects : []
                                delegate: EvidenceCard {
                                    id: effectCard
                                    required property var modelData
                                    effect: effectCard.modelData
                                }
                            }
                        }

                        Rectangle {
                            Layout.fillWidth: true; implicitHeight: 54; radius: 9
                            color: "#151721"; border.width: 1; border.color: "#353446"
                            RowLayout {
                                anchors.fill: parent; anchors.margins: 13; spacing: 12
                                Label { text: "EVIDENCE"; color: "#FA50C1"; font.pixelSize: 8; font.weight: Font.Bold; font.letterSpacing: 1 }
                                Label {
                                    Layout.fillWidth: true
                                    text: "These are public identities from a completed isolated local run—not hashes invented by the UI. Open any proof at localhost:3003."
                                    color: "#A4ADBA"; font.pixelSize: 10; wrapMode: Text.WordWrap
                                }
                                LuxeButton { text: "Open evidence explorer"; quiet: true; onClicked: Qt.openUrlExternally("http://127.0.0.1:3003/#/evidence") }
                            }
                        }
                    }
                }

                GridLayout {
                    // Authenticated cross-pair offer discovery remains available
                    // below the primary Bitcoin desk for the complete Basecamp PoC.
                    Layout.fillWidth: true
                    columns: width > 1040 ? 2 : 1
                    columnSpacing: 16
                    rowSpacing: 16

                    Rectangle {
                        Layout.fillWidth: true
                        Layout.alignment: Qt.AlignTop
                        implicitHeight: discoverColumn.implicitHeight + 44
                        radius: 16
                        color: "#101722"
                        border.width: 1
                        border.color: "#263144"
                        ColumnLayout {
                            id: discoverColumn
                            anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                            anchors.margins: 22
                            spacing: 16
                            RowLayout {
                                Layout.fillWidth: true; spacing: 12
                                StepBadge { number: "03"; accent: "#FA50C1" }
                                ColumnLayout {
                                    Layout.fillWidth: true; spacing: 2
                                    Label { text: "Explore another corridor"; color: "#F5F6F8"; font.pixelSize: 17; font.weight: Font.DemiBold }
                                    Label { text: "This release exposes only the completed Bitcoin M3 corridor"; color: "#7F8A9B"; font.pixelSize: 11 }
                                }
                            }
                            GridLayout {
                                Layout.fillWidth: true; columns: 2; columnSpacing: 10; rowSpacing: 7
                                FieldLabel { text: "ASSET YOU RECEIVE" }
                                FieldLabel { text: "YOUR SIDE" }
                                LuxeCombo { id: pair; objectName: "takerPair"; model: ["Bitcoin"]; Layout.fillWidth: true }
                                LuxeCombo { id: direction; objectName: "takerDirection"; model: ["TakerSellsForeign", "TakerSellsLez"]; Layout.fillWidth: true }
                            }
                            LuxeButton {
                                objectName: "takerOffers"
                                text: pair.currentText === "Bitcoin" ? "Refresh completed BTC evidence" : "Browse authenticated offers"
                                primary: true
                                enabled: root.ready && !root.busy
                                Layout.fillWidth: true
                                onClicked: pair.currentText === "Bitcoin" ? root.loadBtcEvidence() : root.browse()
                            }
                            LuxeButton {
                                text: "Check prepared Node"
                                quiet: true
                                enabled: root.ready && !root.busy
                                Layout.fillWidth: true
                                onClicked: root.health()
                            }
                            Rectangle {
                                Layout.fillWidth: true
                                implicitHeight: 82
                                radius: 11
                                color: pair.currentText === "Bitcoin" || root.selectedOffer !== "" ? "#151C22" : "#0D131C"
                                border.width: 1
                                border.color: pair.currentText === "Bitcoin" || root.selectedOffer !== "" ? "#3C493D" : "#202A39"
                                RowLayout {
                                    anchors.fill: parent; anchors.margins: 14; spacing: 12
                                    Rectangle {
                                        implicitWidth: 38; implicitHeight: 38; radius: 10
                                        color: pair.currentText === "Bitcoin" || root.selectedOffer !== "" ? "#223528" : "#192231"
                                            Label { anchors.centerIn: parent; text: pair.currentText === "Bitcoin" || root.selectedOffer !== "" ? "✓" : "—"; color: "#7EE100"; font.pixelSize: 16; font.weight: Font.Bold }
                                    }
                                    ColumnLayout {
                                        Layout.fillWidth: true; spacing: 3
                                        Label {
                                            text: pair.currentText === "Bitcoin" ? "Completed M3 evidence ready"
                                                : root.selectedOffer === "" ? "No offer selected" : "Authenticated offer ready"
                                            color: "#EDEFF3"; font.pixelSize: 12; font.weight: Font.DemiBold
                                        }
                                        Label {
                                            text: pair.currentText === "Bitcoin"
                                                ? (root.btcEvidenceReady ? root.btcEvidence.run_id : "Loading certified run")
                                                : root.selectedOffer === "" ? "Browse to select the newest valid quote" : root.selectedOffer
                                            color: "#8C97A8"; font.pixelSize: 10; font.family: "DejaVu Sans Mono"; elide: Text.ElideMiddle; Layout.fillWidth: true
                                        }
                                    }
                                    ColumnLayout {
                                        visible: pair.currentText === "Bitcoin" || root.selectedOffer !== ""; spacing: 2
                                        Label { text: pair.currentText === "Bitcoin" ? "STATE" : "VALID UNTIL"; color: "#687486"; font.pixelSize: 9; font.weight: Font.Bold }
                                        Label { text: pair.currentText === "Bitcoin" ? "COMPLETED" : root.selectedExpiry; color: "#FA50C1"; font.pixelSize: 12; font.weight: Font.DemiBold }
                                    }
                                }
                            }
                        }
                    }

                    Rectangle {
                        id: reviewPanel
                        objectName: "takerReview"
                        visible: pair.currentText !== "Bitcoin"
                        Layout.fillWidth: true
                        Layout.alignment: Qt.AlignTop
                        implicitHeight: reviewColumn.implicitHeight + 44
                        radius: 16
                        color: "#101722"
                        border.width: 1
                        border.color: root.selectedOffer === "" ? "#263144" : "#704063"
                        ColumnLayout {
                            id: reviewColumn
                            anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                            anchors.margins: 22
                            spacing: 14
                            RowLayout {
                                Layout.fillWidth: true; spacing: 12
                                StepBadge { number: "02"; accent: "#FA50C1" }
                                ColumnLayout {
                                    Layout.fillWidth: true; spacing: 2
                                    Label { text: "Review exact terms"; color: "#F5F6F8"; font.pixelSize: 17; font.weight: Font.DemiBold }
                                    Label { text: "Cryptographic facts are filled from the selected envelope"; color: "#7F8A9B"; font.pixelSize: 11 }
                                }
                                Label {
                                    visible: root.selectedOffer !== ""
                                    text: "SIGNATURE VERIFIED"
                                    color: "#7EE100"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.8
                                }
                            }
                            FieldLabel { text: "OFFER ID" }
                            LuxeField {
                                id: offerId; objectName: "takerOfferId"
                                placeholderText: "Selected automatically after browsing"
                                Layout.fillWidth: true
                                font.family: "DejaVu Sans Mono"
                            }
                            FieldLabel { text: "MAKER PUBLIC IDENTITY" }
                            LuxeField {
                                id: makerIdentity; objectName: "takerMakerIdentity"
                                placeholderText: "Verified compressed public key"
                                Layout.fillWidth: true
                                font.family: "DejaVu Sans Mono"
                            }
                            FieldLabel { text: "SIGNED-ENVELOPE SHA-256" }
                            LuxeField {
                                id: envelopeDigest; objectName: "takerEnvelopeDigest"
                                placeholderText: "Verified envelope fingerprint"
                                Layout.fillWidth: true
                                font.family: "DejaVu Sans Mono"
                            }
                            GridLayout {
                                Layout.fillWidth: true; columns: 2; columnSpacing: 10; rowSpacing: 7
                                FieldLabel { text: direction.currentIndex === 0 ? "YOU RECEIVE · FOREIGN UNITS" : "YOU SEND · FOREIGN UNITS" }
                                FieldLabel { text: direction.currentIndex === 0 ? "YOU SEND · LEZ UNITS" : "YOU RECEIVE · LEZ UNITS" }
                                LuxeField {
                                    id: foreignUnits; objectName: "takerForeignUnits"
                                    placeholderText: "Exact foreign units"; text: "10000"; Layout.fillWidth: true
                                    font.pixelSize: 16; font.weight: Font.DemiBold
                                }
                                LuxeField {
                                    id: lezUnits; objectName: "takerLezUnits"
                                    placeholderText: "Exact LEZ units"; text: "25000"; Layout.fillWidth: true
                                    font.pixelSize: 16; font.weight: Font.DemiBold
                                }
                            }
                            LuxeButton {
                                objectName: "takerInitiate"
                                text: "Confirm and initiate"
                                primary: true
                                enabled: root.ready && !root.busy && offerId.text.length > 0
                                    && makerIdentity.text.length > 0 && envelopeDigest.text.length === 64
                                Layout.fillWidth: true
                                onClicked: root.initiate()
                            }
                            Label {
                                Layout.fillWidth: true
                                text: "By confirming, you bind these exact terms to a countersigned, replay-safe agreement."
                                color: "#697587"; font.pixelSize: 10; wrapMode: Text.WordWrap
                            }
                        }
                    }
                }

                Rectangle {
                    id: progressPanel
                    objectName: "takerProgress"
                    visible: pair.currentText !== "Bitcoin"
                    Layout.fillWidth: true
                    implicitHeight: progressColumn.implicitHeight + 44
                    radius: 16
                    color: "#101722"
                    border.width: 1
                    border.color: "#263144"
                    ColumnLayout {
                        id: progressColumn
                        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                        anchors.margins: 22
                        spacing: 14
                        RowLayout {
                            Layout.fillWidth: true; spacing: 12
                            StepBadge { number: "03"; accent: "#7EE100" }
                            ColumnLayout {
                                Layout.fillWidth: true; spacing: 2
                                Label { text: "Track settlement"; color: "#F5F6F8"; font.pixelSize: 17; font.weight: Font.DemiBold }
                                Label { text: "Monitor actor progress and use terminal actions only when advertised"; color: "#7F8A9B"; font.pixelSize: 11 }
                            }
                            Rectangle {
                                visible: root.currentState !== ""
                                implicitWidth: stateLabel.implicitWidth + 22; implicitHeight: 30; radius: 15
                                color: "#241A36"; border.width: 1; border.color: "#6944A2"
                                Label {
                                    id: stateLabel; anchors.centerIn: parent
                                    text: root.currentState.split("_").join(" ").toUpperCase()
                                    color: "#C5A9FF"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.7
                                }
                            }
                        }
                        GridLayout {
                            Layout.fillWidth: true; columns: 5; columnSpacing: 10; rowSpacing: 7
                            FieldLabel { text: "SWAP ID"; Layout.columnSpan: 3 }
                            FieldLabel { text: "GENERATION"; Layout.columnSpan: 2 }
                            LuxeField {
                                id: swapId; objectName: "takerSwapId"; placeholderText: "Swap ID"
                                text: root.currentSwap; Layout.fillWidth: true; Layout.columnSpan: 3
                                font.family: "DejaVu Sans Mono"
                            }
                            LuxeField {
                                id: generation; placeholderText: "Generation"; text: "0"
                                Layout.fillWidth: true; Layout.columnSpan: 2
                            }
                        }
                        RowLayout {
                            Layout.fillWidth: true; spacing: 10
                            LuxeButton {
                                objectName: "takerMonitor"; text: "Refresh state"
                                primary: true; enabled: root.ready && !root.busy && swapId.text.length > 0
                                Layout.preferredWidth: 160; onClicked: root.monitor()
                            }
                            LuxeButton {
                                objectName: "takerListSwaps"; text: "Refresh registry"
                                enabled: root.ready && !root.busy; onClicked: root.listSwaps()
                            }
                            Item { Layout.fillWidth: true }
                            LuxeButton {
                                objectName: "takerClaim"; text: "Claim"
                                enabled: root.ready && !root.busy && swapId.text.length > 0
                                onClicked: root.terminal("claim")
                            }
                            LuxeButton {
                                objectName: "takerRefund"; text: "Refund"; destructive: true
                                enabled: root.ready && !root.busy && swapId.text.length > 0
                                onClicked: root.terminal("refund")
                            }
                        }
                        Rectangle {
                            Layout.fillWidth: true; implicitHeight: 54; radius: 10
                            color: "#171A22"; border.width: 1; border.color: "#35323A"
                            RowLayout {
                                anchors.fill: parent; anchors.margins: 13; spacing: 11
                                Label { text: "PRIVACY"; color: "#FA50C1"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 1 }
                                Label {
                                    objectName: "takerShielding"
                                    text: "Bitcoin amounts and transaction linkage are public; use fresh wallet addresses."
                                    color: "#A8AFBB"; font.pixelSize: 11; Layout.fillWidth: true; wrapMode: Text.WordWrap
                                }
                            }
                        }
                    }
                }

                Rectangle {
                    objectName: "takerChat"
                    Layout.fillWidth: true
                    implicitHeight: takerChatColumn.implicitHeight + 40
                    radius: 14
                    color: "#101722"
                    border.width: 1
                    border.color: "#3A3152"
                    ColumnLayout {
                        id: takerChatColumn
                        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                        anchors.margins: 20
                        spacing: 10
                        RowLayout {
                            Layout.fillWidth: true
                            ColumnLayout {
                                Layout.fillWidth: true; spacing: 2
                                Label { text: "Private negotiation Chat"; color: "#F1F3F6"; font.pixelSize: 14; font.weight: Font.DemiBold }
                                Label { text: "Paste the Maker address shown by the currently open Maker app"; color: "#7F8A9B"; font.pixelSize: 10 }
                            }
                            Label { text: root.chatState.toUpperCase(); color: "#B997FF"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.8 }
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
                                text: "Connect Chat"
                                primary: true
                                enabled: root.ready && !root.busy && takerChatAddress.text.trim().length > 0
                                onClicked: root.connectChat()
                            }
                            LuxeButton {
                                objectName: "takerChatStatus"
                                text: "Status"
                                enabled: root.ready && !root.busy
                                onClicked: root.chatStatus()
                            }
                            LuxeButton {
                                objectName: "takerChatReset"
                                text: "Reset"
                                enabled: root.ready && !root.busy
                                onClicked: root.resetChat()
                            }
                        }
                        Label { text: "Session identity and conversation history are intentionally discarded when the app closes."; color: "#657184"; font.pixelSize: 10 }
                    }
                }

                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: technicalColumn.implicitHeight + 32
                    radius: 14
                    color: "#0E141D"
                    border.width: 1
                    border.color: "#222C3B"
                    ColumnLayout {
                        id: technicalColumn
                        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                        anchors.margins: 16
                        spacing: 10
                        RowLayout {
                            Layout.fillWidth: true
                            ColumnLayout {
                                Layout.fillWidth: true; spacing: 2
                                Label { text: "Technical evidence"; color: "#C7CED9"; font.pixelSize: 12; font.weight: Font.DemiBold }
                                Label { text: "Raw owner-Node response for audit and debugging"; color: "#657184"; font.pixelSize: 10 }
                            }
                            LuxeButton {
                                text: root.technicalVisible ? "Hide technical details" : "Show technical details"
                                quiet: true
                                onClicked: root.technicalVisible = !root.technicalVisible
                            }
                        }
                        TextArea {
                            objectName: "takerOutput"
                            text: root.output
                            visible: root.technicalVisible
                            readOnly: true
                            wrapMode: Text.WrapAnywhere
                            selectByMouse: true
                            Layout.fillWidth: true
                            Layout.preferredHeight: root.technicalVisible ? 170 : 0
                            color: "#BAC4D3"
                            selectionColor: "#8950FA"
                            selectedTextColor: "#FFFFFF"
                            font.family: "DejaVu Sans Mono"
                            font.pixelSize: 10
                            leftPadding: 12; rightPadding: 12; topPadding: 10; bottomPadding: 10
                            background: Rectangle { color: "#080C12"; radius: 9; border.width: 1; border.color: "#253043" }
                        }
                    }
                }

                Item { Layout.fillWidth: true; implicitHeight: 8 }
            }
        }
    }
}
