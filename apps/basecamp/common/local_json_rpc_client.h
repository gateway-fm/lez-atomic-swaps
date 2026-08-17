#pragma once

#include <QString>

class LocalJsonRpcClient
{
public:
    explicit LocalJsonRpcClient(QString environmentVariable);

    [[nodiscard]] QString call(const QString& method, const QString& parameterObjectJson) const;

private:
    QString environmentVariable_;
};
