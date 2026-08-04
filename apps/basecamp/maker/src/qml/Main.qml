import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {
    id: root
    readonly property var backend: logos.module("lez_atomic_swap_maker")
    property bool ready: false
    property string output: "No operation submitted"

    Connections {
        target: logos
        function onViewModuleReadyChanged(moduleName, isReady) {
            if (moduleName === "lez_atomic_swap_maker")
                root.ready = isReady && root.backend !== null
        }
    }
    Component.onCompleted: root.ready = root.backend !== null
        && logos.isViewModuleReady("lez_atomic_swap_maker")

    function invoke(operation) {
        if (!root.ready) {
            root.output = "Maker service backend is not ready"
            return
        }
        root.output = "Waiting for owner-local service..."
        logos.watch(operation,
            function(value) { root.output = String(value) },
            function(error) { root.output = "Backend failure: " + String(error) })
    }

    Rectangle {
        anchors.fill: parent
        color: "#10161f"

        ScrollView {
            anchors.fill: parent
            anchors.margins: 20
            contentWidth: availableWidth

            ColumnLayout {
                width: parent.width
                spacing: 12

                Label { text: "LEZ Atomic Swap — Maker Console"; color: "white"; font.pixelSize: 24 }
                Label {
                    objectName: "makerConnection"
                    text: root.ready ? "Backend connected" : "Connecting to process-isolated backend"
                    color: root.ready ? "#58d68d" : "#f5b041"
                }
                Button {
                    text: "Check service"
                    enabled: root.ready
                    onClicked: root.invoke(root.backend.health())
                }

                GroupBox {
                    title: "Configure an exact local route"
                    Layout.fillWidth: true
                    GridLayout {
                        columns: 2
                        anchors.fill: parent
                        Label { text: "Pair" }
                        ComboBox { id: pair; objectName: "makerPair"; model: ["Zcash", "Bitcoin", "Monero"] }
                        Label { text: "Direction" }
                        ComboBox { id: direction; objectName: "makerDirection"; model: ["TakerSellsLez", "TakerSellsForeign"] }
                        Label { text: "Minimum foreign units" }
                        TextField { id: minimum; objectName: "makerForeignUnits"; text: "100000000" }
                        Label { text: "Maximum foreign units" }
                        TextField { id: maximum; text: "100000000" }
                        Label { text: "Offer TTL seconds" }
                        TextField { id: ttl; text: "300" }
                        Label { text: "LEZ units per lot" }
                        TextField { id: lezLot; objectName: "makerLezUnits"; text: "1" }
                        Label { text: "Foreign units per lot" }
                        TextField { id: foreignLot; text: "2" }
                        Button {
                            objectName: "makerSave"
                            text: "Save route atomically"
                            enabled: root.ready
                            Layout.columnSpan: 2
                            onClicked: root.invoke(root.backend.saveRoute(
                                "maker-ui-route-001", pair.currentText, direction.currentText,
                                minimum.text, maximum.text, ttl.text, lezLot.text, foreignLot.text))
                        }
                    }
                }

                GroupBox {
                    objectName: "makerActive"
                    title: "Active swap"
                    Layout.fillWidth: true
                    RowLayout {
                        anchors.fill: parent
                        TextField { id: swapId; placeholderText: "Swap ID"; Layout.fillWidth: true }
                        TextField { id: generation; placeholderText: "Generation"; text: "0" }
                        Button { text: "Monitor"; onClicked: root.invoke(root.backend.monitor(swapId.text)) }
                        Button { text: "Claim"; onClicked: root.invoke(root.backend.claim("maker-ui-claim-001", swapId.text, generation.text)) }
                        Button { text: "Refund"; onClicked: root.invoke(root.backend.refund("maker-ui-refund-001", swapId.text, generation.text)) }
                    }
                }

                Button {
                    objectName: "makerHistory"
                    text: "Refresh swap history"
                    enabled: root.ready
                    onClicked: root.invoke(root.backend.history())
                }
                TextArea {
                    text: root.output
                    readOnly: true
                    wrapMode: Text.WrapAnywhere
                    Layout.fillWidth: true
                    Layout.preferredHeight: 180
                    color: "#d6eaf8"
                    background: Rectangle { color: "#17202a"; radius: 6 }
                }
            }
        }
    }
}
