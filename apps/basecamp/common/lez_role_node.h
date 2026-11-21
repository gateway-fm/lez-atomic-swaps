#ifndef LEZ_ROLE_NODE_H
#define LEZ_ROLE_NODE_H

// The role's Node (the swap engine), launched and supervised by Basecamp: the
// desk asks the role's payload package (lez_maker_node, lez_taker_node: the
// Node and everything it runs, installed with this desk) for its launcher and
// has Basecamp's lez_swap_node run it and keep it running. A desk without the
// configuration below starts no Node (the Node then runs elsewhere).
//
//   LEZ_<ROLE>_NODE_LAUNCHER   "package": the launcher the role's package
//                              installed; an absolute path: that launcher
//                              instead (tests); unset: no Node
//   LEZ_<ROLE>_NODE_LOG        required with a launcher: the absolute path of
//                              the file the Node's output is appended to
//   LEZ_<ROLE>_NODE_HOLD       required with a launcher: an absolute path;
//                              while a file exists there the Node is kept
//                              down (it need not exist)
//
// lez_swap_node.start_node refuses the call ("start_node: invalid arguments",
// nothing started) when the log or the hold path is unset or relative.
//
// Call it after the role's signer is serving (lez_role_signer.h): the Node's
// sidecars ask that signer for every LEZ witness.

#include <QJsonDocument>
#include <QJsonObject>
#include <QString>

#include <string>

namespace lez_role_node {

inline QString text(const QString& value) { return value; }
inline QString text(const std::string& value) { return QString::fromStdString(value); }

template <typename Payload, typename LezSwapNode>
void start(Payload& payload, LezSwapNode& supervisor, const QString& role)
{
    const QString prefix = QStringLiteral("LEZ_") + role.toUpper() + QStringLiteral("_NODE_");
    const auto setting = [&prefix](const char* name) {
        return qEnvironmentVariable((prefix + QLatin1String(name)).toUtf8().constData());
    };
    QString launcher = setting("LAUNCHER");
    if (launcher.isEmpty()) return;
    if (launcher == QLatin1String("package")) {
        launcher = text(payload.launcher());
        if (launcher.isEmpty()) {
            qWarning("lez_role_node: the %s Node package has no launcher (is lez_%s_node installed?)",
                     qPrintable(role), qPrintable(role));
            return;
        }
    }
    const int started = supervisor.start_node(role, launcher, setting("LOG"), setting("HOLD"));
    if (started != 0) qWarning("lez_role_node: %s Node not started (%d)", qPrintable(role), started);
}

// A reply that never reached the Node (`endpoint_unavailable`) while the
// role's launcher refuses to start it (issue 151): lez_swap_node's
// node_status names the refusal in last_exit ("exit 78: <the launcher's
// line>"), and the reply carries that line, verbatim, as its message. Any
// other reply is returned as it is.
template <typename LezSwapNode>
QString withLauncherRefusal(LezSwapNode& supervisor, const QString& role, const QString& reply)
{
    QJsonObject envelope = QJsonDocument::fromJson(reply.toUtf8()).object();
    if (envelope.value(QStringLiteral("code")).toString() != QLatin1String("endpoint_unavailable")) return reply;
    const QString lastExit = QJsonDocument::fromJson(text(supervisor.node_status(role)).toUtf8())
                                 .object().value(QStringLiteral("last_exit")).toString();
    const QString refused = QStringLiteral("exit 78: ");
    if (!lastExit.startsWith(refused) || lastExit.size() == refused.size()) return reply;
    envelope.insert(QStringLiteral("message"), lastExit.mid(refused.size()));
    return QString::fromUtf8(QJsonDocument(envelope).toJson(QJsonDocument::Compact));
}

} // namespace lez_role_node

#endif
