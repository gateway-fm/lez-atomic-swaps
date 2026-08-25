#include "logos_chat_bridge.h"

#include <QJsonDocument>
#include <QJsonParseError>

namespace {
constexpr qsizetype kMaximumGatewayFrameBytes = 1024 * 1024;
constexpr qsizetype kMaximumGatewayRpcMessageBytes = 4 * 1024 * 1024;
constexpr qsizetype kMaximumAddressBytes = 16 * 1024;
constexpr qsizetype kMaximumConversationIdBytes = 4 * 1024;
constexpr int kPollIntervalMs = 50;
constexpr int kInitialSendRetryTicks = 20;
constexpr int kMaximumSendRetryTicks = 100;

QJsonObject parseSuccess(const QString& encoded)
{
    QJsonParseError error;
    const QJsonDocument document = QJsonDocument::fromJson(encoded.toUtf8(), &error);
    if (error.error != QJsonParseError::NoError || !document.isObject()) return {};
    const QJsonObject envelope = document.object();
    if (!envelope.value(QStringLiteral("ok")).toBool(false)
        || !envelope.contains(QStringLiteral("result"))) {
        return {};
    }
    return envelope;
}

QString compact(const QJsonObject& value)
{
    return QString::fromUtf8(QJsonDocument(value).toJson(QJsonDocument::Compact));
}
}

LogosChatBridge::LogosChatBridge(QString role, QObject* parent)
    : QObject(parent)
    , role_(std::move(role))
    , gatewayRpc_(QStringLiteral("LEZ_LOGOS_CHAT_GATEWAY_SOCKET"),
                  kMaximumGatewayRpcMessageBytes, 50, 500)
{
    pollTimer_.setParent(this);
    pollTimer_.setInterval(kPollIntervalMs);
    connect(&pollTimer_, &QTimer::timeout, this, &LogosChatBridge::pollOutbox);
}

LogosChatBridge::~LogosChatBridge()
{
    pollTimer_.stop();
    if (initialised_ && shutdown_) shutdown_();
}

QString LogosChatBridge::statusJson() const
{
    const QJsonObject result{{QStringLiteral("schema_version"), 1},
                             {QStringLiteral("role"), role_},
                             {QStringLiteral("state"), state_},
                             {QStringLiteral("online"), online_},
                             {QStringLiteral("session_bound"), sessionBound_},
                             {QStringLiteral("address"), localAddress_},
                             {QStringLiteral("peer_configured"), !peerAddress_.isEmpty()},
                             {QStringLiteral("conversation_ready"), !conversationId_.isEmpty()}};
    return response(true, QString(), result);
}

QString LogosChatBridge::connectPeer(const QString& peerAddress)
{
    if (role_ != QStringLiteral("taker")) {
        return response(false, QStringLiteral("role_forbidden"), {});
    }
    if (!validText(peerAddress, kMaximumAddressBytes)) {
        return response(false, QStringLiteral("invalid_peer_address"), {});
    }
    if (!peerAddress_.isEmpty() && peerAddress_ != peerAddress) {
        return response(false, QStringLiteral("session_conflict"), {});
    }
    peerAddress_ = peerAddress;
    if (!online_ || !createConversation_) {
        state_ = QStringLiteral("waiting for Chat delivery");
        return response(false, QStringLiteral("chat_not_online"), {});
    }
    createPeerConversation();
    bindSession();
    if (conversationId_.isEmpty()) {
        return response(false, QStringLiteral("conversation_failed"), {});
    }
    return sessionBound_ ? statusJson()
                         : response(false, QStringLiteral("gateway_unavailable"), {});
}

QString LogosChatBridge::resetSession()
{
    const QString encoded = gatewayRpc_.call(
        QStringLiteral("logos_chat_reset_session_v1"),
        QStringLiteral("{\"schema_version\":1}"));
    if (parseSuccess(encoded).isEmpty()) {
        return response(false, QStringLiteral("reset_failed"), {});
    }
    sessionBound_ = false;
    peerAddress_.clear();
    conversationId_.clear();
    polling_ = false;
    bindRetryTicks_ = 0;
    sendRetryTicks_ = 0;
    sendFailureCount_ = 0;
    state_ = online_ ? QStringLiteral("online: waiting for peer")
                     : QStringLiteral("waiting for Chat delivery");
    return statusJson();
}

