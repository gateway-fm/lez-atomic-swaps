pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {
    id: root

    readonly property var backend: logos.module("lez_atomic_swap_maker")
    property bool ready: false
    property bool busy: false
    property bool technicalVisible: false
    property string output: "No operation submitted"
    property string statusMode: "neutral"
    property string statusTitle: "Connecting securely"
    property string statusDetail: "Establishing the owner-local daemon channel"
    property int routeCount: 0
    property int swapCount: 0
    property string currentState: ""
    property string latestSwap: ""
    property string lastSavedRoute: "No route changes in this session"
    property string chatAddress: ""
    property string chatState: "not initialised"
    // The composer side the Maker is selling: LEZ (forward route, the Taker
    // sells Bitcoin) or Bitcoin (reverse route, the Taker sells LEZ).
    property string sellSide: "lez"
    readonly property string offerDirection: root.sellSide === "lez"
        ? "taker_sells_foreign" : "taker_sells_lez"
    property var btcMarket: ({
        inventory: [], swaps: [], wallets: [],
        summary: ({pending_offers: 0, accepted_swaps: 0, completed_swaps: 0}),
        runner_ready: false, runner_busy: false,
        runner_detail: "Checking the local M3 runner",
        latest_balance_evidence: null
    })
    property bool btcMarketReady: false
    property bool btcMarketBusy: false
    // Toggleable bucket filters: ACTIVE and OPEN may be on together, DONE
    // is exclusive with both, and an empty selection shows everything.
    property bool showActive: true
    property bool showOpen: true
    property bool showDone: false
    property bool newOfferOpen: false
    property var expandedSwaps: ({})

    function toggleSwapHashes(uiSwapId) {
        var next = {}
        for (var key in root.expandedSwaps) next[key] = root.expandedSwaps[key]
        next[uiSwapId] = !root.expandedSwaps[uiSwapId]
        root.expandedSwaps = next
    }
    function copyText(value) {
        clipboardHelper.text = String(value)
        clipboardHelper.selectAll()
        clipboardHelper.copy()
    }

    // One unified activity list: publishable offers plus every swap this
    // wallet owns. Offers that were taken live on as their swap row, so only
    // pending and withdrawn offers appear as offer rows. Live swaps — whether
    // waiting on this desk or on the counterparty — share the ACTIVE bucket;
    // rows that need this desk carry a badge and float to the top.
    function marketBucket(item) {
        if (item.kind === "offer")
            return item.state === "pending" ? "open" : "done"
        if (item.state === "completed" || item.state === "failed") return "done"
        return "active"
    }
    function marketRows() {
        var rows = []
        var swaps = root.btcMarket.swaps ?? []
        for (var i = 0; i < swaps.length; i++)
            rows.push(Object.assign({kind: "swap"}, swaps[i]))
        var offers = root.btcMarket.inventory ?? []
        for (var j = 0; j < offers.length; j++) {
            if (offers[j].state === "pending" || offers[j].state === "withdrawn")
                rows.push(Object.assign({kind: "offer"}, offers[j]))
        }
        return rows
    }
    function filteredMarketRows() {
        // Bucket order is fixed: ACTIVE above OPEN above DONE — a row
        // belongs to exactly one bucket, and the merged list never
        // interleaves them.
        var rank = { active: 0, open: 1, done: 2 }
        var rows = root.marketRows().filter(function(item) {
            var bucket = root.marketBucket(item)
            if (bucket === "active") return root.showActive
            if (bucket === "open") return root.showOpen
            return root.showDone
        })
        rows.sort(function(a, b) {
            var ra = rank[root.marketBucket(a)], rb = rank[root.marketBucket(b)]
            if (ra !== rb) return ra - rb
            // Within the list, rows waiting on this desk float to the top.
            var aUrgent = a.can_act === true, bUrgent = b.can_act === true
            if (aUrgent !== bUrgent) return aUrgent ? -1 : 1
            return 0
        })
        return rows
    }
    function attentionCount() {
        return root.marketRows().filter(function(item) { return item.can_act === true }).length
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
    function marketCount(tab) {
        return root.marketRows().filter(function(item) {
            return root.marketBucket(item) === tab
        }).length
    }
    function openOffersBySide(directionName) {
        return (root.btcMarket.inventory ?? []).filter(function(item) {
            return item.state === "pending" && item.direction === directionName
        }).length
    }

    TextEdit { id: clipboardHelper; visible: false }

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
            color: "#111925"
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
            background: Rectangle { color: "#111925"; radius: 10; border.width: 1; border.color: "#344052" }
        }
    }

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
        property bool alert: false
        property bool showCount: true
        signal picked()
        implicitHeight: 28
        implicitWidth: filterTabRow.implicitWidth + 24
        radius: 2
        color: filterTab.active ? "#1D2739" : filterTabArea.containsMouse ? "#151D2A" : "transparent"
        border.width: 1
        border.color: filterTab.active ? "#8950FA" : filterTab.alert ? "#FA50C1" : "#2A3547"
        RowLayout {
            id: filterTabRow
            anchors.centerIn: parent
            spacing: 6
            Label {
                text: filterTab.label
                color: filterTab.active ? "#EDEFF4" : filterTab.alert ? "#E9EDF3" : "#7F8A9B"
                font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 1.1
            }
            Label {
                visible: filterTab.showCount
                text: String(filterTab.count)
                color: filterTab.active ? "#B997FF" : filterTab.alert ? "#FA50C1" : "#5F6B7D"
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

    // DEX-style segmented control for the side the Maker sells. Plain
    // anchored pills — a RowLayout here collapsed its two options onto
    // each other when sized through implicitWidth + centerIn.
    component SideToggle: Rectangle {
        id: sideToggle
        property string value: "lez"
        signal picked(string side)
        width: lezOption.width + btcOption.width + 12
        height: 34
        radius: 9
        color: "#0E1520"
        border.width: 1
        border.color: "#28344A"

        Rectangle {
            id: lezOption
            anchors.left: parent.left; anchors.leftMargin: 4
            anchors.verticalCenter: parent.verticalCenter
            width: lezLabel.implicitWidth + 26; height: 26; radius: 7
            readonly property bool chosen: sideToggle.value === "lez"
            color: chosen ? "#241A3B" : lezArea.containsMouse ? "#182233" : "transparent"
            border.width: chosen ? 1 : 0
            border.color: "#8950FA"
            Label {
                id: lezLabel
                anchors.centerIn: parent
                text: "SELL LEZ"
                color: parent.chosen ? "#C5A9FF" : "#8B96A8"
                font.pixelSize: 10; font.weight: Font.Bold; font.letterSpacing: 0.8
            }
            MouseArea {
                id: lezArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: sideToggle.picked("lez")
            }
        }

        Rectangle {
            id: btcOption
            anchors.left: lezOption.right; anchors.leftMargin: 4
            anchors.verticalCenter: parent.verticalCenter
            width: btcLabel.implicitWidth + 26; height: 26; radius: 7
            readonly property bool chosen: sideToggle.value === "btc"
            color: chosen ? "#33203B" : btcArea.containsMouse ? "#182233" : "transparent"
            border.width: chosen ? 1 : 0
            border.color: "#FA50C1"
            Label {
                id: btcLabel
                anchors.centerIn: parent
                text: "SELL BTC"
                color: parent.chosen ? "#FFB8EC" : "#8B96A8"
                font.pixelSize: 10; font.weight: Font.Bold; font.letterSpacing: 0.8
            }
            MouseArea {
                id: btcArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: sideToggle.picked("btc")
            }
        }
    }

    // One leg of the compose card: asset chip, fixed amount, balance hint.
    // Width is set by the owner (anchored column), never by Layout attached
    // properties — the layout engine ignored fillWidth here and drew this
    // leg wider than its card.
    component SwapLeg: Rectangle {
        id: swapLeg
        property string label: "YOU SELL"
        property string asset: "LEZ"
        property string amount: "1,000"
        property color accent: "#7EE100"
        property string note: ""
        height: 66
        radius: 12
        color: "#0E1520"
        border.width: 1
        border.color: "#26334A"
        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 14
            anchors.rightMargin: 14
            spacing: 12
            Rectangle {
                implicitWidth: 38; implicitHeight: 38; radius: 19
                color: Qt.rgba(swapLeg.accent.r, swapLeg.accent.g, swapLeg.accent.b, 0.14)
                border.width: 1
                border.color: swapLeg.accent
                Label {
                    anchors.centerIn: parent
                    text: swapLeg.asset === "BTC" ? "₿" : "Z"
                    color: swapLeg.accent
                    font.pixelSize: 15; font.weight: Font.Bold
                }
            }
            ColumnLayout {
                Layout.fillWidth: true
                spacing: 1
                Label {
                    text: swapLeg.label
                    color: "#77839A"; font.pixelSize: 9
                    font.weight: Font.Bold; font.letterSpacing: 1.1
                }
                Label {
                    visible: swapLeg.note !== ""
                    text: swapLeg.note
                    color: "#5F6E85"; font.pixelSize: 9
                    elide: Text.ElideMiddle; Layout.fillWidth: true
                }
            }
            Label {
                text: swapLeg.amount
                color: "#F2F5F9"
                font.pixelSize: 21; font.weight: Font.DemiBold
            }
            Label {
                text: swapLeg.asset
                color: swapLeg.accent
                font.pixelSize: 13; font.weight: Font.Bold
                Layout.rightMargin: 2
            }
        }
    }

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

    Connections {
        target: logos
        function onViewModuleReadyChanged(moduleName, isReady) {
            if (moduleName !== "lez_atomic_swap_maker") return
            root.ready = isReady && root.backend !== null
            if (root.ready) {
                root.statusMode = "success"
                root.statusTitle = "Maker daemon connected"
                root.statusDetail = "Loading this wallet's offer inventory and swap actions"
                btcMarketBootstrapTimer.restart()
            }
        }
    }

    Component.onCompleted: {
        root.ready = root.backend !== null && logos.isViewModuleReady("lez_atomic_swap_maker")
        if (root.ready) {
            root.statusMode = "success"
            root.statusTitle = "Maker daemon connected"
            root.statusDetail = "Loading this wallet's offer inventory and swap actions"
            btcMarketBootstrapTimer.restart()
        }
    }

    function decode(raw) {
        var envelope = JSON.parse(String(raw))
        if (envelope.ok !== true)
            throw new Error(envelope.message || envelope.code || "The daemon rejected this request")
        return envelope.result ?? {}
    }

    function run(operation, pendingTitle, onSuccess) {
        if (!root.ready) {
            root.output = "Maker service backend is not ready"
            root.statusMode = "error"
            root.statusTitle = "Daemon unavailable"
            root.statusDetail = "Wait for the owner-local connection and try again"
            return
        }
        root.busy = true
        root.output = "Waiting for owner-local service..."
        root.statusMode = "working"
        root.statusTitle = pendingTitle
        root.statusDetail = "Committing the request over the owner-only channel"
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
                root.statusTitle = "Secure daemon error"
                root.statusDetail = String(error)
                root.technicalVisible = true
            })
    }

    function selectedMakerWallet() {
        return makerWallet.currentIndex === 1 ? "maker-basel-02" : "maker-munich-01"
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

    function applyBtcMarket(result) {
        root.btcMarket = result
        root.btcMarketReady = true
    }

    function refreshBtcMarket(silent) {
        if (!root.ready || root.busy || root.btcMarketBusy) return
        logos.watch(root.backend.btcMarket(root.selectedMakerWallet()),
            function(value) {
                try {
                    if (!silent) root.output = String(value)
                    root.applyBtcMarket(root.decode(value))
                } catch (error) {
                    if (!silent) {
                        root.statusMode = "error"
                        root.statusTitle = "BTC inventory unavailable"
                        root.statusDetail = String(error)
                    }
                }
            },
            function(error) {
                if (!silent) root.output = "Backend failure: " + String(error)
                if (!silent) {
                    root.statusMode = "error"
                    root.statusTitle = "BTC inventory unavailable"
                    root.statusDetail = String(error)
                }
            })
    }

    function createBtcOffers() {
        if (root.btcMarketBusy) return
        root.btcMarketBusy = true
        var segment = root.sellSide === "lez" ? "sell-lez" : "sell-btc"
        var requestId = "ui-maker-" + segment + "-" + String(Date.now())
        root.run(root.backend.btcCreateOffers(requestId, root.selectedMakerWallet(),
            "1", "1000000", "1000", root.offerDirection),
            "Publishing BTC / LEZ inventory", function(result) {
                root.applyBtcMarket(result)
                root.showOpen = true
                root.newOfferOpen = false
                root.statusMode = "success"
                root.statusTitle = "Offer published"
                root.statusDetail = "Indexed to " + makerWallet.currentText + " until taken or withdrawn"
            })
    }

    function withdrawBtcOffer(offer) {
        if (root.btcMarketBusy) return
        root.btcMarketBusy = true
        var requestId = "ui-maker-withdraw-offer-" + String(Date.now())
        root.run(root.backend.btcWithdrawOffer(requestId, root.selectedMakerWallet(),
            String(offer.offer_id)), "Withdrawing pending offer", function(result) {
                root.applyBtcMarket(result)
                root.statusMode = "success"
                root.statusTitle = "Offer withdrawn"
                root.statusDetail = "Only this Maker wallet's inventory was changed"
            })
    }

    function runMakerAction(swap) {
        if (root.btcMarketBusy || swap.can_act !== true) return
        root.btcMarketBusy = true
        var requestId = "ui-maker-swap-action-" + String(Date.now())
        root.run(root.backend.btcSwapAction(requestId, root.selectedMakerWallet(),
            String(swap.ui_swap_id), String(swap.action_required)),
            String(swap.action_label), function(result) {
                root.applyBtcMarket(result)
                root.statusMode = "working"
                root.statusTitle = "Maker action submitted"
                root.statusDetail = "Waiting for finalized chain evidence before the next actor turn"
            })
    }

    function health() {
        root.run(root.backend.health(), "Checking daemon health", function(result) {
            root.routeCount = (result.routes ?? []).length
            root.statusMode = result.ready === true && result.degraded !== true ? "success" : "error"
            root.statusTitle = result.degraded === true ? "Maker is operating in degraded mode" : "Maker systems ready"
            root.statusDetail = root.routeCount + (root.routeCount === 1 ? " active route" : " active routes") + " · owner socket verified"
        })
    }

    function chatStatus() {
        root.run(root.backend.chatStatus(), "Reading Logos Chat session", function(result) {
            root.chatAddress = String(result.address ?? "")
            root.chatState = String(result.state ?? "unknown")
            root.statusMode = result.online === true ? "success" : "working"
            root.statusTitle = result.online === true ? "Private Chat is online" : "Private Chat is starting"
            root.statusDetail = result.session_bound === true
                ? "Direct Taker conversation bound for this app session"
                : "Share this session address with the Taker"
        })
    }

    function resetChat() {
        root.run(root.backend.resetChat(), "Resetting private Chat session", function(result) {
            root.chatAddress = String(result.address ?? "")
            root.chatState = String(result.state ?? "online")
            root.statusMode = "working"
            root.statusTitle = "Private Chat session reset"
            root.statusDetail = "Share this address again; no previous peer binding remains"
        })
    }

    function saveRoute() {
        var requestId = "maker-ui-route-" + Date.now()
        root.run(root.backend.saveRoute(
            requestId, pair.currentText, direction.currentText,
            minimum.text, maximum.text, ttl.text, lezLot.text, foreignLot.text),
            "Saving route atomically", function(result) {
                root.lastSavedRoute = pair.currentText + " · " + direction.currentText
                root.statusMode = "success"
                root.statusTitle = "Route and price committed"
                root.statusDetail = "Policy and pricing changed together in one transaction"
            })
    }

    function history() {
        root.run(root.backend.history(), "Loading swap history", function(result) {
            var swaps = Array.isArray(result) ? result : (result.swaps ?? [])
            root.swapCount = swaps.length
            if (swaps.length > 0) {
                var candidate = swaps[swaps.length - 1]
                root.latestSwap = String(candidate.id ?? candidate.swap_id ?? "")
                root.currentState = String(candidate.phase ?? candidate.state ?? "")
            }
            root.statusMode = "success"
            root.statusTitle = swaps.length === 1 ? "1 swap in maker history" : swaps.length + " swaps in maker history"
            root.statusDetail = swaps.length > 0 ? "Latest state: " + root.currentState : "No swaps have been recorded yet"
        })
    }

    function monitor() {
        root.run(root.backend.monitor(swapId.text), "Reading actor progress", function(result) {
            root.currentState = String(result.schedule_state ?? result.state ?? "unknown")
            generation.text = String(result.progress_generation ?? result.generation ?? generation.text)
            root.statusMode = "success"
            root.statusTitle = "Actor state refreshed"
            root.statusDetail = root.currentState.split("_").join(" ") + " · generation " + generation.text
        })
    }

    function terminal(action) {
        var requestId = ["maker-ui", action, swapId.text, generation.text].join("-")
        var operation = action === "claim"
            ? root.backend.claim(requestId, swapId.text, generation.text)
            : root.backend.refund(requestId, swapId.text, generation.text)
        root.run(operation, action === "claim" ? "Submitting claim" : "Submitting refund", function(result) {
            root.statusMode = "success"
            root.statusTitle = action === "claim" ? "Claim accepted" : "Refund accepted"
            root.statusDetail = "The actor verified the expected progress generation"
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
                    implicitHeight: 150
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
                        anchors.fill: parent; anchors.margins: 24; spacing: 24
                        ColumnLayout {
                            Layout.fillWidth: true; spacing: 7
                            Label {
                                text: "LEZ / BTC — Maker Desk"
                                color: "#F7F8FA"; font.pixelSize: 30; font.weight: Font.Bold; font.letterSpacing: -0.7
                            }
                            Label {
                                text: "Quote both directions, publish wallet-owned inventory, settle atomically."
                                color: "#9FA9B9"; font.pixelSize: 13
                            }
                        }
                        ColumnLayout {
                            Layout.alignment: Qt.AlignRight | Qt.AlignVCenter; spacing: 8
                            RowLayout {
                                Layout.alignment: Qt.AlignRight; spacing: 9
                                Label {
                                    text: "ACCOUNT"
                                    color: "#6F7A8B"; font.pixelSize: 9
                                    font.weight: Font.Bold; font.letterSpacing: 1.3
                                }
                                Rectangle {
                                    implicitWidth: 9; implicitHeight: 9; radius: 5
                                    color: makerWallet.currentIndex === 0 ? "#8950FA" : "#FA50C1"
                                }
                                LuxeCombo {
                                    id: makerWallet
                                    objectName: "makerBtcWallet"
                                    model: ["Munich Vault 01 · Maker", "Basel Vault 02 · Maker"]
                                    implicitWidth: 240
                                    onActivated: root.refreshBtcMarket(false)
                                }
                            }
                            Rectangle {
                                Layout.alignment: Qt.AlignRight
                                implicitWidth: connectionRow.implicitWidth + 22; implicitHeight: 32; radius: 16
                                color: root.ready ? "#11271F" : "#292318"
                                border.width: 1; border.color: root.ready ? "#497621" : "#62438B"
                                RowLayout {
                                    id: connectionRow; anchors.centerIn: parent; spacing: 8
                                    Rectangle { implicitWidth: 7; implicitHeight: 7; radius: 4; color: root.ready ? "#7EE100" : "#8950FA" }
                                    Label {
                                        objectName: "makerConnection"
                                        text: root.ready ? "Backend connected" : "Connecting securely"
                                        color: root.ready ? "#B8F57C" : "#C6AAFF"
                                        font.pixelSize: 11; font.weight: Font.DemiBold
                                    }
                                }
                            }
                            Label {
                                Layout.alignment: Qt.AlignRight
                                text: "Owner-local · atomic commits · generation fenced"
                                color: "#6F7A8B"; font.pixelSize: 10
                            }
                        }
                    }
                }

                Rectangle {
                    Layout.fillWidth: true; implicitHeight: 76; radius: 14
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
                        RowLayout {
                            spacing: 6
                            LuxeButton {
                                objectName: "makerMarketRefresh"
                                text: "Refresh wallet inventory"
                                quiet: true
                                enabled: root.ready && !root.busy && !root.btcMarketBusy
                                onClicked: root.refreshBtcMarket(false)
                            }
                            LuxeButton {
                                objectName: "makerHealth"
                                text: "Check service"
                                quiet: true
                                enabled: root.ready && !root.busy
                                onClicked: root.health()
                            }
                        }
                    }
                }

                // ----- Market strip: live counts per side and the runner.
                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: 74
                    radius: 14
                    color: "#0E1520"
                    border.width: 1
                    border.color: "#26334A"
                    RowLayout {
                        anchors.fill: parent; anchors.margins: 16; spacing: 14
                        Repeater {
                            model: [
                                {label: "SELL LEZ OFFERS", value: root.openOffersBySide("taker_sells_foreign"), accent: "#7EE100"},
                                {label: "SELL BTC OFFERS", value: root.openOffersBySide("taker_sells_lez"), accent: "#B997FF"},
                                {label: "COMPLETED SWAPS", value: root.btcMarket.summary?.completed_swaps ?? 0, accent: "#FA50C1"}
                            ]
                            delegate: ColumnLayout {
                                id: stripCell
                                required property var modelData
                                spacing: 3
                                Label {
                                    text: stripCell.modelData.label
                                    color: "#6F7A8B"; font.pixelSize: 8
                                    font.weight: Font.Bold; font.letterSpacing: 1.0
                                }
                                Label {
                                    text: String(stripCell.modelData.value)
                                    color: stripCell.modelData.accent
                                    font.pixelSize: 20; font.weight: Font.Bold
                                    font.family: "DejaVu Sans Mono"
                                }
                            }
                        }
                        Item { Layout.fillWidth: true }
                        Rectangle {
                            implicitWidth: makerRunnerLabel.implicitWidth + 24; implicitHeight: 31; radius: 2
                            color: root.btcMarket.runner_ready === true ? "#142A20" : "#2A1820"
                            border.width: 1; border.color: root.btcMarket.runner_ready === true ? "#416F4F" : "#724051"
                            Label {
                                id: makerRunnerLabel; anchors.centerIn: parent
                                text: root.btcMarket.runner_busy === true ? "RUNNER ACTIVE"
                                    : root.btcMarket.runner_ready === true ? "RUNNER READY" : "RUNNER OFFLINE"
                                color: root.btcMarket.runner_ready === true ? "#7EE100" : "#FF9FAF"
                                font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.9
                            }
                        }
                    }
                }

                // ----- Compose (left) beside the live order book (right) on
                // wide views; stacked on narrow ones. One anchored Item with
                // explicit geometry only — the layout engine's attached
                // properties mis-sized these cards (legs wider than their
                // card, the right card vertically re-centered).
                Item {
                    id: deskArea
                    property bool deskWide: scroll.availableWidth >= 1180
                    Layout.fillWidth: true
                    implicitHeight: deskWide
                        ? Math.max(composeCard.implicitHeight, makerMarketPanel.implicitHeight)
                        : composeCard.implicitHeight + 16 + makerMarketPanel.implicitHeight

                // ----- The compose card: a Cowswap-shaped two-leg quote with
                // a fixed preset, a sell-side toggle, and a live rate line.
                Rectangle {
                    id: composeCard
                    objectName: "makerComposeCard"
                    width: deskArea.deskWide ? 560 : deskArea.width
                    height: composeColumn.implicitHeight + 44
                    radius: 16
                    color: "#101722"
                    border.width: 1
                    border.color: "#38465A"

                    // Plain anchored column: children take the card's width
                    // directly (width: parent.width), nothing depends on
                    // Layout attached properties.
                    Column {
                        id: composeColumn
                        anchors.left: parent.left; anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.margins: 22
                        spacing: 14

                        Row {
                            width: parent.width; spacing: 12
                            Column {
                                width: parent.width - composeSide.width - 12; spacing: 2
                                Label { text: "Compose an offer"; color: "#F5F6F8"; font.pixelSize: 17; font.weight: Font.DemiBold }
                                Label {
                                    text: "One offer per publish"
                                    color: "#7F8A9B"; font.pixelSize: 11
                                    width: parent.width
                                    elide: Text.ElideMiddle
                                }
                            }
                            SideToggle {
                                id: composeSide
                                value: root.sellSide
                                onPicked: function(side) { root.sellSide = side }
                            }
                        }

                        SwapLeg {
                            objectName: root.sellSide === "lez" ? "makerSellLegLez" : "makerSellLegBtc"
                            width: parent.width
                            label: "YOU SELL"
                            asset: root.sellSide === "lez" ? "LEZ" : "BTC"
                            amount: root.sellSide === "lez" ? "1,000" : "0.01000000"
                            accent: root.sellSide === "lez" ? "#7EE100" : "#B997FF"
                            note: root.sellSide === "lez"
                                ? "Locked in the LEZ escrow until settlement"
                                : "Locked in the Bitcoin P2TR contract until settlement"
                        }

                        Row {
                            width: parent.width; spacing: 10
                            Rectangle { height: 1; width: (parent.width - 30 - 20) / 2; anchors.verticalCenter: parent.verticalCenter; color: "#1C2739" }
                            Rectangle {
                                width: 30; height: 30; radius: 15
                                color: "#1A2334"; border.width: 1; border.color: "#3A4B6B"
                                Label {
                                    anchors.centerIn: parent
                                    text: "⇅"
                                    color: "#9FB0D0"; font.pixelSize: 14; font.weight: Font.Bold
                                }
                            }
                            Rectangle { height: 1; width: (parent.width - 30 - 20) / 2; anchors.verticalCenter: parent.verticalCenter; color: "#1C2739" }
                        }

                        SwapLeg {
                            width: parent.width
                            label: "YOU RECEIVE"
                            asset: root.sellSide === "lez" ? "BTC" : "LEZ"
                            amount: root.sellSide === "lez" ? "0.01000000" : "1,000"
                            accent: root.sellSide === "lez" ? "#B997FF" : "#7EE100"
                            note: root.sellSide === "lez"
                                ? "Claimed from the P2TR contract once the secret is revealed"
                                : "Claimed from the LEZ escrow once the secret is revealed"
                        }

                        Rectangle {
                            width: parent.width
                            height: rateRow.implicitHeight + 20
                            radius: 10
                            color: "#0C1320"
                            border.width: 1; border.color: "#233250"
                            RowLayout {
                                id: rateRow
                                anchors.left: parent.left; anchors.right: parent.right
                                anchors.top: parent.top
                                anchors.margins: 10; spacing: 12
                                Label {
                                    text: "MARKET RATE"
                                    color: "#6F7A8B"; font.pixelSize: 9
                                    font.weight: Font.Bold; font.letterSpacing: 1.0
                                }
                                Label {
                                    text: "1 BTC = 100,000 LEZ"
                                    color: "#D9E2F2"; font.pixelSize: 12; font.weight: Font.DemiBold
                                }
                                Item { Layout.fillWidth: true }
                                Label {
                                    text: root.sellSide === "lez" ? "ROUTE BTC → LEZ" : "ROUTE LEZ → BTC"
                                    color: root.sellSide === "lez" ? "#B997FF" : "#7EE100"
                                    font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.8
                                }
                                Label {
                                    text: "· network fee ≈ 1,000 sat per leg"
                                    color: "#5F6B7D"; font.pixelSize: 9
                                }
                            }
                        }

                        Row {
                            width: parent.width; spacing: 10
                            Item { width: parent.width - 250; height: 1 }
                            LuxeButton {
                                objectName: "makerNewOffer"
                                text: root.sellSide === "lez"
                                    ? "Sell 1,000 LEZ for BTC"
                                    : "Sell 0.01 BTC for LEZ"
                                primary: true
                                implicitWidth: 240
                                enabled: root.ready && root.btcMarketReady
                                onClicked: root.newOfferOpen = true
                            }
                        }
                    }
                }

                // ----- The order book: this wallet's offers and swaps.
                Rectangle {
                    id: makerMarketPanel
                    objectName: "makerBtcMarket"
                    anchors.top: deskArea.deskWide ? parent.top : composeCard.bottom
                    anchors.topMargin: deskArea.deskWide ? 0 : 16
                    anchors.left: deskArea.deskWide ? composeCard.right : parent.left
                    anchors.leftMargin: deskArea.deskWide ? 16 : 0
                    anchors.right: parent.right
                    implicitHeight: makerMarketColumn.implicitHeight + 44
                    radius: 16; color: "#101722"; border.width: 1
                    border.color: root.btcMarketReady ? "#38465A" : "#5C3341"

                    ColumnLayout {
                        id: makerMarketColumn
                        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                        anchors.margins: 22; spacing: 16

                        RowLayout {
                            Layout.fillWidth: true; spacing: 12
                            StepBadge { number: "01"; accent: "#FA50C1" }
                            ColumnLayout {
                                Layout.fillWidth: true; spacing: 2
                                Label { text: "My orders"; color: "#F5F6F8"; font.pixelSize: 17; font.weight: Font.DemiBold }
                                Label {
                                    text: Number((root.btcMarket.inventory ?? []).filter(function(item) { return item.state === "pending" }).length)
                                        + " open offers in " + makerWallet.currentText
                                        + " · you drive only your side of each gate"
                                    color: "#7F8A9B"; font.pixelSize: 11
                                }
                            }
                        }

                        // Toggleable filters: ACTIVE and OPEN can be on
                        // together; DONE is exclusive with them. No ALL —
                        // an empty selection shows everything.
                        Row {
                            spacing: 7
                            FilterTab {
                                objectName: "makerMarketTabActive"
                                label: "ACTIVE"
                                count: root.marketCount("active")
                                alert: root.attentionCount() > 0
                                active: root.showActive
                                onPicked: root.showActive = !root.showActive
                            }
                            FilterTab {
                                objectName: "makerMarketTabOpen"
                                label: "OPEN OFFERS"; count: root.marketCount("open")
                                active: root.showOpen
                                onPicked: root.showOpen = !root.showOpen
                            }
                            FilterTab {
                                label: "DONE"; count: root.marketCount("done")
                                active: root.showDone
                                onPicked: {
                                    root.showDone = !root.showDone
                                    if (root.showDone) { root.showActive = false; root.showOpen = false }
                                }
                            }
                        }

                        Rectangle {
                            id: makerAttentionBanner
                            property var target: root.otherWalletNeedingAction()
                            visible: makerAttentionBanner.target !== null
                            Layout.fillWidth: true
                            implicitHeight: visible ? 42 : 0
                            radius: 10
                            color: makerAttentionArea.containsMouse ? "#2A1930" : "#1F1526"
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
                                    text: makerAttentionBanner.target
                                        ? makerAttentionBanner.target.label + " has "
                                          + makerAttentionBanner.target.count
                                          + (Number(makerAttentionBanner.target.count) === 1
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
                                id: makerAttentionArea
                                anchors.fill: parent
                                hoverEnabled: true
                                cursorShape: Qt.PointingHandCursor
                                onClicked: {
                                    if (!makerAttentionBanner.target) return
                                    makerWallet.currentIndex = makerAttentionBanner.target.index
                                    root.refreshBtcMarket(false)
                                }
                            }
                        }

                        Label {
                            visible: root.filteredMarketRows().length === 0
                            text: !root.btcMarketReady ? "Loading wallet market…"
                                : root.marketRows().length === 0 ? "No offers or swaps belong to this wallet yet."
                                : (root.showActive || root.showOpen || root.showDone)
                                  ? "Nothing under the selected filters for " + makerWallet.currentText + "."
                                  : "All filters are off — turn on ACTIVE, OPEN OFFERS or DONE."
                            color: "#7F8A9B"; font.pixelSize: 12
                        }

                        Repeater {
                            model: root.filteredMarketRows().filter(function(row) { return row.kind === "offer" })
                            delegate: Rectangle {
                                id: makerOfferRow
                                required property var modelData
                                readonly property bool sellsLez:
                                    (makerOfferRow.modelData.direction ?? "taker_sells_foreign") === "taker_sells_foreign"
                                Layout.fillWidth: true; implicitHeight: 68; radius: 10
                                color: "#0D141E"; border.width: 1
                                border.color: makerOfferRow.modelData.state === "pending" ? "#36465B" : "#252E3C"
                                RowLayout {
                                    anchors.fill: parent; anchors.margins: 13; spacing: 14
                                    Rectangle {
                                        implicitWidth: 64; implicitHeight: 24; radius: 3
                                        color: makerOfferRow.sellsLez ? "#15251C" : "#221733"
                                        border.width: 1
                                        border.color: makerOfferRow.sellsLez ? "#3C6B33" : "#6944A2"
                                        Label {
                                            anchors.centerIn: parent
                                            text: makerOfferRow.sellsLez ? "SELL LEZ" : "SELL BTC"
                                            color: makerOfferRow.sellsLez ? "#7EE100" : "#C5A9FF"
                                            font.pixelSize: 8; font.weight: Font.Bold; font.letterSpacing: 0.8
                                        }
                                    }
                                    ColumnLayout {
                                        Layout.fillWidth: true; spacing: 2
                                        Label { text: String(makerOfferRow.modelData.offer_id); color: "#E9EDF3"; font.pixelSize: 10; font.family: "DejaVu Sans Mono"; elide: Text.ElideMiddle; Layout.fillWidth: true }
                                        Label { text: String(makerOfferRow.modelData.state).toUpperCase(); color: "#718095"; font.pixelSize: 8; font.weight: Font.Bold; font.letterSpacing: 0.8 }
                                    }
                                    Label {
                                        text: makerOfferRow.sellsLez ? "1,000 LEZ" : "0.01000000 BTC"
                                        color: "#AAB4C3"; font.pixelSize: 11; font.weight: Font.DemiBold
                                    }
                                    Label { text: "→"; color: "#687486"; font.pixelSize: 13 }
                                    Label {
                                        text: makerOfferRow.sellsLez ? "0.01 BTC" : "1,000 LEZ"
                                        color: makerOfferRow.sellsLez ? "#B997FF" : "#7EE100"
                                        font.pixelSize: 11; font.weight: Font.DemiBold
                                    }
                                    LuxeButton {
                                        objectName: "makerWithdrawOffer"
                                        visible: makerOfferRow.modelData.state === "pending"
                                        text: "Withdraw"; destructive: true
                                        enabled: root.ready && !root.btcMarketBusy
                                        onClicked: root.withdrawBtcOffer(makerOfferRow.modelData)
                                    }
                                }
                            }
                        }

                        Repeater {
                            model: root.filteredMarketRows().filter(function(row) { return row.kind === "swap" })
                            delegate: Rectangle {
                                id: makerSwapRow
                                required property var modelData
                                readonly property var effects: makerSwapRow.modelData.effects ?? []
                                readonly property bool sellsLez:
                                    (makerSwapRow.modelData.direction ?? "taker_sells_foreign") === "taker_sells_foreign"
                                readonly property bool hashesShown:
                                    root.expandedSwaps[makerSwapRow.modelData.ui_swap_id] === true
                                        && makerSwapRow.effects.length > 0
                                Layout.fillWidth: true; radius: 10
                                // The NEEDS YOU badge rides the title row, so
                                // budget its height when this row waits on
                                // this desk — anchored content taller than the
                                // delegate would otherwise bleed onto the next.
                                implicitHeight: (makerSwapRow.modelData.progress_detail ? 104 : 86)
                                    + (makerSwapRow.modelData.can_act === true ? 16 : 0)
                                    + (makerSwapRow.hashesShown ? makerSwapRow.effects.length * 21 + 30 : 0)
                                color: makerSwapRow.modelData.can_act === true ? "#17152A" : "#0D141E"
                                border.width: 1; border.color: makerSwapRow.modelData.can_act === true ? "#8950FA" : "#28364A"
                                RowLayout {
                                    anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                                    anchors.margins: 13; spacing: 14
                                    ColumnLayout {
                                        Layout.fillWidth: true; spacing: 3
                                        RowLayout {
                                            Layout.fillWidth: true; spacing: 8
                                            Rectangle {
                                                implicitWidth: 58; implicitHeight: 18; radius: 3
                                                color: makerSwapRow.sellsLez ? "#15251C" : "#221733"
                                                border.width: 1
                                                border.color: makerSwapRow.sellsLez ? "#3C6B33" : "#6944A2"
                                                Label {
                                                    anchors.centerIn: parent
                                                    text: makerSwapRow.sellsLez ? "SELL LEZ" : "SELL BTC"
                                                    color: makerSwapRow.sellsLez ? "#7EE100" : "#C5A9FF"
                                                    font.pixelSize: 7; font.weight: Font.Bold; font.letterSpacing: 0.7
                                                }
                                            }
                                            Label {
                                                text: String(makerSwapRow.modelData.taker_wallet_label) + " · " + String(makerSwapRow.modelData.state_label)
                                                color: "#F1F3F6"; font.pixelSize: 12; font.weight: Font.DemiBold
                                                elide: Text.ElideRight; Layout.fillWidth: true
                                            }
                                            Rectangle {
                                                objectName: "makerSwapNeedsYouBadge"
                                                visible: makerSwapRow.modelData.can_act === true
                                                implicitWidth: badgeRow.implicitWidth + 16; implicitHeight: 22; radius: 11
                                                color: "#2A1530"
                                                border.width: 1; border.color: "#FA50C1"
                                                RowLayout {
                                                    id: badgeRow
                                                    anchors.centerIn: parent; spacing: 6
                                                    Rectangle {
                                                        implicitWidth: 7; implicitHeight: 7; radius: 4
                                                        color: "#FF6AD5"
                                                        SequentialAnimation on opacity {
                                                            loops: Animation.Infinite
                                                            NumberAnimation { to: 0.25; duration: 700; easing.type: Easing.InOutQuad }
                                                            NumberAnimation { to: 1; duration: 700; easing.type: Easing.InOutQuad }
                                                        }
                                                    }
                                                    Label {
                                                        text: "NEEDS YOU"
                                                        color: "#FFB8EC"; font.pixelSize: 8
                                                        font.weight: Font.Bold; font.letterSpacing: 0.8
                                                    }
                                                }
                                            }
                                        }
                                        Label {
                                            text: String(makerSwapRow.modelData.ui_swap_id) + "  /  " + String(makerSwapRow.modelData.offer_id)
                                            color: "#68768A"; font.pixelSize: 9; font.family: "DejaVu Sans Mono"; elide: Text.ElideMiddle; Layout.fillWidth: true
                                        }
                                        RowLayout {
                                            Layout.fillWidth: true; spacing: 8
                                            Rectangle {
                                                Layout.fillWidth: true; implicitHeight: 4; radius: 2; color: "#252E3C"
                                                Rectangle {
                                                    id: makerSwapFill
                                                    property bool settled: false
                                                    width: parent.width * Number(makerSwapRow.modelData.progress_percent ?? 0) / 100
                                                    height: parent.height; radius: 2
                                                    color: makerSwapRow.modelData.state === "completed" ? "#7EE100" : "#8950FA"
                                                    Timer { interval: 400; running: true; onTriggered: makerSwapFill.settled = true }
                                                    Behavior on width {
                                                        enabled: makerSwapFill.settled
                                                        NumberAnimation { duration: 600; easing.type: Easing.OutCubic }
                                                    }
                                                }
                                            }
                                            Label {
                                                text: String(makerSwapRow.modelData.progress_percent ?? 0) + "%"
                                                color: "#9AA6B8"; font.pixelSize: 9; font.family: "DejaVu Sans Mono"
                                            }
                                        }
                                        Label {
                                            visible: !!makerSwapRow.modelData.progress_detail
                                            text: String(makerSwapRow.modelData.progress_detail ?? "")
                                                + (makerSwapRow.modelData.eta_display ? "  ·  " + String(makerSwapRow.modelData.eta_display) : "")
                                            color: "#8E7BC6"; font.pixelSize: 10
                                            elide: Text.ElideRight; Layout.fillWidth: true
                                        }
                                        ColumnLayout {
                                            visible: makerSwapRow.hashesShown
                                            Layout.fillWidth: true; Layout.topMargin: 4; spacing: 4
                                            Repeater {
                                                model: makerSwapRow.hashesShown ? makerSwapRow.effects : []
                                                delegate: RowLayout {
                                                    id: makerFxRow
                                                    required property var modelData
                                                    property bool copied: false
                                                    Layout.fillWidth: true; spacing: 8
                                                    Label {
                                                        text: String(makerFxRow.modelData.chain ?? "").toUpperCase()
                                                        color: makerFxRow.modelData.chain === "Bitcoin" ? "#B997FF" : "#7EE100"
                                                        font.pixelSize: 8; font.weight: Font.Bold; font.letterSpacing: 0.6
                                                        Layout.preferredWidth: 52
                                                    }
                                                    Label {
                                                        text: String(makerFxRow.modelData.label ?? "")
                                                        color: "#8E99AA"; font.pixelSize: 9
                                                        Layout.preferredWidth: 148; elide: Text.ElideRight
                                                    }
                                                    Label {
                                                        text: String(makerFxRow.modelData.transaction_id ?? "")
                                                        color: "#C7CED9"; font.pixelSize: 9; font.family: "DejaVu Sans Mono"
                                                        elide: Text.ElideMiddle; Layout.fillWidth: true
                                                    }
                                                    Label {
                                                        text: makerFxRow.copied ? "COPIED ✓" : "COPY"
                                                        color: makerFxRow.copied ? "#7EE100" : "#B997FF"
                                                        font.pixelSize: 8; font.weight: Font.Bold; font.letterSpacing: 0.8
                                                        MouseArea {
                                                            anchors.fill: parent; anchors.margins: -4
                                                            cursorShape: Qt.PointingHandCursor
                                                            onClicked: {
                                                                root.copyText(makerFxRow.modelData.transaction_id)
                                                                makerFxRow.copied = true
                                                                makerFxCopyReset.restart()
                                                            }
                                                        }
                                                    }
                                                    Timer { id: makerFxCopyReset; interval: 1600; onTriggered: makerFxRow.copied = false }
                                                }
                                            }
                                            Label {
                                                text: "Verify any hash — paste it into the proof explorer search at 127.0.0.1:3003"
                                                color: "#5F6B7D"; font.pixelSize: 8; font.letterSpacing: 0.4
                                            }
                                        }
                                    }
                                    Label {
                                        visible: makerSwapRow.modelData.can_act !== true
                                        text: makerSwapRow.modelData.action_role === "taker" ? "WAITING FOR TAKER" : String(makerSwapRow.modelData.state).toUpperCase()
                                        color: "#7B8798"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.7
                                    }
                                    LuxeButton {
                                        objectName: "makerSwapHashes"
                                        visible: makerSwapRow.effects.length > 0
                                        text: makerSwapRow.hashesShown ? "Hide hashes" : "Tx hashes"
                                        quiet: true
                                        onClicked: root.toggleSwapHashes(makerSwapRow.modelData.ui_swap_id)
                                    }
                                    LuxeButton {
                                        objectName: "makerSwapAction"
                                        visible: makerSwapRow.modelData.can_act === true
                                        text: String(makerSwapRow.modelData.action_label ?? "Continue")
                                        primary: true; enabled: root.ready && !root.btcMarketBusy
                                        onClicked: root.runMakerAction(makerSwapRow.modelData)
                                    }
                                }
                            }
                        }

                        Rectangle {
                            id: makerBalanceLedger
                            objectName: "makerBalanceLedger"
                            Layout.fillWidth: true
                            implicitHeight: root.btcMarket.latest_balance_evidence ? 124 : 64
                            radius: 10; color: "#0B111A"; border.width: 1
                            border.color: root.btcMarket.latest_balance_evidence ? "#3D671E" : "#283244"
                            ColumnLayout {
                                anchors.fill: parent; anchors.margins: 13; spacing: 7
                                Label {
                                    text: root.btcMarket.latest_balance_evidence
                                        ? "LATEST FINALIZED BALANCE · " + String(root.btcMarket.latest_balance_evidence.run_id)
                                        : "WALLET BALANCE PROOF"
                                    color: root.btcMarket.latest_balance_evidence ? "#7EE100" : "#748196"
                                    font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.7
                                }
                                Label {
                                    visible: !root.btcMarket.latest_balance_evidence
                                    text: "This wallet's opening → closing ledger appears after its interactive swap completes."
                                    color: "#7A8697"; font.pixelSize: 10
                                }
                                RowLayout {
                                    visible: root.btcMarket.latest_balance_evidence !== null
                                    Layout.fillWidth: true
                                    Label { text: "BTC"; color: "#8950FA"; font.pixelSize: 10; font.weight: Font.Bold; Layout.preferredWidth: 38 }
                                    Label { text: root.btcMarket.latest_balance_evidence ? root.formatBtcSats(root.btcMarket.latest_balance_evidence.wallet.balances.bitcoin.opening) : ""; color: "#9CA7B7"; font.pixelSize: 10 }
                                    Label { text: "→"; color: "#5F6B7D"; font.pixelSize: 10 }
                                    Label { text: root.btcMarket.latest_balance_evidence ? root.formatBtcSats(root.btcMarket.latest_balance_evidence.wallet.balances.bitcoin.closing) : ""; color: "#F0F3F7"; font.pixelSize: 10; font.weight: Font.DemiBold }
                                    Item { Layout.fillWidth: true }
                                    Label { text: root.btcMarket.latest_balance_evidence ? root.formatSignedBtc(root.btcMarket.latest_balance_evidence.wallet.balances.bitcoin.net_change) : ""; color: "#7EE100"; font.pixelSize: 10; font.weight: Font.Bold }
                                }
                                RowLayout {
                                    visible: root.btcMarket.latest_balance_evidence !== null
                                    Layout.fillWidth: true
                                    Label { text: "LEZ"; color: "#7EE100"; font.pixelSize: 10; font.weight: Font.Bold; Layout.preferredWidth: 38 }
                                    Label { text: root.btcMarket.latest_balance_evidence ? root.formatLez(root.btcMarket.latest_balance_evidence.wallet.balances.lez.opening) : ""; color: "#9CA7B7"; font.pixelSize: 10 }
                                    Label { text: "→"; color: "#5F6B7D"; font.pixelSize: 10 }
                                    Label { text: root.btcMarket.latest_balance_evidence ? root.formatLez(root.btcMarket.latest_balance_evidence.wallet.balances.lez.closing) : ""; color: "#F0F3F7"; font.pixelSize: 10; font.weight: Font.DemiBold }
                                    Item { Layout.fillWidth: true }
                                    Label { text: root.btcMarket.latest_balance_evidence ? root.formatSignedLez(root.btcMarket.latest_balance_evidence.wallet.balances.lez.net_change) : ""; color: "#FA50C1"; font.pixelSize: 10; font.weight: Font.Bold }
                                }
                            }
                        }
                    }
                }
                } // compose + orders desk area

                Label {
                    // Non-Bitcoin routes are parked while the M3 demo is BTC-only.
                    visible: false
                    text: "ADVANCED SERVICE CONTROLS · PREPARED NON-BITCOIN ROUTES"
                    color: "#566377"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 1.1
                    Layout.topMargin: 4
                }

                GridLayout {
                    visible: false
                    Layout.fillWidth: true
                    columns: width > 1040 ? 2 : 1
                    columnSpacing: 16
                    rowSpacing: 16

                    Rectangle {
                        Layout.fillWidth: true; Layout.alignment: Qt.AlignTop
                        implicitHeight: routeColumn.implicitHeight + 44
                        radius: 16; color: "#101722"; border.width: 1; border.color: "#263144"
                        ColumnLayout {
                            id: routeColumn
                            anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                            anchors.margins: 22
                            spacing: 15
                            RowLayout {
                                Layout.fillWidth: true; spacing: 12
                                StepBadge { number: "01"; accent: "#8950FA" }
                                ColumnLayout {
                                    Layout.fillWidth: true; spacing: 2
                                    Label { text: "Configure a market"; color: "#F5F6F8"; font.pixelSize: 17; font.weight: Font.DemiBold }
                                    Label { text: "Route policy and price settle in one transaction"; color: "#7F8A9B"; font.pixelSize: 11 }
                                }
                            }
                            GridLayout {
                                Layout.fillWidth: true; columns: 2; columnSpacing: 10; rowSpacing: 7
                                FieldLabel { text: "FOREIGN ASSET" }
                                FieldLabel { text: "TAKER DIRECTION" }
                                LuxeCombo { id: pair; objectName: "makerPair"; model: ["Zcash", "Bitcoin", "Monero"]; Layout.fillWidth: true }
                                LuxeCombo { id: direction; objectName: "makerDirection"; model: ["TakerSellsLez", "TakerSellsForeign"]; Layout.fillWidth: true }
                                FieldLabel { text: "MINIMUM FOREIGN UNITS" }
                                FieldLabel { text: "MAXIMUM FOREIGN UNITS" }
                                LuxeField { id: minimum; objectName: "makerForeignUnits"; text: "10000"; Layout.fillWidth: true }
                                LuxeField { id: maximum; text: "10000"; Layout.fillWidth: true }
                                FieldLabel { text: "OFFER LIFETIME · SECONDS" }
                                FieldLabel { text: "PRICE · LEZ / FOREIGN LOT" }
                                LuxeField { id: ttl; text: "7200"; Layout.fillWidth: true }
                                RowLayout {
                                    Layout.fillWidth: true; spacing: 8
                                    LuxeField { id: lezLot; objectName: "makerLezUnits"; text: "5"; Layout.fillWidth: true }
                                    Label { text: "/"; color: "#697587"; font.pixelSize: 16 }
                                    LuxeField { id: foreignLot; text: "2"; Layout.fillWidth: true }
                                }
                            }
                            LuxeButton {
                                objectName: "makerSave"
                                text: "Save route atomically"
                                primary: true
                                enabled: root.ready && !root.busy
                                Layout.fillWidth: true
                                onClicked: root.saveRoute()
                            }
                            Rectangle {
                                Layout.fillWidth: true; implicitHeight: 58; radius: 10
                                color: "#0D141E"; border.width: 1; border.color: "#222D3D"
                                RowLayout {
                                    anchors.fill: parent; anchors.margins: 13
                                    ColumnLayout {
                                        Layout.fillWidth: true; spacing: 2
                                        Label { text: "LAST ATOMIC COMMIT"; color: "#687486"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.8 }
                                        Label { text: root.lastSavedRoute; color: "#B9C2CF"; font.pixelSize: 11 }
                                    }
                                    Label { text: "POLICY + PRICE"; color: "#7EE100"; font.pixelSize: 9; font.weight: Font.Bold }
                                }
                            }
                        }
                    }

                    Rectangle {
                        id: activePanel
                        objectName: "makerActive"
                        Layout.fillWidth: true; Layout.alignment: Qt.AlignTop
                        implicitHeight: actorColumn.implicitHeight + 44
                        radius: 16; color: "#101722"; border.width: 1; border.color: "#263144"
                        ColumnLayout {
                            id: actorColumn
                            anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                            anchors.margins: 22
                            spacing: 15
                            RowLayout {
                                Layout.fillWidth: true; spacing: 12
                                StepBadge { number: "02"; accent: "#FA50C1" }
                                ColumnLayout {
                                    Layout.fillWidth: true; spacing: 2
                                    Label { text: "Oversee a live swap"; color: "#F5F6F8"; font.pixelSize: 17; font.weight: Font.DemiBold }
                                    Label { text: "Read current actor state before any terminal action"; color: "#7F8A9B"; font.pixelSize: 11 }
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
                            FieldLabel { text: "SWAP ID" }
                            LuxeField {
                                id: swapId; placeholderText: "Swap ID"; text: root.latestSwap
                                Layout.fillWidth: true; font.family: "DejaVu Sans Mono"
                            }
                            FieldLabel { text: "EXPECTED PROGRESS GENERATION" }
                            LuxeField { id: generation; placeholderText: "Generation"; text: "0"; Layout.fillWidth: true }
                            LuxeButton {
                                text: "Monitor"
                                primary: true
                                enabled: root.ready && !root.busy && swapId.text.length > 0
                                Layout.fillWidth: true
                                onClicked: root.monitor()
                            }
                            RowLayout {
                                Layout.fillWidth: true; spacing: 10
                                LuxeButton {
                                    text: "Claim"; enabled: root.ready && !root.busy && swapId.text.length > 0
                                    Layout.fillWidth: true; onClicked: root.terminal("claim")
                                }
                                LuxeButton {
                                    text: "Refund"; destructive: true
                                    enabled: root.ready && !root.busy && swapId.text.length > 0
                                    Layout.fillWidth: true; onClicked: root.terminal("refund")
                                }
                            }
                            Rectangle {
                                Layout.fillWidth: true; implicitHeight: 58; radius: 10
                                color: "#171A22"; border.width: 1; border.color: "#35323A"
                                RowLayout {
                                    anchors.fill: parent; anchors.margins: 13; spacing: 10
                                    Label { text: "FENCED"; color: "#FA50C1"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 1 }
                                    Label {
                                        text: "Claim and refund require the exact generation returned by Monitor."
                                        color: "#A8AFBB"; font.pixelSize: 10; Layout.fillWidth: true; wrapMode: Text.WordWrap
                                    }
                                }
                            }
                        }
                    }
                }

                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: 94
                    radius: 16
                    color: "#101722"
                    border.width: 1
                    border.color: "#263144"
                    RowLayout {
                        anchors.fill: parent; anchors.margins: 20; spacing: 18
                        Rectangle {
                            implicitWidth: 48; implicitHeight: 48; radius: 12; color: "#1A2432"
                            Label { anchors.centerIn: parent; text: String(root.swapCount); color: "#7EE100"; font.pixelSize: 18; font.weight: Font.Bold }
                        }
                        ColumnLayout {
                            Layout.fillWidth: true; spacing: 3
                            Label { text: "Swap history"; color: "#F1F3F6"; font.pixelSize: 14; font.weight: Font.DemiBold }
                            Label {
                                text: root.swapCount === 0 ? "Refresh the durable maker database" : root.swapCount + " recorded swaps · latest " + root.currentState
                                color: "#7F8A9B"; font.pixelSize: 11
                            }
                        }
                        Label {
                            text: root.latestSwap === "" ? "DURABLE RECORD" : root.latestSwap
                            color: root.latestSwap === "" ? "#657184" : "#B997FF"
                            font.pixelSize: 10
                            font.weight: Font.DemiBold
                            font.family: root.latestSwap === "" ? "DejaVu Sans" : "DejaVu Sans Mono"
                            elide: Text.ElideMiddle
                            Layout.preferredWidth: 220
                        }
                    }
                }

                Rectangle {
                    objectName: "makerChat"
                    Layout.fillWidth: true
                    implicitHeight: makerChatColumn.implicitHeight + 40
                    radius: 14
                    color: "#101722"
                    border.width: 1
                    border.color: "#3A3152"
                    ColumnLayout {
                        id: makerChatColumn
                        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                        anchors.margins: 20
                        spacing: 10
                        RowLayout {
                            Layout.fillWidth: true
                            ColumnLayout {
                                Layout.fillWidth: true; spacing: 2
                                Label { text: "Private negotiation Chat"; color: "#F1F3F6"; font.pixelSize: 14; font.weight: Font.DemiBold }
                                Label { text: "End-to-end encrypted by Logos Chat; valid only while this Maker app is open"; color: "#7F8A9B"; font.pixelSize: 10 }
                            }
                            Label { text: root.chatState.toUpperCase(); color: "#B997FF"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.8 }
                            LuxeButton {
                                objectName: "makerChatStatus"
                                text: "Refresh Chat"
                                enabled: root.ready && !root.busy
                                onClicked: root.chatStatus()
                            }
                            LuxeButton {
                                objectName: "makerChatReset"
                                text: "Reset"
                                enabled: root.ready && !root.busy
                                onClicked: root.resetChat()
                            }
                        }
                        LuxeField {
                            objectName: "makerChatAddress"
                            text: root.chatAddress
                            placeholderText: "Refresh after Logos Chat reaches online"
                            readOnly: true
                            selectByMouse: true
                            Layout.fillWidth: true
                            font.family: "DejaVu Sans Mono"
                        }
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
                        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; anchors.margins: 16
                        spacing: 10
                        RowLayout {
                            Layout.fillWidth: true
                            ColumnLayout {
                                Layout.fillWidth: true; spacing: 2
                                Label { text: "Technical evidence"; color: "#C7CED9"; font.pixelSize: 12; font.weight: Font.DemiBold }
                                Label { text: "Raw owner-daemon response for audit and debugging"; color: "#657184"; font.pixelSize: 10 }
                            }
                            LuxeButton {
                                text: root.technicalVisible ? "Hide technical details" : "Show technical details"
                                quiet: true
                                onClicked: root.technicalVisible = !root.technicalVisible
                            }
                        }
                        TextArea {
                            objectName: "makerOutput"
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

    Rectangle {
        // In-scene dialog: Popup/Overlay never renders inside Basecamp's
        // embedded plugin view, so the dialog lives in the same scene.
        id: newOfferOverlay
        anchors.fill: parent
        visible: root.newOfferOpen
        z: 1000
        color: "#D0060A12"
        MouseArea { anchors.fill: parent; onClicked: root.newOfferOpen = false }
        Rectangle {
            anchors.centerIn: parent
            width: 470
            implicitHeight: newOfferColumn.implicitHeight + 48
            color: "#101722"; radius: 16
            border.width: 1; border.color: "#8950FA"
            MouseArea { anchors.fill: parent }
            ColumnLayout {
                id: newOfferColumn
                anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                anchors.margins: 24
                spacing: 14
                Label {
                    text: root.sellSide === "lez"
                        ? "Sell 1,000 LEZ for 0.01 BTC"
                        : "Sell 0.01 BTC for 1,000 LEZ"
                    color: "#F5F6F8"; font.pixelSize: 17; font.weight: Font.DemiBold
                }
                Label {
                    text: "Indexed to " + makerWallet.currentText + " until taken or withdrawn."
                    color: "#8793A5"; font.pixelSize: 11
                    wrapMode: Text.WordWrap; Layout.fillWidth: true
                }
                GridLayout {
                    Layout.fillWidth: true; columns: 2; columnSpacing: 10; rowSpacing: 6
                    FieldLabel { text: "YOU SELL" }
                    FieldLabel { text: "YOU RECEIVE" }
                    LuxeField {
                        text: root.sellSide === "lez" ? "1,000 LEZ" : "0.01000000 BTC"
                        readOnly: true; Layout.fillWidth: true
                    }
                    LuxeField {
                        text: root.sellSide === "lez" ? "0.01000000 BTC" : "1,000 LEZ"
                        readOnly: true; Layout.fillWidth: true
                    }
                    FieldLabel { text: "TAKER ROUTE" }
                    FieldLabel { text: "MAKER STEPS" }
                    LuxeField {
                        text: root.sellSide === "lez" ? "BTC → LEZ" : "LEZ → BTC"
                        readOnly: true; Layout.fillWidth: true
                    }
                    LuxeField {
                        text: root.sellSide === "lez" ? "Fund LEZ · Claim BTC" : "Lock BTC · Claim LEZ"
                        readOnly: true; Layout.fillWidth: true
                    }
                }
                Label {
                    text: "One offer per publish"
                    color: "#68768A"; font.pixelSize: 10
                    wrapMode: Text.WordWrap; Layout.fillWidth: true
                }
                RowLayout {
                    Layout.fillWidth: true; spacing: 10
                    Item { Layout.fillWidth: true }
                    LuxeButton {
                        text: "Cancel"; quiet: true
                        onClicked: root.newOfferOpen = false
                    }
                    LuxeButton {
                        objectName: "makerCreateOffers"
                        text: root.btcMarketBusy ? "Publishing…" : "Publish offer"
                        primary: true
                        enabled: root.ready && root.btcMarketReady && !root.btcMarketBusy
                        onClicked: root.createBtcOffers()
                    }
                }
            }
        }
    }
}
