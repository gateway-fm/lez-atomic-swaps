import QtQuick
import QtQuick.Controls

TextField {
    id: control
    implicitHeight: 40
    color: "#F5F7FA"
    placeholderTextColor: "#707A8B"
    selectionColor: "#8950FA"
    selectedTextColor: "#0C1017"
    font.pixelSize: 13
    leftPadding: 12
    rightPadding: 12
    selectByMouse: true
    background: Rectangle {
        radius: 8
        color: control.readOnly ? "#0E141E" : "#111925"
        border.width: 1
        border.color: control.activeFocus ? "#8950FA" : "#2A3547"
    }
}
