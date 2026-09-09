import QtQuick
import QtQuick.Layouts

// A card: one bordered surface with a vertical stack inside. Every panel on
// both desks is one of these.
Rectangle {
    id: panel
    default property alias content: column.data
    property alias spacing: column.spacing
    implicitHeight: column.implicitHeight + 36
    radius: 12
    color: "#101722"
    border.width: 1
    border.color: "#263144"
    ColumnLayout {
        id: column
        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
        anchors.margins: 18
        spacing: 12
    }
}
