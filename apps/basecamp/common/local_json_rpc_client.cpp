#include "local_json_rpc_client.h"

#include <QByteArray>
#include <QDeadlineTimer>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLocalSocket>

#include <sys/stat.h>
#include <unistd.h>

#include <utility>

namespace {
QString failure(const QString& code, const QString& message)
{
    QJsonObject envelope{{"ok", false}, {"code", code}, {"message", message}};
    return QString::fromUtf8(QJsonDocument(envelope).toJson(QJsonDocument::Compact));
}

bool isOwnerSocket(const QByteArray& encodedPath)
{
    struct stat information {};
    return !encodedPath.isEmpty() && ::lstat(encodedPath.constData(), &information) == 0
        && S_ISSOCK(information.st_mode) && information.st_uid == ::geteuid()
        && (information.st_mode & 07777) == 0600;
}

bool readMore(QLocalSocket& socket, QByteArray& response, qsizetype maximumMessageBytes,
              QDeadlineTimer& deadline)
{
    const qint64 remaining = deadline.remainingTime();
    if (remaining <= 0
        || (!socket.bytesAvailable() && !socket.waitForReadyRead(static_cast<int>(remaining)))) {
        return false;
    }
    response += socket.readAll();
    return response.size() <= maximumMessageBytes;
}
}

LocalJsonRpcClient::LocalJsonRpcClient(QString environmentVariable, qsizetype maximumMessageBytes,
                                       int connectTimeoutMs, int ioTimeoutMs)
    : environmentVariable_(std::move(environmentVariable))
    , maximumMessageBytes_(maximumMessageBytes)
    , connectTimeoutMs_(connectTimeoutMs)
    , ioTimeoutMs_(ioTimeoutMs)
{
}

QString LocalJsonRpcClient::call(const QString& method, const QString& parameterObjectJson) const
{
    const QByteArray socketPath = qEnvironmentVariable(environmentVariable_.toUtf8().constData()).toUtf8();
    if (!socketPath.startsWith('/') || !isOwnerSocket(socketPath)) {
        return failure("endpoint_unavailable", "Owner-local service endpoint is unavailable");
    }

    QJsonParseError parameterError;
    const QJsonDocument parameters = QJsonDocument::fromJson(parameterObjectJson.toUtf8(), &parameterError);
    if (parameterError.error != QJsonParseError::NoError || !parameters.isObject()) {
        return failure("invalid_input", "Request fields are invalid");
    }

    QJsonObject request{{"jsonrpc", "2.0"}, {"id", 1}, {"method", method},
                        {"params", QJsonArray{parameters.object()}}};
    const QByteArray body = QJsonDocument(request).toJson(QJsonDocument::Compact);
    if (maximumMessageBytes_ <= 0 || connectTimeoutMs_ <= 0 || ioTimeoutMs_ <= 0) {
        return failure("invalid_configuration", "Local RPC client limits are invalid");
    }
    if (body.size() > maximumMessageBytes_) {
        return failure("request_too_large", "Request exceeds the local RPC limit");
    }
    const QByteArray wire = "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n"
        "Connection: close\r\nContent-Length: " + QByteArray::number(body.size()) + "\r\n\r\n" + body;

    QLocalSocket socket;
    socket.connectToServer(QString::fromUtf8(socketPath), QIODevice::ReadWrite);
    if (!socket.waitForConnected(connectTimeoutMs_)) {
        return failure("endpoint_unavailable", "Owner-local service did not accept the connection");
    }
    QDeadlineTimer ioDeadline(ioTimeoutMs_);
    if (socket.write(wire) != wire.size()
        || !socket.waitForBytesWritten(static_cast<int>(ioDeadline.remainingTime()))) {
        return failure("transport_failure", "Local request could not be sent");
    }

    QByteArray response;
    qsizetype headerEnd = -1;
    while ((headerEnd = response.indexOf("\r\n\r\n")) < 0) {
        if (!readMore(socket, response, maximumMessageBytes_, ioDeadline)) {
            return failure("transport_failure", "Local response header was incomplete");
        }
    }
    const QByteArray header = response.left(headerEnd);
    if (!header.startsWith("HTTP/1.1 200 ") && !header.startsWith("HTTP/1.0 200 ")) {
        return failure("rpc_failure", "Owner-local service rejected the request");
    }
    if (header.toLower().contains("transfer-encoding:")) {
        return failure("invalid_response", "Unsupported local response framing");
    }

    qsizetype contentLength = -1;
    for (const QByteArray& line : header.split('\n')) {
        const QByteArray clean = line.trimmed();
        if (clean.toLower().startsWith("content-length:")) {
            if (contentLength >= 0) {
                return failure("invalid_response", "Ambiguous local response framing");
            }
            bool ok = false;
            contentLength = clean.mid(sizeof("Content-Length:") - 1).trimmed().toLongLong(&ok);
            if (!ok || contentLength < 0 || contentLength > maximumMessageBytes_) {
                return failure("invalid_response", "Local response exceeds the framing limit");
            }
        }
    }
    if (contentLength < 0) {
        return failure("invalid_response", "Local response has no Content-Length");
    }

    QByteArray responseBody = response.mid(headerEnd + 4);
    while (responseBody.size() < contentLength) {
        QByteArray chunk;
        if (!readMore(socket, chunk, maximumMessageBytes_, ioDeadline)) {
            return failure("transport_failure", "Local response body was incomplete");
        }
        responseBody += chunk;
    }
    if (responseBody.size() != contentLength) {
        return failure("invalid_response", "Local response length was invalid");
    }

    QJsonParseError responseError;
    const QJsonDocument rpc = QJsonDocument::fromJson(responseBody, &responseError);
    if (responseError.error != QJsonParseError::NoError || !rpc.isObject()) {
        return failure("invalid_response", "Local service returned invalid JSON");
    }
    const QJsonObject object = rpc.object();
    if (object.value("jsonrpc").toString() != "2.0" || object.value("id").toInt(-1) != 1
        || object.contains("result") == object.contains("error")) {
        return failure("invalid_response", "Local service returned an invalid JSON-RPC envelope");
    }
    if (object.contains("error")) {
        return failure("rpc_failure", "Owner-local service rejected the operation");
    }
    QJsonObject success{{"ok", true}, {"result", object.value("result")}};
    return QString::fromUtf8(QJsonDocument(success).toJson(QJsonDocument::Compact));
}