void LogosChatBridge::createPeerConversation()
{
    if (role_ != QStringLiteral("taker") || !online_ || peerAddress_.isEmpty()
        || !conversationId_.isEmpty() || !createConversation_) {
        return;
    }
    const auto [ok, value] = createConversation_(peerAddress_);
    if (!ok || !validText(value, kMaximumConversationIdBytes)) {
        state_ = QStringLiteral("error: conversation creation failed");
        return;
    }
    conversationId_ = value;
    bindSession();
}

void LogosChatBridge::deliveryStateChanged(const QVariantList& values)
{
    if (values.size() < 2) return;
    state_ = values.at(0).toString();
    online_ = state_ == QStringLiteral("online");
    if (online_) {
        // Chat v0.2.2 forbids synchronous module calls from an event callback.
        QTimer::singleShot(0, this, [this] {
            if (!online_) return;
            if (localAddress_.isEmpty() && getAddress_) localAddress_ = getAddress_();
            createPeerConversation();
            bindSession();
        });
    }
}

void LogosChatBridge::conversationCreated(const QVariantList& values)
{
    if (values.size() < 6 || values.at(3).toString() != QStringLiteral("direct")) return;
    const QString conversation = values.at(0).toString();
    if (!validText(conversation, kMaximumConversationIdBytes)) return;
    // `create_conversation` returns the Taker's exact conversation id, so an
    // event cannot add authority there. The Maker pins both identities from
    // the first structurally valid frame and authenticated sender instead.
    if (role_ != QStringLiteral("maker") || peerAddress_.isEmpty()) return;
    if (!conversationId_.isEmpty() && conversationId_ != conversation) return;
    conversationId_ = conversation;
    // `peer_label` is only a shortened conversation label in chat_module v0.2.2,
    // not the peer address. The initiator already has the exact address; the
    // recipient pins it from the first authenticated message_received sender.
    if (!peerAddress_.isEmpty()) QTimer::singleShot(0, this, [this] { bindSession(); });
}

void LogosChatBridge::messageReceived(const QVariantList& values)
{
    if (values.size() < 4) return;
    const QString conversation = values.at(0).toString();
    const QString content = values.at(1).toString();
    const QString sender = values.at(3).toString();
    if (!validText(conversation, kMaximumConversationIdBytes)
        || !validText(sender, kMaximumAddressBytes) || content.isEmpty()
        || content.toUtf8().size() > kMaximumGatewayFrameBytes
        || !validInboundFrameEnvelope(content)) {
        return;
    }
    if (conversationId_.isEmpty() && role_ == QStringLiteral("maker")
        && peerAddress_.isEmpty()) {
        conversationId_ = conversation;
        peerAddress_ = sender;
    } else if (conversation != conversationId_ || sender != peerAddress_) {
        return;
    }
    QTimer::singleShot(0, this, [this, conversation, sender, content] {
        bindSession();
        if (!sessionBound_) return;
        ingest(conversation, sender, content);
    });
}

void LogosChatBridge::bindSession()
{
    if (sessionBound_ || !validText(localAddress_, kMaximumAddressBytes)
        || !validText(peerAddress_, kMaximumAddressBytes)
        || !validText(conversationId_, kMaximumConversationIdBytes)) {
        return;
    }
    const QString encoded = gatewayRpc_.call(
        QStringLiteral("logos_chat_bind_session_v1"),
        compact({{QStringLiteral("schema_version"), 1},
                 {QStringLiteral("conversation_id"), conversationId_},
                 {QStringLiteral("local_address"), localAddress_},
                 {QStringLiteral("peer_address"), peerAddress_}}));
    sessionBound_ = !parseSuccess(encoded).isEmpty();
    if (sessionBound_) bindRetryTicks_ = 0;
    if (!sessionBound_) state_ = QStringLiteral("error: local gateway unavailable");
}

