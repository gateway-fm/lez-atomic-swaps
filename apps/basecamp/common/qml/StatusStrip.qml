import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// The last request's outcome, and the requests every desk can always make.
Rectangle {
    id: strip
    property string mode: "neutral"
    property string title: ""
    property string detail: ""
    default property alias actions: actionRow.data
    implicitHeight: 64
    radius: 12
    color: strip.mode === "error" ? "#21151A" : strip.mode === "success" ? "#102019" : strip.mode === "working" ? "#1C1628" : "#11131A"
    border.width: 1
    border.color: strip.mode === "error" ? "#61323E" : strip.mode === "success" ? "#285540" : strip.mode === "working" ? "#6742A0" : "#2A2E3A"
    RowLayout {
        anchors.fill: parent; anchors.margins: 14; spacing: 12
        Label {
            text: strip.mode === "error" ? "!" : strip.mode === "success" ? "✓" : strip.mode === "working" ? "···" : "i"
            color: strip.mode === "error" ? "#FF9FAF" : strip.mode === "success" ? "#7EE100" : strip.mode === "working" ? "#B997FF" : "#A8B5C7"
            font.pixelSize: 14; font.weight: Font.Bold
            Layout.preferredWidth: 18
        }
        ColumnLayout {
            Layout.fillWidth: true; spacing: 2
            Label { text: strip.title; color: "#F2F4F7"; font.pixelSize: 13; font.weight: Font.DemiBold }
            Label { text: strip.detail; color: "#929DAD"; font.pixelSize: 11; elide: Text.ElideRight; Layout.fillWidth: true }
        }
        RowLayout { id: actionRow; spacing: 6 }
    }
}
