#ifndef LEZ_ROLE_NODE_H
#define LEZ_ROLE_NODE_H

// The role's Node (the swap engine), launched and supervised by Basecamp: the
// desk asks Basecamp's lez_swap_node module to run the role's Node entrypoint
// and keep it running. A desk without the configuration below starts no Node
// (the Node then runs elsewhere, e.g. its own container).
//
//   LEZ_<ROLE>_NODE_LAUNCHER   the Node entrypoint, run with the role as its argument
//   LEZ_<ROLE>_NODE_LOG        the file the Node's output is appended to
//   LEZ_<ROLE>_NODE_HOLD       while this file exists the Node is kept down
//
// Call it after the role's signer is serving (lez_role_signer.h): the Node's
// sidecars ask that signer for every LEZ witness.

#include <QString>

namespace lez_role_node {

template <typename LezSwapNode>
void start(LezSwapNode& supervisor, const QString& role)
{
    const QString prefix = QStringLiteral("LEZ_") + role.toUpper() + QStringLiteral("_NODE_");
    const auto setting = [&prefix](const char* name) {
        return qEnvironmentVariable((prefix + QLatin1String(name)).toUtf8().constData());
    };
    const QString launcher = setting("LAUNCHER");
    if (launcher.isEmpty()) return;
    const int started = supervisor.start_node(role, launcher, setting("LOG"), setting("HOLD"));
    if (started != 0) qWarning("lez_role_node: %s Node not started (%d)", qPrintable(role), started);
}

} // namespace lez_role_node

#endif
