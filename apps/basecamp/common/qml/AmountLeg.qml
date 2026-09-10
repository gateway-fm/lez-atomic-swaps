import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// One leg of an offer form: a label, an editable amount, the asset.
Rectangle {
    id: leg
    property string label: "YOU SELL"
    property string asset: "LEZ"
    property alias amount: amountInput.text
    property color accent: "#7EE100"
    property string note: ""
    implicitHeight: 62
    radius: 10
    color: "#0E1520"
    border.width: 1
    border.color: amountInput.activeFocus ? "#8950FA" : "#26334A"
    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 14
        anchors.rightMargin: 14
        spacing: 12
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 1
            Label { text: leg.label; color: "#77839A"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 1.1 }
            Label {
                visible: leg.note !== ""
                text: leg.note
                color: "#5F6E85"; font.pixelSize: 9
                elide: Text.ElideMiddle; Layout.fillWidth: true
            }
        }
        TextField {
            id: amountInput
            Layout.preferredWidth: 190
            horizontalAlignment: Text.AlignRight
            placeholderText: leg.asset === "BTC" ? "0.00000000" : "0"
            placeholderTextColor: "#4B586B"
            color: "#F2F5F9"
            selectionColor: "#8950FA"; selectedTextColor: "#0C1017"
            font.pixelSize: 21; font.weight: Font.DemiBold
            selectByMouse: true
            background: Item {}
        }
        Label {
            text: leg.asset
            color: leg.accent
            font.pixelSize: 13; font.weight: Font.Bold
            Layout.rightMargin: 2
        }
    }
}
