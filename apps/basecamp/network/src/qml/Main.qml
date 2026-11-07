import QtQuick
import QtQuick.Controls

// The LEZ network desk: shows what this Basecamp hosts (the LEZ network node,
// the LEZ indexer) as its backend reports it.
Item {
    id: root

    readonly property var backend: logos.module("lez_network")
    property string status: "LEZ Network"

    Component.onCompleted: {
        if (root.backend)
            logos.watch(root.backend.status(), function(value) { root.status = String(value) })
    }

    Label {
        anchors.centerIn: parent
        width: parent.width - 32
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.Wrap
        text: root.status
    }
}
