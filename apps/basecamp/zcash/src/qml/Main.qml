import QtQuick
import QtQuick.Controls

// The Zcash Node desk: shows the Zcash node (Zebra, zebrad_module) this
// Basecamp hosts as its backend reports it, refreshed every five seconds. On
// Regtest, Reset chain stops the node, deletes its chain and starts a new one
// (tests/fixtures/zec-catalog-fixture.sh presses it for a worn chain or one on
// another consensus branch).
Item {
    id: root

    readonly property var backend: logos.module("lez_zcash_node")
    property string status: "Zcash Node"

    function refresh() {
        if (root.backend)
            logos.watch(root.backend.status(), function(value) { root.status = String(value) })
    }

    Component.onCompleted: refresh()

    Timer {
        interval: 5000
        repeat: true
        running: !!root.backend
        onTriggered: root.refresh()
    }

    Column {
        anchors.centerIn: parent
        width: parent.width - 32
        spacing: 16

        Label {
            objectName: "zcashNodeStatus"
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
            text: root.status
        }

        Button {
            objectName: "resetChainButton"
            anchors.horizontalCenter: parent.horizontalCenter
            text: "Reset chain"
            enabled: !!root.backend
            onClicked: {
                root.status = "resetting the chain"
                logos.watch(root.backend.resetChain(), function(value) { root.status = String(value) })
            }
        }
    }
}
