import QtQuick
import QtQuick.Controls

ComboBox {
    id: control
    implicitHeight: 40
    font.pixelSize: 13
    font.weight: Font.Medium
    contentItem: Label {
        text: control.displayText
        color: "#F5F7FA"
        verticalAlignment: Text.AlignVCenter
        leftPadding: 12
        rightPadding: 32
        font: control.font
    }
    indicator: Label {
        x: control.width - width - 12
        y: (control.height - height) / 2 - 1
        text: "⌄"
        color: "#7EE100"
        font.pixelSize: 18
    }
    background: Rectangle {
        radius: 8
        color: "#111925"
        border.width: 1
        border.color: control.activeFocus ? "#8950FA" : "#2A3547"
    }
    delegate: ItemDelegate {
        id: option
        required property int index
        required property var modelData
        width: control.width
        height: 36
        contentItem: Label {
            text: option.modelData
            color: option.highlighted ? "#B997FF" : "#E8ECF2"
            verticalAlignment: Text.AlignVCenter
            leftPadding: 10
            font.pixelSize: 13
        }
        background: Rectangle { color: option.highlighted ? "#222D3D" : "transparent"; radius: 6 }
        highlighted: control.highlightedIndex === option.index
    }
    popup: Popup {
        y: control.height + 4
        width: control.width
        implicitHeight: contentItem.implicitHeight + 8
        padding: 4
        contentItem: ListView {
            clip: true
            implicitHeight: contentHeight
            model: control.popup.visible ? control.delegateModel : null
            currentIndex: control.highlightedIndex
        }
        background: Rectangle { color: "#111925"; radius: 8; border.width: 1; border.color: "#344052" }
    }
}
