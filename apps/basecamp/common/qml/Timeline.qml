import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

// A schedule as a strip of moments with live countdowns. A moment is a
// unix time (`at_unix_seconds`) or a Bitcoin height (`height`); the next
// timed moment still ahead is lit, passed ones dim.
RowLayout {
    id: timeline
    property var moments: []
    // Unix seconds; the desk ticks it once a second.
    property real now: 0
    spacing: 14
    function countdown(at) {
        var delta = Math.round(Number(at) - timeline.now)
        var abs = Math.abs(delta)
        var text = abs >= 3600 ? Math.floor(abs / 3600) + "h " + String(Math.floor(abs % 3600 / 60)).padStart(2, "0") + "m"
                 : abs >= 60 ? Math.floor(abs / 60) + "m " + String(abs % 60).padStart(2, "0") + "s"
                 : abs + "s"
        return delta >= 0 ? "in " + text : text + " ago"
    }
    function clock(at) {
        return new Date(Number(at) * 1000).toLocaleTimeString(Qt.locale("en_US"), "HH:mm")
    }
    readonly property int nextIndex: {
        for (var i = 0; i < timeline.moments.length; ++i) {
            var moment = timeline.moments[i]
            if (moment.at_unix_seconds !== undefined && Number(moment.at_unix_seconds) > timeline.now) return i
        }
        return -1
    }
    Repeater {
        model: timeline.moments
        delegate: ColumnLayout {
            id: moment
            required property var modelData
            required property int index
            readonly property bool timed: moment.modelData.at_unix_seconds !== undefined
            readonly property bool passed: moment.timed && Number(moment.modelData.at_unix_seconds) <= timeline.now
            readonly property bool next: moment.index === timeline.nextIndex
            spacing: 1
            Label {
                text: String(moment.modelData.label).toUpperCase()
                color: moment.next ? "#D8C6FF" : "#5F6B7D"
                font.pixelSize: 8; font.weight: Font.Bold; font.letterSpacing: 0.8
            }
            Label {
                text: moment.timed
                    ? timeline.countdown(moment.modelData.at_unix_seconds) + " · " + timeline.clock(moment.modelData.at_unix_seconds)
                    : "height " + String(moment.modelData.height)
                color: moment.next ? "#F1F3F6" : moment.passed ? "#4E5A6B" : "#9AA6B8"
                font.pixelSize: 10; font.family: "DejaVu Sans Mono"
            }
        }
    }
    Item { Layout.fillWidth: true }
}
