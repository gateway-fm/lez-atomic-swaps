#ifndef LEZ_NETWORK_HOST_H
#define LEZ_NETWORK_HOST_H

// The LEZ network node and the LEZ indexer, hosted by Basecamp: the LEZ Network
// desk asks Basecamp's blockchain_module (the LEZ network node as a Logos
// module) and lez_indexer_module to start, each only when the stack configures
// it, so one Basecamp may host either or both.
//
//   LEZ_NETWORK_NODE_CONFIG       the node's node-config.yaml (absolute path)
//   LEZ_NETWORK_NODE_DEPLOYMENT   its deployment settings (absolute path)
//   LEZ_INDEXER_CONFIG            the indexer's indexer_config.json (absolute path)
//   LEZ_INDEXER_RPC_PORT          the port the indexer serves indexer_service's JSON-RPC on
//
// Returns what it started, as one JSON object, for the desk to show.

#include <QJsonDocument>
#include <QJsonObject>
#include <QString>

namespace lez_network_host {

template <typename Chain, typename Indexer>
QString start(Chain& chain, Indexer& indexer)
{
    QJsonObject status;
    const QString config = qEnvironmentVariable("LEZ_NETWORK_NODE_CONFIG");
    if (!config.isEmpty()) {
        const auto result = chain.start(config, qEnvironmentVariable("LEZ_NETWORK_NODE_DEPLOYMENT"));
        status.insert(QStringLiteral("network_node"),
                      result.success ? QStringLiteral("started")
                                     : QStringLiteral("not started: ") + result.template getError<QString>());
    }
    const QString indexerConfig = qEnvironmentVariable("LEZ_INDEXER_CONFIG");
    if (!indexerConfig.isEmpty()) {
        const int port = qEnvironmentVariable("LEZ_INDEXER_RPC_PORT").toInt();
        const int started = indexer.start_indexer(indexerConfig);
        const int serving = started == 0 ? indexer.serve_rpc(port) : -1;
        status.insert(QStringLiteral("indexer"),
                      serving == 0 ? QStringLiteral("serving on %1").arg(port)
                                   : QStringLiteral("not started: start_indexer %1, serve_rpc %2").arg(started).arg(serving));
    }
    if (status.isEmpty()) status.insert(QStringLiteral("hosts"), QStringLiteral("nothing configured"));
    const QString line = QString::fromUtf8(QJsonDocument(status).toJson(QJsonDocument::Compact));
    qInfo("lez_network_host: %s", qPrintable(line));
    return line;
}

} // namespace lez_network_host

#endif
