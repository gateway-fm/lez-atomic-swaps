import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// One row per swap: the state the Node reports, how far along it is, and
// the one action this desk may take on it, when there is one.
Rectangle {
    id: swapRow
    required property var modelData
    property string counterpartyLabel: ""
    property string role: ""
    property bool actionEnabled: true
    property string actionObjectName: ""
    // A section label drawn above this row, e.g. "DONE" on the first done row.
    property string divider: ""
    // Unix seconds, for the schedule's countdowns.
    property real now: 0
    readonly property bool done: ["completed", "refunded", "failed"].indexOf(String(swapRow.modelData.state)) >= 0
    signal act()
    implicitHeight: swapColumn.implicitHeight + 26 + (swapRow.divider !== "" ? 22 : 0)
    radius: 9
    color: swapRow.modelData.can_act === true ? "#17152A" : "#0D141E"
    border.width: 1
    border.color: swapRow.modelData.can_act === true ? "#8950FA" : "#28364A"
    Label {
        visible: swapRow.divider !== ""
        anchors.left: parent.left; anchors.top: parent.top; anchors.margins: 8
        text: swapRow.divider
        color: "#5F6B7D"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 1.0
    }
    RowLayout {
        anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top
        anchors.margins: 13; anchors.topMargin: 13 + (swapRow.divider !== "" ? 22 : 0); spacing: 14
        ColumnLayout {
            id: swapColumn
            Layout.fillWidth: true; spacing: 4
            Label {
                text: String(swapRow.modelData.state_label) + " · " + String(swapRow.modelData.direction_display)
                color: "#F1F3F6"; font.pixelSize: 12; font.weight: Font.DemiBold
                elide: Text.ElideRight; Layout.fillWidth: true
            }
            Label {
                text: String(swapRow.modelData.ui_swap_id) + "  ·  " + String(swapRow.modelData.offer_id)
                color: "#68768A"; font.pixelSize: 9; font.family: "DejaVu Sans Mono"
                elide: Text.ElideMiddle; Layout.fillWidth: true
            }
            RowLayout {
                Layout.fillWidth: true; spacing: 8
                Rectangle {
                    Layout.fillWidth: true; implicitHeight: 4; radius: 2; color: "#252E3C"
                    Rectangle {
                        width: parent.width * Number(swapRow.modelData.progress_percent ?? 0) / 100
                        height: parent.height; radius: 2
                        color: swapRow.modelData.state === "completed" ? "#7EE100" : "#8950FA"
                    }
                }
                Label {
                    text: String(swapRow.modelData.progress_percent ?? 0) + "%"
                    color: "#9AA6B8"; font.pixelSize: 9; font.family: "DejaVu Sans Mono"
                }
            }
            Label {
                visible: !!swapRow.modelData.progress_detail
                text: String(swapRow.modelData.progress_detail ?? "")
                color: "#8E7BC6"; font.pixelSize: 10
                wrapMode: Text.WordWrap; Layout.fillWidth: true
            }
            Label {
                visible: !!swapRow.modelData.amounts_display
                text: String(swapRow.modelData.amounts_display ?? "")
                    + (swapRow.modelData.fill_display ? "  ·  " + String(swapRow.modelData.fill_display) : "")
                color: "#9AA6B8"; font.pixelSize: 10; font.family: "DejaVu Sans Mono"
                elide: Text.ElideRight; Layout.fillWidth: true
            }
            Timeline {
                visible: !swapRow.done && (swapRow.modelData.timeline ?? []).length > 0
                moments: swapRow.modelData.timeline ?? []
                now: swapRow.now
                Layout.fillWidth: true; Layout.topMargin: 4
            }
        }
        Label {
            visible: swapRow.modelData.can_act !== true
            text: swapRow.modelData.action_role && swapRow.modelData.action_role !== swapRow.role
                ? "WAITING FOR " + swapRow.counterpartyLabel : String(swapRow.modelData.state).toUpperCase()
            color: "#7B8798"; font.pixelSize: 9; font.weight: Font.Bold; font.letterSpacing: 0.7
        }
        LuxeButton {
            objectName: swapRow.actionObjectName
            visible: swapRow.modelData.can_act === true
            text: String(swapRow.modelData.action_label ?? "Continue")
            primary: true
            enabled: swapRow.actionEnabled
            onClicked: swapRow.act()
        }
    }
}
