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
    property string statusDetail: "Establishing the owner-local service channel"
    property string selectedOffer: ""
    property string selectedExpiry: ""
    property int availableOffers: 0
    property string currentState: ""
    property string currentSwap: ""
    property bool replayed: false

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

    Connections {
        target: logos
        function onViewModuleReadyChanged(moduleName, isReady) {
            if (moduleName !== "lez_atomic_swap_taker") return
            root.ready = isReady && root.backend !== null
            if (root.ready) {
                root.statusMode = "success"
                root.statusTitle = "Private service connected"
                root.statusDetail = "Ready to discover authenticated offers"
            }
        }
    }

    Component.onCompleted: {
        root.ready = root.backend !== null && logos.isViewModuleReady("lez_atomic_swap_taker")
        if (root.ready) {
            root.statusMode = "success"
            root.statusTitle = "Private service connected"
            root.statusDetail = "Ready to discover authenticated offers"
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
            throw new Error(envelope.message || envelope.code || "The service rejected this request")
        return envelope.result ?? {}
    }

    function run(operation, pendingTitle, onSuccess) {
        if (!root.ready) {
            root.output = "Taker service backend is not ready"
            root.statusMode = "error"
            root.statusTitle = "Service unavailable"
            root.statusDetail = "Wait for the secure local connection and try again"
            return
        }
        root.busy = true
        root.output = "Waiting for owner-local service..."
        root.statusMode = "working"
        root.statusTitle = pendingTitle
        root.statusDetail = "Verifying the request over the owner-only channel"
        logos.watch(operation,
            function(value) {
                root.busy = false
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
                root.output = "Backend failure: " + String(error)
                root.statusMode = "error"
                root.statusTitle = "Secure service error"
                root.statusDetail = String(error)
                root.technicalVisible = true
            })
    }

    function chooseNewest(result) {
        var entries = result.offers ?? []
        root.availableOffers = entries.length
        if (entries.length === 0) {
            root.selectedOffer = ""
            root.statusMode = "neutral"
            root.statusTitle = "No offers available"
            root.statusDetail = "Prepare a local swap or choose another market"
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
        var configuration = offer.pair_configuration ?? {}
        var price = offer.price ?? {}
        offerId.text = String(offer.id ?? "")
        makerIdentity.text = String(selected.maker_identity ?? selected.maker_public_key ?? "")
        envelopeDigest.text = root.digestHex(selected.signed_envelope_sha256)
        foreignUnits.text = String(configuration.minimum_foreign_units ?? "")
        if (Number(price.foreign_units_per_lot ?? 0) > 0) {
            var expected = Number(configuration.minimum_foreign_units)
                * Number(price.lez_units_per_lot) / Number(price.foreign_units_per_lot)
            lezUnits.text = String(expected)
        }
        root.selectedOffer = offerId.text
        root.selectedExpiry = Number(offer.expires_at_unix_seconds ?? 0) > 0
            ? new Date(Number(offer.expires_at_unix_seconds) * 1000).toLocaleTimeString(Qt.locale(), "HH:mm") : "—"
        root.statusMode = "success"
        root.statusTitle = "Newest offer selected"
        root.statusDetail = "Signature verified · exact terms filled automatically"
    }

    function browse() {
        root.run(root.backend.listOffers(pair.currentText, direction.currentText),
            "Scanning authenticated offers", function(result) { root.chooseNewest(result) })
    }

    function health() {
        root.run(root.backend.health(), "Checking service health", function(result) {
            root.statusMode = result.ready === true ? "success" : "error"
            root.statusTitle = result.ready === true ? "All systems ready" : "Service needs attention"
            root.statusDetail = "Offer delivery: " + String(result.delivery ?? "unknown")
        })
    }

    function initiate() {
        root.run(root.backend.initiate(
            "taker-ui-initiate-001", offerId.text, pair.currentText,
            direction.currentText, makerIdentity.text, envelopeDigest.text,
            foreignUnits.text, lezUnits.text), "Securing the swap agreement", function(result) {
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
                                text: "PRIVATE ATOMIC SETTLEMENT"
                                color: "#7EE100"
                                font.pixelSize: 10
                                font.weight: Font.Bold
                                font.letterSpacing: 1.8
                            }
                            Label {
                                text: "LEZ Atomic Swap — Taker Route"
                                color: "#F7F8FA"
                                font.pixelSize: 30
                                font.weight: Font.Bold
                                font.letterSpacing: -0.7
                            }
                            Label {
                                text: "Discover a signed offer, review exact terms, and secure the agreement."
                                color: "#9FA9B9"
                                font.pixelSize: 13
                            }
                        }

                        ColumnLayout {
                            Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
                            spacing: 8
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
                                text: "Owner-local · signed · replay-safe"
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
                            visible: root.currentSwap !== ""
                            enabled: root.ready && !root.busy
                            implicitHeight: 38
                            primary: true
                            onClicked: root.monitor()
                        }
                        LuxeButton {
                            text: "List my swaps"
                            enabled: root.ready && !root.busy
                            implicitHeight: 38
                            onClicked: root.listSwaps()
                        }
                        Label {
                            visible: root.selectedOffer !== ""
                            text: root.availableOffers + (root.availableOffers === 1 ? " VERIFIED OFFER" : " VERIFIED OFFERS")
                            color: "#7EE100"; font.pixelSize: 10; font.weight: Font.Bold; font.letterSpacing: 0.8
                        }
                    }
                }

                GridLayout {
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
                                StepBadge { number: "01"; accent: "#8950FA" }
                                ColumnLayout {
                                    Layout.fillWidth: true; spacing: 2
                                    Label { text: "Choose your market"; color: "#F5F6F8"; font.pixelSize: 17; font.weight: Font.DemiBold }
                                    Label { text: "Only authenticated Maker offers are shown"; color: "#7F8A9B"; font.pixelSize: 11 }
                                }
                            }
                            GridLayout {
                                Layout.fillWidth: true; columns: 2; columnSpacing: 10; rowSpacing: 7
                                FieldLabel { text: "ASSET YOU RECEIVE" }
                                FieldLabel { text: "YOUR SIDE" }
                                LuxeCombo { id: pair; objectName: "takerPair"; model: ["Zcash", "Bitcoin", "Monero"]; Layout.fillWidth: true }
                                LuxeCombo { id: direction; objectName: "takerDirection"; model: ["TakerSellsLez", "TakerSellsForeign"]; Layout.fillWidth: true }
                            }
                            LuxeButton {
                                objectName: "takerOffers"
                                text: "Browse authenticated offers"
                                primary: true
                                enabled: root.ready && !root.busy
                                Layout.fillWidth: true
                                onClicked: root.browse()
                            }
                            LuxeButton {
                                text: "Service health"
                                quiet: true
                                enabled: root.ready && !root.busy
                                Layout.fillWidth: true
                                onClicked: root.health()
                            }
                            Rectangle {
                                Layout.fillWidth: true
                                implicitHeight: 82
                                radius: 11
                                color: root.selectedOffer === "" ? "#0D131C" : "#151C22"
                                border.width: 1
                                border.color: root.selectedOffer === "" ? "#202A39" : "#3C493D"
                                RowLayout {
                                    anchors.fill: parent; anchors.margins: 14; spacing: 12
                                    Rectangle {
                                        implicitWidth: 38; implicitHeight: 38; radius: 10
                                        color: root.selectedOffer === "" ? "#192231" : "#223528"
                                            Label { anchors.centerIn: parent; text: root.selectedOffer === "" ? "—" : "✓"; color: "#7EE100"; font.pixelSize: 16; font.weight: Font.Bold }
                                    }
                                    ColumnLayout {
                                        Layout.fillWidth: true; spacing: 3
                                        Label {
                                            text: root.selectedOffer === "" ? "No offer selected" : "Authenticated offer ready"
                                            color: "#EDEFF3"; font.pixelSize: 12; font.weight: Font.DemiBold
                                        }
                                        Label {
                                            text: root.selectedOffer === "" ? "Browse to select the newest valid quote" : root.selectedOffer
                                            color: "#8C97A8"; font.pixelSize: 10; font.family: "DejaVu Sans Mono"; elide: Text.ElideMiddle; Layout.fillWidth: true
                                        }
                                    }
                                    ColumnLayout {
                                        visible: root.selectedOffer !== ""; spacing: 2
                                        Label { text: "VALID UNTIL"; color: "#687486"; font.pixelSize: 9; font.weight: Font.Bold }
                                        Label { text: root.selectedExpiry; color: "#FA50C1"; font.pixelSize: 12; font.weight: Font.DemiBold }
                                    }
                                }
                            }
                        }
                    }

                    Rectangle {
                        id: reviewPanel
                        objectName: "takerReview"
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
                                    text: "After a transparent ZEC claim, move funds to a shielded wallet address."
                                    color: "#A8AFBB"; font.pixelSize: 11; Layout.fillWidth: true; wrapMode: Text.WordWrap
                                }
                            }
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
                        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
                        anchors.margins: 16
                        spacing: 10
                        RowLayout {
                            Layout.fillWidth: true
                            ColumnLayout {
                                Layout.fillWidth: true; spacing: 2
                                Label { text: "Technical evidence"; color: "#C7CED9"; font.pixelSize: 12; font.weight: Font.DemiBold }
                                Label { text: "Raw owner-service response for audit and debugging"; color: "#657184"; font.pixelSize: 10 }
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
