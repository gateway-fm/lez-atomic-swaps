#pragma once

#include <QString>
#include <QtTypes>

class LocalJsonRpcClient
{
public:
    // Ordinary owner calls (health, a single monitor) answer in well under
    // 64 KiB. A market snapshot on a Node with history (every offer and swap it
    // ever served, one entry each) does not; the snapshot readers and the
    // actions whose reply embeds a snapshot (publish, withdraw, take, lock,
    // claim, refund) use the larger budget.
    static constexpr qsizetype kMarketSnapshotBytes = 4 * 1024 * 1024;

    explicit LocalJsonRpcClient(QString environmentVariable,
                                qsizetype maximumMessageBytes = 64 * 1024,
                                int connectTimeoutMs = 3000,
                                int ioTimeoutMs = 10000);

    [[nodiscard]] QString call(const QString& method, const QString& parameterObjectJson) const;

private:
    QString environmentVariable_;
    qsizetype maximumMessageBytes_;
    int connectTimeoutMs_;
    int ioTimeoutMs_;
};
