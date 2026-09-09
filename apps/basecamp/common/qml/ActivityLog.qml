import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// What the desk asked, what the Node answered, and what the poll saw
// change, newest last. The desk appends the raw last reply underneath.
Panel {
    id: log
    property var entries: []
    signal copyRequested(string text)
    signal clearRequested()
    RowLayout {
        Layout.fillWidth: true; spacing: 8
        SectionTitle { text: "Activity"; Layout.fillWidth: true }
        LuxeButton {
            text: "Copy"; quiet: true; enabled: log.entries.length > 0
            onClicked: log.copyRequested(log.entries.map(function(e) { return e.time + "  " + e.kind + "  " + e.text }).join("\n"))
        }
        LuxeButton { text: "Clear"; quiet: true; enabled: log.entries.length > 0; onClicked: log.clearRequested() }
    }
    Rectangle {
        Layout.fillWidth: true
        Layout.preferredHeight: 420
        radius: 8
        color: "#080C12"; border.width: 1; border.color: "#253043"
        ListView {
            id: list
            anchors.fill: parent; anchors.margins: 8
            clip: true
            model: log.entries
            spacing: 3
            onCountChanged: positionViewAtEnd()
            delegate: RowLayout {
                id: entry
                required property var modelData
                width: list.width
                spacing: 8
                Label { text: entry.modelData.time; color: "#5F6B7D"; font.pixelSize: 10; font.family: "DejaVu Sans Mono" }
                Label {
                    text: entry.modelData.kind
                    color: entry.modelData.kind === "error" ? "#FF9FAF"
                        : entry.modelData.kind === "action" ? "#FA50C1"
                        : entry.modelData.kind === "reply" ? "#7EE100"
                        : entry.modelData.kind === "swap" || entry.modelData.kind === "offers" ? "#B997FF" : "#8B96A8"
                    font.pixelSize: 10; font.family: "DejaVu Sans Mono"
                    Layout.preferredWidth: 52
                }
                Label {
                    text: entry.modelData.text
                    color: "#C7CED9"; font.pixelSize: 10; font.family: "DejaVu Sans Mono"
                    wrapMode: Text.WrapAnywhere; Layout.fillWidth: true
                }
            }
        }
    }
}
