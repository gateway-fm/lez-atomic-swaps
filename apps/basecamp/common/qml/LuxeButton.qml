import QtQuick
import QtQuick.Controls

// The one button. `primary` for the single most important action on a
// panel, `quiet` for requests, `destructive` for withdrawals and refunds.
Button {
    id: control
    property bool primary: false
    property bool quiet: false
    property bool destructive: false
    hoverEnabled: true
    implicitHeight: 40
    leftPadding: 16
    rightPadding: 16
    font.pixelSize: 13
    font.weight: Font.DemiBold
    contentItem: Label {
        text: control.text
        color: !control.enabled ? "#6F7787"
            : control.primary ? "#FFFFFF"
            : control.destructive ? "#FF9BE0" : "#F3F5F8"
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        font: control.font
    }
    background: Rectangle {
        radius: 9
        color: !control.enabled ? "#181E29"
            : control.primary ? (control.down ? "#7139DA" : control.hovered ? "#9D72FF" : "#8950FA")
            : control.destructive ? (control.hovered ? "#3B1C34" : "#251621")
            : control.quiet ? (control.hovered ? "#1B2432" : "transparent")
            : (control.down ? "#202938" : control.hovered ? "#252F40" : "#1B2330")
        border.width: control.primary || control.quiet ? 0 : 1
        border.color: control.destructive ? "#8F3A77" : "#344052"
    }
}
