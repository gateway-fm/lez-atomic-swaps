import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ApplicationWindow {
    id: window
    width: 1020
    height: 880
    minimumWidth: 720
    minimumHeight: 560
    visible: true
    title: "LEZ Atomic Swaps — real Basecamp views on the stub host (sample data, no chains)"
    color: "#0b0f14"

    header: ToolBar {
        RowLayout {
            anchors.fill: parent
            anchors.margins: 8
            spacing: 8

            Label {
                text: "M3+ preview"
                color: "#f5b041"
                font.bold: true
            }
            Button {
                text: "Maker Console"
                checkable: true
                checked: true
                onToggled: if (checked) {
                    takerButton.checked = false;
                    viewLoader.source = sourceRoot + "/basecamp/maker/src/qml/Main.qml";
                }
            }
            Button {
                id: takerButton
                text: "Taker Route"
                checkable: true
                onToggled: if (checked) {
                    makerButton.checked = false;
                    viewLoader.source = sourceRoot + "/basecamp/taker/src/qml/Main.qml";
                }
            }
            Item { Layout.fillWidth: true }
            Label {
                text: "Stub host: responses are canned; no daemon, no Delivery/Chat, no chains"
                color: "#8899aa"
                font.italic: true
            }
        }
    }

    footer: ToolBar {
        height: 28
        Label {
            anchors.centerIn: parent
            text: "The loaded QML is the production view source, unmodified, from apps/basecamp"
            color: "#8899aa"
        }
    }

    function selectMaker() {
        makerButton.checked = true;
        takerButton.checked = false;
        viewLoader.source = sourceRoot + "/basecamp/maker/src/qml/Main.qml";
    }

    Component.onCompleted: selectMaker()

    Loader {
        id: viewLoader
        anchors.fill: parent
    }
}
