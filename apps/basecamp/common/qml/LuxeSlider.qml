import QtQuick
import QtQuick.Controls

// A slider over whole units that snaps to `stepSize`.
Slider {
    id: control
    implicitHeight: 24
    snapMode: Slider.SnapAlways
    background: Rectangle {
        x: control.leftPadding; y: control.topPadding + control.availableHeight / 2 - height / 2
        width: control.availableWidth; height: 4; radius: 2
        color: "#252E3C"
        Rectangle { width: control.visualPosition * parent.width; height: parent.height; radius: 2; color: "#8950FA" }
    }
    handle: Rectangle {
        x: control.leftPadding + control.visualPosition * (control.availableWidth - width)
        y: control.topPadding + control.availableHeight / 2 - height / 2
        width: 14; height: 14; radius: 7
        color: control.pressed ? "#B997FF" : "#F1F3F6"
        border.width: 1; border.color: "#8950FA"
    }
}
