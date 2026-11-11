import QtQuick
import QtQuick.Controls

// The LEZ network desk: shows what this Basecamp hosts (the LEZ network node,
// the LEZ indexer) as its backend reports it, and rebuilds the indexer's
// database from the node on Re-index while the node keeps running
// (deploy/scripts/repair-indexer.sh presses it).
Item {
    id: root

    readonly property var backend: logos.module("lez_network")
    property string status: "LEZ Network"

    Component.onCompleted: {
        if (root.backend)
            logos.watch(root.backend.status(), function(value) { root.status = String(value) })
    }

    Column {
        anchors.centerIn: parent
        width: parent.width - 32
        spacing: 16

        Label {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
            text: root.status
        }

        Button {
            objectName: "reindexButton"
            anchors.horizontalCenter: parent.horizontalCenter
            text: "Re-index"
            enabled: !!root.backend
            onClicked: {
                root.status = "re-indexing"
                logos.watch(root.backend.reindex(), function(value) { root.status = String(value) })
            }
        }
    }
}
