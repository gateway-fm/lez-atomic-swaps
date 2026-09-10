import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Rectangle {
    id: tab
    property string label: ""
    property int count: 0
    property bool active: false
    property bool alert: false
    signal picked()
    implicitWidth: tabRow.implicitWidth + 22
    implicitHeight: 30
    radius: 7
    color: tab.active ? "#1E1830" : tabArea.containsMouse ? "#151C29" : "transparent"
    border.width: 1
    border.color: tab.alert ? "#FA50C1" : tab.active ? "#8950FA" : "#2B3446"
    RowLayout {
        id: tabRow
        anchors.centerIn: parent
        spacing: 7
        Label {
            text: tab.label
            color: tab.alert ? "#FFB8EC" : tab.active ? "#D8C6FF" : "#8B96A8"
            font.pixelSize: 10; font.weight: Font.Bold; font.letterSpacing: 0.8
        }
        Label {
            text: String(tab.count)
            color: tab.alert ? "#FA50C1" : tab.active ? "#B997FF" : "#6D7889"
            font.pixelSize: 10; font.weight: Font.Bold; font.family: "DejaVu Sans Mono"
        }
    }
    MouseArea {
        id: tabArea
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: tab.picked()
    }
}
