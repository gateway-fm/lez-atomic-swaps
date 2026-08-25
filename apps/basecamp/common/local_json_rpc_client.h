#pragma once

#include <QString>
#include <QtTypes>

class LocalJsonRpcClient
{
public:
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