void LogosChatBridge::pollOutbox()
{
    if (online_ && !sessionBound_ && !peerAddress_.isEmpty()
        && ++bindRetryTicks_ >= 20) {
        bindRetryTicks_ = 0;
        if (conversationId_.isEmpty()) {
            createPeerConversation();
        } else {
            bindSession();
        }
    }
    if (sendRetryTicks_ > 0) {
        --sendRetryTicks_;
        return;
    }
    if (polling_ || !online_ || !sessionBound_ || conversationId_.isEmpty() || !sendMessage_) return;
    polling_ = true;
    const QJsonObject envelope = parseSuccess(gatewayRpc_.call(
        QStringLiteral("logos_chat_outbox_peek_v1"),
        QStringLiteral("{\"schema_version\":1}")));
    if (envelope.isEmpty()) {
        sessionBound_ = false;
        bindRetryTicks_ = 0;
        state_ = QStringLiteral("error: local gateway unavailable");
        polling_ = false;
        return;
    }
    if (envelope.value(QStringLiteral("result")).isNull()) {
        polling_ = false;
        return;
    }
    const QJsonObject item = envelope.value(QStringLiteral("result")).toObject();
    const QString frameId = item.value(QStringLiteral("frame_id")).toString();
    const QString content = item.value(QStringLiteral("content")).toString();
    if (frameId.size() != 64 || content.isEmpty()
        || content.toUtf8().size() > kMaximumGatewayFrameBytes) {
        state_ = QStringLiteral("error: invalid local gateway frame");
        polling_ = false;
        return;
    }
    const auto [sent, error] = sendMessage_(conversationId_, content);
    if (!sent) {
        state_ = QStringLiteral("error: Chat send failed");
        sendFailureCount_ = qMin(sendFailureCount_ + 1, 4);
        const int shift = sendFailureCount_ - 1;
        sendRetryTicks_ = qMin(kInitialSendRetryTicks << shift, kMaximumSendRetryTicks);
        polling_ = false;
        return;
    }
    sendFailureCount_ = 0;
    sendRetryTicks_ = 0;
    const QString acknowledged = gatewayRpc_.call(
        QStringLiteral("logos_chat_outbox_ack_v1"),
        compact({{QStringLiteral("schema_version"), 1},
                 {QStringLiteral("frame_id"), frameId}}));
    if (parseSuccess(acknowledged).isEmpty()) {
        state_ = QStringLiteral("error: local gateway acknowledgement failed");
    }
    polling_ = false;
}

bool LogosChatBridge::validInboundFrameEnvelope(const QString& content) const
{
    QJsonParseError error;
    const QJsonDocument document = QJsonDocument::fromJson(content.toUtf8(), &error);
    if (error.error != QJsonParseError::NoError || !document.isObject()) return false;
    const QJsonObject frame = document.object();
    const QString frameId = frame.value(QStringLiteral("frame_id")).toString();
    if (frame.value(QStringLiteral("schema_version")).toInt() != 1 || frameId.size() != 64)
        return false;
    for (const QChar character : frameId) {
        const ushort value = character.unicode();
        if (!((value >= '0' && value <= '9') || (value >= 'a' && value <= 'f')
              || (value >= 'A' && value <= 'F'))) {
            return false;
        }
    }
    const QString senderRole = frame.value(QStringLiteral("sender_role")).toString();
    const QString recipientRole = frame.value(QStringLiteral("recipient_role")).toString();
    const QString expectedSender = role_ == QStringLiteral("maker")
        ? QStringLiteral("taker") : QStringLiteral("maker");
    const QJsonObject message = frame.value(QStringLiteral("message")).toObject();
    const QString expectedKind = role_ == QStringLiteral("maker")
        ? QStringLiteral("request") : QStringLiteral("response");
    return senderRole == expectedSender && recipientRole == role_
        && message.value(QStringLiteral("kind")).toString() == expectedKind;
}

void LogosChatBridge::ingest(const QString& conversationId, const QString& sender,
                             const QString& content)
{
    const QString encoded = gatewayRpc_.call(
        QStringLiteral("logos_chat_ingest_v1"),
        compact({{QStringLiteral("schema_version"), 1},
                 {QStringLiteral("conversation_id"), conversationId},
                 {QStringLiteral("sender_address"), sender},
                 {QStringLiteral("content"), content}}));
    if (parseSuccess(encoded).isEmpty()) state_ = QStringLiteral("error: inbound frame rejected");
}

bool LogosChatBridge::validText(const QString& value, qsizetype maximum) const
{
    if (value.isEmpty() || value.toUtf8().size() > maximum) return false;
    for (const QChar character : value) {
        if (!character.isPrint()) return false;
    }
    return true;
}

QString LogosChatBridge::response(bool ok, const QString& code, const QJsonObject& result) const
{
    QJsonObject envelope{{QStringLiteral("ok"), ok}};
    if (ok) {
        envelope.insert(QStringLiteral("result"), result);
    } else {
        envelope.insert(QStringLiteral("code"), code);
        envelope.insert(QStringLiteral("message"), QStringLiteral("Logos Chat operation failed"));
    }
    return compact(envelope);
}
