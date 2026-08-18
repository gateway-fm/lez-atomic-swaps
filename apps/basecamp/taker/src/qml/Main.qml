import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {
    id: root
    readonly property var backend: logos.module("lez_atomic_swap_taker")
    property bool ready: false
    property string output: "Browse authenticated offers to begin"

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
                        onClicked: root.invoke(root.backend.listOffers(pair.currentText, direction.currentText))
                    }
                    Button { text: "Service health"; onClicked: root.invoke(root.backend.health()) }
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
                            text: "10000"
                        }
                        Label { text: "Expected LEZ atomic units" }
                        TextField {
                            id: lezUnits
                            objectName: "takerLezUnits"
                            placeholderText: "Expected LEZ atomic units"
                            text: "25000"
                        }
                        Button {
                            objectName: "takerInitiate"
                            text: "Confirm and initiate"
                            enabled: root.ready
                            Layout.columnSpan: 2
                            onClicked: root.invoke(root.backend.initiate(
                                "taker-ui-initiate-001", offerId.text, pair.currentText,
                                direction.currentText, makerIdentity.text, envelopeDigest.text,
                                foreignUnits.text, lezUnits.text))
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
