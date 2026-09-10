import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// Two exclusive choices, e.g. which asset the Maker sells.
RowLayout {
    id: toggle
    property string value: ""
    // [[value, label, accent], ...]
    property var options: []
    signal picked(string value)
    spacing: 4
    Repeater {
        model: toggle.options
        delegate: Rectangle {
            id: option
            required property var modelData
            readonly property bool chosen: toggle.value === option.modelData[0]
            implicitWidth: optionLabel.implicitWidth + 22; implicitHeight: 28; radius: 7
            color: option.chosen ? "#1E1830" : "transparent"
            border.width: 1; border.color: option.chosen ? option.modelData[2] : "#2B3446"
            Label {
                id: optionLabel
                anchors.centerIn: parent
                text: option.modelData[1]
                color: option.chosen ? option.modelData[2] : "#8B96A8"
                font.pixelSize: 10; font.weight: Font.Bold; font.letterSpacing: 0.8
            }
            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: toggle.picked(option.modelData[0]) }
        }
    }
}
