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
    property var btcMarket: ({
        inventory: [], swaps: [], wallets: [],
        summary: ({pending_offers: 0, accepted_swaps: 0, completed_swaps: 0}),
        runner_ready: false, runner_busy: false,
        runner_detail: "Checking the local M3 runner",
        latest_balance_evidence: null
    })
    property bool btcMarketReady: false
    property bool btcMarketBusy: false
    property string marketTab: "attention"

    // One unified activity list: publishable offers plus every swap this
    // wallet owns. Offers that were taken live on as their swap row, so only
    // pending and withdrawn offers appear as offer rows.
    function marketBucket(item) {
        if (item.kind === "offer")
            return item.state === "pending" ? "open" : "done"
        if (item.state === "completed" || item.state === "failed") return "done"
        if (item.can_act === true) return "attention"
        return "running"
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
        var rows = root.marketRows()
        if (root.marketTab === "all") return rows
        return rows.filter(function(item) { return root.marketBucket(item) === root.marketTab })
    }
    function marketCount(tab) {
        return root.marketRows().filter(function(item) {
            return root.marketBucket(item) === tab
        }).length
    }

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
        var requestId = "ui-maker-create-offers-" + String(Date.now())
        root.run(root.backend.btcCreateOffers(requestId, root.selectedMakerWallet(),
            "1", "1000000", "1000"),
            "Publishing BTC / LEZ inventory", function(result) {
                root.applyBtcMarket(result)
                root.marketTab = "open"
                newOfferPopup.close()
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
                        anchors.fill: parent; anchors.margins: 24; spacing: 24
                        ColumnLayout {
                            Layout.fillWidth: true; spacing: 7
                            Label {
                                text: "INSTITUTIONAL LIQUIDITY DESK"
                                color: "#7EE100"; font.pixelSize: 10; font.weight: Font.Bold; font.letterSpacing: 1.8
                            }
                            Label {
                                text: "LEZ / BTC — Maker Desk"
                                color: "#F7F8FA"; font.pixelSize: 30; font.weight: Font.Bold; font.letterSpacing: -0.7
                            }
                            Label {
                                text: "Publish wallet-owned inventory and authorize only the Maker side of each atomic swap."
                                color: "#9FA9B9"; font.pixelSize: 13
                            }
                        }
                        ColumnLayout {
                            Layout.alignment: Qt.AlignRight | Qt.AlignVCenter; spacing: 8
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

                Rectangle {
                    id: makerMarketPanel
                    objectName: "makerBtcMarket"
                    Layout.fillWidth: true
                    implicitHeight: makerMarketColumn.implicitHeight + 44
                    radius: 16; color: "#101722"; border.width: 1
                    border.color: root.btcMarketReady ? "#38465A" : "#5C3341"

                    ColumnLayout {
                        id: makerMarketColumn
                        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                        anchors.margins: 22; spacing: 16

                        RowLayout {
                            Layout.fillWidth: true; spacing: 12
                            StepBadge { number: "01"; accent: "#7EE100" }
                            ColumnLayout {
                                Layout.fillWidth: true; spacing: 2
                                Label { text: "Choose the Maker wallet"; color: "#F5F6F8"; font.pixelSize: 19; font.weight: Font.DemiBold }
                                Label { text: "Each vault keeps its own pending offers and sees only the swaps it must settle."; color: "#8793A5"; font.pixelSize: 11 }
                            }
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

                        GridLayout {
                            Layout.fillWidth: true; columns: 2; columnSpacing: 10; rowSpacing: 6
                            FieldLabel { text: "MAKER WALLET" }
                            Item { implicitWidth: 170; implicitHeight: 1 }
                            LuxeCombo {
                                id: makerWallet
                                objectName: "makerBtcWallet"
                                model: ["Munich Vault 01 · Maker", "Basel Vault 02 · Maker"]
                                Layout.fillWidth: true
                                onActivated: root.refreshBtcMarket(false)
                            }
                            LuxeButton {
                                objectName: "makerNewOffer"
                                text: "New offer"
                                primary: true; Layout.preferredWidth: 170
                                enabled: root.ready && root.btcMarketReady
                                onClicked: newOfferPopup.open()
                            }
                        }

                        Popup {
                            id: newOfferPopup
                            parent: Overlay.overlay
                            x: Math.round((parent.width - width) / 2)
                            y: Math.round((parent.height - height) / 2)
                            width: 430
                            modal: false
                            dim: true
                            padding: 24
                            closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutside
                            background: Rectangle {
                                color: "#101722"; radius: 16
                                border.width: 1; border.color: "#8950FA"
                            }
                            contentItem: ColumnLayout {
                                spacing: 14
                                Label {
                                    text: "Publish a new offer"
                                    color: "#F5F6F8"; font.pixelSize: 17; font.weight: Font.DemiBold
                                }
                                Label {
                                    text: "Indexed to " + makerWallet.currentText + " until taken or withdrawn."
                                    color: "#8793A5"; font.pixelSize: 11
                                    wrapMode: Text.WordWrap; Layout.fillWidth: true
                                }
                                GridLayout {
                                    Layout.fillWidth: true; columns: 2; columnSpacing: 10; rowSpacing: 6
                                    FieldLabel { text: "TAKER PAYS" }
                                    FieldLabel { text: "MAKER FUNDS" }
                                    LuxeField { text: "0.01000000 BTC"; readOnly: true; Layout.fillWidth: true }
                                    LuxeField { text: "1,000 LEZ"; readOnly: true; Layout.fillWidth: true }
                                }
                                Label {
                                    text: "Fixed M3 preset · direction BTC → LEZ · one offer per publish"
                                    color: "#68768A"; font.pixelSize: 10
                                }
                                RowLayout {
                                    Layout.fillWidth: true; spacing: 10
                                    Item { Layout.fillWidth: true }
                                    LuxeButton {
                                        text: "Cancel"; quiet: true
                                        onClicked: newOfferPopup.close()
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

                        RowLayout {
                            Layout.fillWidth: true; spacing: 12
                            StepBadge { number: "02"; accent: "#FA50C1" }
                            ColumnLayout {
                                Layout.fillWidth: true; spacing: 2
                                Label { text: "This wallet's market"; color: "#F5F6F8"; font.pixelSize: 17; font.weight: Font.DemiBold }
                                Label {
                                    text: Number((root.btcMarket.inventory ?? []).filter(function(item) { return item.state === "pending" }).length)
                                        + " open offers in " + makerWallet.currentText
                                        + " · you control only Fund LEZ and Claim Bitcoin"
                                    color: "#7F8A9B"; font.pixelSize: 11
                                }
                            }
                        }

                        RowLayout {
                            spacing: 7
                            FilterTab {
                                objectName: "makerMarketTabAttention"
                                label: "NEEDS YOU"; count: root.marketCount("attention")
                                alert: root.marketCount("attention") > 0
                                active: root.marketTab === "attention"; onPicked: root.marketTab = "attention"
                            }
                            FilterTab {
                                objectName: "makerMarketTabOpen"
                                label: "OPEN OFFERS"; count: root.marketCount("open")
                                active: root.marketTab === "open"; onPicked: root.marketTab = "open"
                            }
                            FilterTab {
                                label: "RUNNING"; count: root.marketCount("running")
                                active: root.marketTab === "running"; onPicked: root.marketTab = "running"
                            }
                            FilterTab {
                                label: "DONE"; count: root.marketCount("done")
                                active: root.marketTab === "done"; onPicked: root.marketTab = "done"
                            }
                            FilterTab {
                                label: "ALL"; count: root.marketRows().length
                                active: root.marketTab === "all"; onPicked: root.marketTab = "all"
                            }
                        }

                        Label {
                            visible: root.filteredMarketRows().length === 0
                            text: !root.btcMarketReady ? "Loading wallet market…"
                                : root.marketRows().length === 0 ? "No offers or swaps belong to this wallet yet."
                                : root.marketTab === "attention" ? "Nothing needs you right now — check RUNNING or OPEN OFFERS."
                                : "Nothing under this tab for " + makerWallet.currentText + "."
                            color: "#7F8A9B"; font.pixelSize: 12
                        }

                        Repeater {
                            model: root.filteredMarketRows().filter(function(row) { return row.kind === "offer" })
                            delegate: Rectangle {
                                id: makerOfferRow
                                required property var modelData
                                Layout.fillWidth: true; implicitHeight: 68; radius: 10
                                color: "#0D141E"; border.width: 1
                                border.color: makerOfferRow.modelData.state === "pending" ? "#36465B" : "#252E3C"
                                RowLayout {
                                    anchors.fill: parent; anchors.margins: 13; spacing: 14
                                    Rectangle {
                                        implicitWidth: 10; implicitHeight: 38; radius: 2
                                        color: makerOfferRow.modelData.state === "pending" ? "#7EE100"
                                            : makerOfferRow.modelData.state === "completed" ? "#8950FA" : "#4A5362"
                                    }
                                    ColumnLayout {
                                        Layout.fillWidth: true; spacing: 2
                                        Label { text: String(makerOfferRow.modelData.offer_id); color: "#E9EDF3"; font.pixelSize: 10; font.family: "DejaVu Sans Mono"; elide: Text.ElideMiddle; Layout.fillWidth: true }
                                        Label { text: String(makerOfferRow.modelData.state).toUpperCase(); color: "#718095"; font.pixelSize: 8; font.weight: Font.Bold; font.letterSpacing: 0.8 }
                                    }
                                    Label { text: "0.01000000 BTC → 1,000 LEZ"; color: "#AAB4C3"; font.pixelSize: 11; font.weight: Font.DemiBold }
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
                                Layout.fillWidth: true; radius: 10
                                implicitHeight: makerSwapRow.modelData.progress_detail ? 104 : 86
                                color: makerSwapRow.modelData.can_act === true ? "#17152A" : "#0D141E"
                                border.width: 1; border.color: makerSwapRow.modelData.can_act === true ? "#8950FA" : "#28364A"
                                RowLayout {
                                    anchors.fill: parent; anchors.margins: 13; spacing: 14
                                    ColumnLayout {
                                        Layout.fillWidth: true; spacing: 3
                                        Label {
                                            text: String(makerSwapRow.modelData.taker_wallet_label) + " · " + String(makerSwapRow.modelData.state_label)
                                            color: "#F1F3F6"; font.pixelSize: 12; font.weight: Font.DemiBold
                                            elide: Text.ElideRight; Layout.fillWidth: true
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
                                    }
                                    Label {
                                        visible: makerSwapRow.modelData.can_act !== true
                                        text: makerSwapRow.modelData.action_role === "taker" ? "WAITING FOR TAKER" : String(makerSwapRow.modelData.state).toUpperCase()
                                        color: "#7B8798"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.7
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
                                    Label { text: "→"; color: "#5F6B7D" }
                                    Label { text: root.btcMarket.latest_balance_evidence ? root.formatBtcSats(root.btcMarket.latest_balance_evidence.wallet.balances.bitcoin.closing) : ""; color: "#F0F3F7"; font.pixelSize: 10; font.weight: Font.DemiBold }
                                    Item { Layout.fillWidth: true }
                                    Label { text: root.btcMarket.latest_balance_evidence ? root.formatSignedBtc(root.btcMarket.latest_balance_evidence.wallet.balances.bitcoin.net_change) : ""; color: "#7EE100"; font.pixelSize: 10; font.weight: Font.Bold }
                                }
                                RowLayout {
                                    visible: root.btcMarket.latest_balance_evidence !== null
                                    Layout.fillWidth: true
                                    Label { text: "LEZ"; color: "#7EE100"; font.pixelSize: 10; font.weight: Font.Bold; Layout.preferredWidth: 38 }
                                    Label { text: root.btcMarket.latest_balance_evidence ? root.formatLez(root.btcMarket.latest_balance_evidence.wallet.balances.lez.opening) : ""; color: "#9CA7B7"; font.pixelSize: 10 }
                                    Label { text: "→"; color: "#5F6B7D" }
                                    Label { text: root.btcMarket.latest_balance_evidence ? root.formatLez(root.btcMarket.latest_balance_evidence.wallet.balances.lez.closing) : ""; color: "#F0F3F7"; font.pixelSize: 10; font.weight: Font.DemiBold }
                                    Item { Layout.fillWidth: true }
                                    Label { text: root.btcMarket.latest_balance_evidence ? root.formatSignedLez(root.btcMarket.latest_balance_evidence.wallet.balances.lez.net_change) : ""; color: "#FA50C1"; font.pixelSize: 10; font.weight: Font.Bold }
                                }
                            }
                        }
                    }
                }

                Label {
                    text: "ADVANCED SERVICE CONTROLS · PREPARED NON-BITCOIN ROUTES"
                    color: "#566377"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 1.1
                    Layout.topMargin: 4
                }

                GridLayout {
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
                            anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; anchors.margins: 22
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
                            anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; anchors.margins: 22
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
}
