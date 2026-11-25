#ifndef LEZ_ZCASH_HOST_H
#define LEZ_ZCASH_HOST_H

// A Zcash node (Zebra), hosted by Basecamp: the Zcash Node desk configures
// Basecamp's zebrad_module (logos-zebrad-module, Zebra in-process) and starts
// it, only when the stack names a network.
//
//   LEZ_ZEBRA_NETWORK   regtest, testnet or mainnet; unset: nothing is started
//   LEZ_ZEBRA_CONFIG    a JSON object of the module's settings for that network
//                       (absolute path): cacheDir, exposeJsonRpc, listenAddr,
//                       logFilter, peersetInitialTargetSize and, on regtest,
//                       activationHeights and minerAddress
//                       (deploy/scripts/gen-config.sh renders the devnet's)
//
// The JSON-RPC the module opens with exposeJsonRpc keeps Zebra's cookie
// authentication: the cookie is <cacheDir>/.cookie, owner-only, new at every
// start. restart() stops the node and starts it again with the settings read
// again; its chain stays. resetChain() is for Regtest only: it stops the node, deletes the chain
// under cacheDir and starts a new one with the same settings.
//
// Each returns what it did or what the node reports, as one JSON object, for
// the desk to show.

#include <QDir>
#include <QFile>
#include <QJsonDocument>
#include <QJsonObject>
#include <QString>
#include <QVariantMap>

namespace lez_zcash_host {

inline QString report(const QJsonObject& status)
{
    const QString line = QString::fromUtf8(QJsonDocument(status).toJson(QJsonDocument::Compact));
    qInfo("lez_zcash_host: %s", qPrintable(line));
    return line;
}

inline QString network()
{
    return qEnvironmentVariable("LEZ_ZEBRA_NETWORK");
}

// The settings file, as the module's configure() takes them; an empty map and
// a reason when the file is missing or not a JSON object.
inline QVariantMap settings(QString* why)
{
    QFile file(qEnvironmentVariable("LEZ_ZEBRA_CONFIG"));
    if (file.fileName().isEmpty() || !file.open(QIODevice::ReadOnly)) {
        *why = QStringLiteral("LEZ_ZEBRA_CONFIG names no readable file");
        return {};
    }
    const QJsonDocument document = QJsonDocument::fromJson(file.read(64 * 1024));
    if (!document.isObject()) {
        *why = QStringLiteral("LEZ_ZEBRA_CONFIG is not a JSON object");
        return {};
    }
    return document.object().toVariantMap();
}

template <typename Zebra>
QString configureAndStart(Zebra& zebra, const QString& net)
{
    QString why;
    const QVariantMap config = settings(&why);
    if (!why.isEmpty()) return QStringLiteral("not started: ") + why;
    const auto configured = zebra.configure(net, config);
    if (!configured.success)
        return QStringLiteral("not configured: ") + configured.template getError<QString>();
    const auto started = zebra.start(net);
    return started.success ? QStringLiteral("started on ") + net
                           : QStringLiteral("not started: ") + started.template getError<QString>();
}

template <typename Zebra>
QString start(Zebra& zebra)
{
    QJsonObject status;
    const QString net = network();
    status.insert(QStringLiteral("zcash_node"),
                  net.isEmpty() ? QStringLiteral("nothing configured") : configureAndStart(zebra, net));
    return report(status);
}

// The node's own status, trimmed to what an operator reads.
template <typename Zebra>
QString status(Zebra& zebra)
{
    const QVariantMap node = zebra.status();
    QJsonObject status;
    for (const char* key : {"state", "network", "height", "peers", "syncPercent", "version", "lastError", "rpcExposed"})
        if (node.contains(QLatin1String(key)))
            status.insert(QLatin1String(key), QJsonValue::fromVariant(node.value(QLatin1String(key))));
    if (status.isEmpty()) status.insert(QStringLiteral("zcash_node"), network().isEmpty() ? QStringLiteral("nothing configured") : QStringLiteral("no status yet"));
    return QString::fromUtf8(QJsonDocument(status).toJson(QJsonDocument::Compact));
}

template <typename Zebra>
QString restart(Zebra& zebra)
{
    QJsonObject status;
    const QString net = network();
    if (net.isEmpty()) {
        status.insert(QStringLiteral("zcash_node"), QStringLiteral("nothing configured"));
        return report(status);
    }
    const auto stopped = zebra.stop();
    status.insert(QStringLiteral("zcash_node"),
                  stopped.success ? QStringLiteral("restarted: ") + configureAndStart(zebra, net)
                                  : QStringLiteral("not restarted: stop failed: ") + stopped.template getError<QString>());
    return report(status);
}

template <typename Zebra>
QString resetChain(Zebra& zebra)
{
    QJsonObject status;
    const QString net = network();
    if (net != QStringLiteral("regtest")) {
        status.insert(QStringLiteral("zcash_node"), QStringLiteral("not reset: only a regtest chain is reset"));
        return report(status);
    }
    const QString cacheDir = zebra.getConfig(net).value(QStringLiteral("cacheDir")).toString();
    const QDir dir(cacheDir);
    // Never a relative path or a filesystem root: the cache directory is the
    // chain's own (gen-config.sh: /var/lib/zebra/regtest).
    if (cacheDir.isEmpty() || !QDir::isAbsolutePath(cacheDir) || dir.isRoot()) {
        status.insert(QStringLiteral("zcash_node"), QStringLiteral("not reset: no cacheDir"));
        return report(status);
    }
    const auto stopped = zebra.stop();
    if (!stopped.success) {
        status.insert(QStringLiteral("zcash_node"), QStringLiteral("not reset: stop failed: ") + stopped.template getError<QString>());
        return report(status);
    }
    QDir(cacheDir).removeRecursively();
    QDir().mkpath(cacheDir);
    status.insert(QStringLiteral("zcash_node"), QStringLiteral("chain reset, ") + configureAndStart(zebra, net));
    return report(status);
}

} // namespace lez_zcash_host

#endif
