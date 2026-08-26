import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {
    id: root
    readonly property var backend: logos.module("lez_atomic_swap_taker")
    property bool ready: false
    property string output: "Browse authenticated offers to begin"
    property string selectedAnnouncementBase64: ""

    Connections {
        target: logos
        function onViewModuleReadyChanged(moduleName, isReady) {
            if (moduleName === "lez_atomic_swap_taker")
                root.ready = isReady && root.backend !== null
        }
    }
    Component.onCompleted: root.ready = root.backend !== null
        && logos.isViewModuleReady("lez_atomic_swap_taker")

    function invoke(operation) {
        if (!root.ready) {
            root.output = "Taker service backend is not ready"
            return
        }
        root.output = "Waiting for owner-local service..."
        logos.watch(operation,
            function(value) { root.output = String(value) },
            function(error) { root.output = "Backend failure: " + String(error) })
    }

    function browseOffers() {
        if (!root.ready) return
        root.output = "Waiting for signed Delivery broadcasts..."
        logos.watch(root.backend.listOffers(pair.currentText, direction.currentText),
            function(value) {
                var envelope
                try { envelope = JSON.parse(String(value)) }
                catch (error) { root.output = "Offer index returned invalid JSON"; return }
                if (envelope.ok !== true) {
                    root.selectedAnnouncementBase64 = ""
                    root.output = "Offer index unavailable: " + String(envelope.code ?? "gateway_error")
                    return
                }
                var offers = envelope.result.offers ?? []
                if (offers.length === 0) {
                    root.selectedAnnouncementBase64 = ""
                    var unavailable = Number(envelope.result.unavailable_offers ?? 0)
                        + Number(envelope.result.locally_contended_offers ?? 0)
                    root.output = unavailable > 0
                        ? String(unavailable) + " offer(s) are already negotiating, taken, or unavailable"
                        : "No live signed offers; Maker rebroadcasts repair missed messages"
                    return
                }
                var selected = offers[0]
                var offer = selected.offer
                root.selectedAnnouncementBase64 = String(selected.announcement_base64 ?? "")
                offerId.text = String(offer.id)
                makerIdentity.text = String(selected.maker_identity)
                envelopeDigest.text = (selected.signed_envelope_sha256 ?? []).map(function(byte) {
                    return Number(byte).toString(16).padStart(2, "0")
                }).join("")
                foreignUnits.text = String(offer.pair_configuration.minimum_foreign_units)
                var price = offer.price
                var numerator = Number(foreignUnits.text) * Number(price.lez_units_per_lot)
                var divisor = Number(price.foreign_units_per_lot)
                lezUnits.text = Number.isSafeInteger(numerator) && Number.isSafeInteger(divisor)
                    && divisor > 0 && numerator % divisor === 0
                    && Number.isSafeInteger(numerator / divisor)
                    ? String(numerator / divisor) : ""
                root.output = "Authenticated live offer selected; connecting its signed Chat address"
                logos.watch(root.backend.connectOffer(makerIdentity.text, offerId.text),
                    function(connection) { root.output = String(connection) },
                    function(error) { root.output = "Chat connection failed: " + String(error) })
            },
            function(error) { root.output = "Offer index failure: " + String(error) })
    }

    Rectangle {
        anchors.fill: parent
        color: "#111827"
        ScrollView {
            anchors.fill: parent
            anchors.margins: 20
            contentWidth: availableWidth
            ColumnLayout {
                width: parent.width
                spacing: 12
                Label { text: "LEZ Atomic Swap — Taker Route"; color: "white"; font.pixelSize: 24 }
                Label {
                    objectName: "takerConnection"
                    text: root.ready ? "Backend connected" : "Connecting to process-isolated backend"
                    color: root.ready ? "#58d68d" : "#f5b041"
                }
                RowLayout {
                    ComboBox { id: pair; model: ["Zcash", "Bitcoin", "Monero"] }
                    ComboBox { id: direction; model: ["TakerSellsLez", "TakerSellsForeign"] }
                    Button {
                        objectName: "takerOffers"
                        text: "Browse authenticated offers"
                        enabled: root.ready
                        onClicked: root.browseOffers()
                    }
                    Button { text: "Service health"; onClicked: root.invoke(root.backend.health()) }
                }

                GroupBox {
                    objectName: "takerChat"
                    title: "Private Maker Chat session"
                    Layout.fillWidth: true
                    RowLayout {
                        anchors.fill: parent
                        TextField {
                            id: makerChatAddress
                            objectName: "takerChatAddress"
                            placeholderText: "Maker Chat address (valid while Maker app is open)"
                            Layout.fillWidth: true
                        }
                        Button {
                            objectName: "takerChatConnect"
                            text: "Connect Chat"
                            enabled: root.ready && makerChatAddress.text.length > 0
                            onClicked: root.invoke(root.backend.connectChat(makerChatAddress.text))
                        }
                        Button {
                            objectName: "takerChatStatus"
                            text: "Chat status"
                            enabled: root.ready
                            onClicked: root.invoke(root.backend.chatStatus())
                        }
                        Button {
                            objectName: "takerChatReset"
                            text: "Reset Chat"
                            enabled: root.ready
                            onClicked: root.invoke(root.backend.resetChat())
                        }
                    }
                }

                GroupBox {
                    objectName: "takerReview"
                    title: "Review exact public offer facts"
                    Layout.fillWidth: true
                    GridLayout {
                        columns: 2
                        anchors.fill: parent
                        Label { text: "Offer ID" }
                        TextField {
                            id: offerId
                            objectName: "takerOfferId"
                            placeholderText: "Authenticated offer ID"
                            Layout.fillWidth: true
                        }
                        Label { text: "Maker public identity" }
                        TextField {
                            id: makerIdentity
                            objectName: "takerMakerIdentity"
                            placeholderText: "Compressed Maker public key"
                            Layout.fillWidth: true
                        }
                        Label { text: "Signed-envelope SHA-256" }
                        TextField {
                            id: envelopeDigest
                            objectName: "takerEnvelopeDigest"
                            placeholderText: "Signed-envelope digest"
                            Layout.fillWidth: true
                        }
                        Label { text: "Foreign atomic units" }
                        TextField {
                            id: foreignUnits
                            objectName: "takerForeignUnits"
                            placeholderText: "Foreign atomic units"
                            text: "100000000"
                        }
                        Label { text: "Expected LEZ atomic units" }
                        TextField {
                            id: lezUnits
                            objectName: "takerLezUnits"
                            placeholderText: "Expected LEZ atomic units"
                            text: "50000"
                        }
                        Button {
                            objectName: "takerInitiate"
                            text: "Confirm and initiate"
                            enabled: root.ready
                            Layout.columnSpan: 2
                            onClicked: root.invoke(root.backend.initiate(
                                "taker-ui-initiate-" + envelopeDigest.text.slice(0, 32),
                                offerId.text, pair.currentText,
                                direction.currentText, makerIdentity.text, envelopeDigest.text,
                                foreignUnits.text, lezUnits.text,
                                root.selectedAnnouncementBase64))
                        }
                    }
                }

                GroupBox {
                    objectName: "takerProgress"
                    title: "Swap progress and fenced terminal action"
                    Layout.fillWidth: true
                    RowLayout {
                        anchors.fill: parent
                        TextField { id: swapId; objectName: "takerSwapId"; placeholderText: "Swap ID"; Layout.fillWidth: true }
                        TextField { id: generation; placeholderText: "Generation"; text: "0" }
                        Button { objectName: "takerMonitor"; text: "Monitor"; onClicked: root.invoke(root.backend.monitor(swapId.text)) }
                        Button { objectName: "takerClaim"; text: "Claim"; onClicked: root.invoke(root.backend.claim("taker-ui-claim-001", swapId.text, generation.text)) }
                        Button { objectName: "takerRefund"; text: "Refund"; onClicked: root.invoke(root.backend.refund("taker-ui-refund-001", swapId.text, generation.text)) }
                    }
                }

                RowLayout {
                    Button { objectName: "takerListSwaps"; text: "List my swaps"; onClicked: root.invoke(root.backend.listSwaps()) }
                    Label {
                        objectName: "takerShielding"
                        text: "Privacy reminder: shield claimed transparent ZEC in your wallet"
                        color: "#f9e79f"
                    }
                }
                TextArea {
                    objectName: "takerOutput"
                    text: root.output
                    readOnly: true
                    wrapMode: Text.WrapAnywhere
                    Layout.fillWidth: true
                    Layout.preferredHeight: 180
                    color: "#d6eaf8"
                    background: Rectangle { color: "#1f2937"; radius: 6 }
                }
            }
        }
    }
}
