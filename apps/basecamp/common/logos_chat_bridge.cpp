#include "logos_chat_bridge.h"

#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonParseError>
#include <QScopedValueRollback>

namespace {
constexpr qsizetype kMaximumGatewayFrameBytes = 1024 * 1024;
constexpr qsizetype kMaximumGatewayRpcMessageBytes = 4 * 1024 * 1024;
constexpr qsizetype kMaximumAddressBytes = 16 * 1024;
constexpr qsizetype kMaximumConversationIdBytes = 4 * 1024;
constexpr qsizetype kMaximumOfferAnnouncementBytes = 32 * 1024;
constexpr qsizetype kMaximumOfferIdBytes = 256;
constexpr int kPollIntervalMs = 50;
constexpr int kOfferRebroadcastIntervalMs = 10000;
// Process at most one owner-RPC page per event-loop turn. A retained cursor and
// zero-delay continuation keep large sweeps moving without monopolising Qt.
constexpr int kMaximumOfferSnapshotPagesPerTurn = 1;
constexpr int kMaximumPendingOfferIngests = 64;
constexpr int kInitialSendRetryTicks = 20;
constexpr int kMaximumSendRetryTicks = 100;
const QString kOfferContentTopic = QStringLiteral("/lez-atomic-swaps/1/offers/json");

QJsonObject parseSuccess(const QString& encoded)
{
    QJsonParseError error;
    const QJsonDocument document = QJsonDocument::fromJson(encoded.toUtf8(), &error);
    if (error.error != QJsonParseError::NoError || !document.isObject()) return {};
    const QJsonObject envelope = document.object();
    if (!envelope.value(QStringLiteral("ok")).toBool(false)
        || !envelope.contains(QStringLiteral("result"))) return {};
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
    , makerRpc_(QStringLiteral("LEZ_MAKER_RPC_SOCKET"),
                kMaximumGatewayRpcMessageBytes, 50, 500)
{
    pollTimer_.setParent(this);
    pollTimer_.setInterval(kPollIntervalMs);
    connect(&pollTimer_, &QTimer::timeout, this, &LogosChatBridge::pollOutbox);
    offerTimer_.setParent(this);
    offerTimer_.setInterval(kOfferRebroadcastIntervalMs);
    connect(&offerTimer_, &QTimer::timeout, this, [this] {
        if (offerSubscribed_) broadcastOffers();
        else subscribeOfferTopic();
    });
}

LogosChatBridge::~LogosChatBridge()
{
    pollTimer_.stop();
    offerTimer_.stop();
    if (initialised_ && shutdown_) shutdown_();
}

QString LogosChatBridge::statusJson() const
{
    const QJsonObject result{{QStringLiteral("schema_version"), 1},
                             {QStringLiteral("role"), role_},
                             {QStringLiteral("state"), state_},
                             {QStringLiteral("online"), online_},
                             {QStringLiteral("offer_subscribed"), offerSubscribed_},
                             {QStringLiteral("session_bound"), sessionBound_},
                             {QStringLiteral("session_count"), boundConversations_.size()},
                             {QStringLiteral("address"), localAddress_},
                             {QStringLiteral("peer_configured"),
                              role_ == QStringLiteral("maker") ? !makerPeers_.isEmpty()
                                                               : !peerAddress_.isEmpty()},
                             {QStringLiteral("conversation_ready"),
                              role_ == QStringLiteral("maker") ? !makerPeers_.isEmpty()
                                                               : !conversationId_.isEmpty()}};
    return response(true, QString(), result);
}

QString LogosChatBridge::connectPeer(const QString& peerAddress)
{
    if (role_ != QStringLiteral("taker"))
        return response(false, QStringLiteral("role_forbidden"), {});
    if (!validText(peerAddress, kMaximumAddressBytes))
        return response(false, QStringLiteral("invalid_peer_address"), {});
    if (!peerAddress_.isEmpty() && peerAddress_ != peerAddress)
        return response(false, QStringLiteral("session_conflict"), {});
    peerAddress_ = peerAddress;
    if (!online_ || !createConversation_) {
        state_ = QStringLiteral("waiting for Chat delivery");
        return response(false, QStringLiteral("chat_not_online"), {});
    }
    createPeerConversation();
    if (!conversationId_.isEmpty()) (void)bindSession(conversationId_, peerAddress_);
    if (conversationId_.isEmpty())
        return response(false, QStringLiteral("conversation_failed"), {});
    return sessionBound_ ? statusJson()
                         : response(false, QStringLiteral("gateway_unavailable"), {});
}

QString LogosChatBridge::connectOffer(const QString& makerIdentity, const QString& offerId)
{
    if (role_ != QStringLiteral("taker"))
        return response(false, QStringLiteral("role_forbidden"), {});
    const QJsonObject envelope = parseSuccess(selectOffer(makerIdentity, offerId));
    const QJsonObject selected = envelope.value(QStringLiteral("result"))
                                     .toObject()
                                     .value(QStringLiteral("selected"))
                                     .toObject();
    const QString address = selected.value(QStringLiteral("maker_chat_address")).toString();
    if (!validText(address, kMaximumAddressBytes))
        return response(false, QStringLiteral("offer_unavailable"), {});
    if (!peerAddress_.isEmpty() && peerAddress_ != address)
        return response(false, QStringLiteral("session_busy"), {});
    return connectPeer(address);
}

QString LogosChatBridge::selectOffer(const QString& makerIdentity, const QString& offerId)
{
    if (role_ != QStringLiteral("taker"))
        return response(false, QStringLiteral("role_forbidden"), {});
    return gatewayRpc_.call(
        QStringLiteral("logos_offer_select_v1"),
        compact({{QStringLiteral("schema_version"), 1},
                 {QStringLiteral("maker_identity"), makerIdentity},
                 {QStringLiteral("offer_id"), offerId}}));
}

QString LogosChatBridge::listOffers(const QString& pair, const QString& direction)
{
    if (role_ != QStringLiteral("taker"))
        return response(false, QStringLiteral("role_forbidden"), {});
    return gatewayRpc_.call(QStringLiteral("logos_offer_list_v1"),
        compact({{QStringLiteral("schema_version"), 1},
                 {QStringLiteral("route"),
                  QJsonObject{{QStringLiteral("pair"), pair},
                              {QStringLiteral("direction"), direction}}}}));
}

QString LogosChatBridge::resetSession()
{
    const QString encoded = gatewayRpc_.call(
        QStringLiteral("logos_chat_reset_session_v1"),
        QStringLiteral("{\"schema_version\":1}"));
    if (parseSuccess(encoded).isEmpty())
        return response(false, QStringLiteral("reset_failed"), {});
    sessionBound_ = false;
    peerAddress_.clear();
    conversationId_.clear();
    makerPeers_.clear();
    boundConversations_.clear();
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
        || !conversationId_.isEmpty() || !createConversation_) return;
    const auto [ok, value] = createConversation_(peerAddress_);
    if (!ok || !validText(value, kMaximumConversationIdBytes)) {
        state_ = QStringLiteral("error: conversation creation failed");
        return;
    }
    conversationId_ = value;
    (void)bindSession(conversationId_, peerAddress_);
}

void LogosChatBridge::deliveryStateChanged(const QVariantList& values)
{
    if (values.size() < 2) return;
    state_ = values.at(0).toString();
    online_ = state_ == QStringLiteral("online");
    if (!online_) {
        offerSubscribed_ = false;
        offerTimer_.stop();
        return;
    }
    QTimer::singleShot(0, this, [this] {
        if (!online_) return;
        if (localAddress_.isEmpty() && getAddress_) localAddress_ = getAddress_();
        offerTimer_.start();
        subscribeOfferTopic();
        createPeerConversation();
        if (!conversationId_.isEmpty()) (void)bindSession(conversationId_, peerAddress_);
    });
}

void LogosChatBridge::conversationCreated(const QVariantList& values)
{
    if (values.size() < 6 || values.at(3).toString() != QStringLiteral("direct")) return;
    const QString conversation = values.at(0).toString();
    if (!validText(conversation, kMaximumConversationIdBytes)) return;
    // The Maker binds only from message_received's authenticated sender.
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
        || !validInboundFrameEnvelope(content)) return;
    if (role_ == QStringLiteral("maker")) {
        const auto existing = makerPeers_.constFind(conversation);
        if (existing != makerPeers_.constEnd() && existing.value() != sender) return;
        if (existing == makerPeers_.constEnd() && makerPeers_.size() >= 32) return;
    } else if (conversation != conversationId_ || sender != peerAddress_) return;
    QTimer::singleShot(0, this, [this, conversation, sender, content] {
        if (!bindSession(conversation, sender)) return;
        if (role_ == QStringLiteral("maker")) makerPeers_.insert(conversation, sender);
        ingest(conversation, sender, content);
    });
}

void LogosChatBridge::offerMessageReceived(const QVariantList& values)
{
    if (role_ != QStringLiteral("taker") || values.size() < 4
        || values.at(1).toString() != kOfferContentTopic) return;
    const QByteArray payload = values.at(2).toByteArray();
    if (payload.isEmpty() || payload.size() > kMaximumOfferAnnouncementBytes) return;
    if (pendingOfferIngests_ >= kMaximumPendingOfferIngests) {
        state_ = QStringLiteral("warning: offer ingest queue is full");
        return;
    }
    ++pendingOfferIngests_;
    const QString payloadBase64 = QString::fromLatin1(payload.toBase64());
    QTimer::singleShot(0, this, [this, payloadBase64] {
        const QString encoded = gatewayRpc_.call(
            QStringLiteral("logos_offer_ingest_v1"),
            compact({{QStringLiteral("schema_version"), 1},
                     {QStringLiteral("payload_base64"), payloadBase64}}));
        if (parseSuccess(encoded).isEmpty())
            state_ = QStringLiteral("warning: rejected offer announcement");
        --pendingOfferIngests_;
    });
}

void LogosChatBridge::offerConnectionStateChanged(const QVariantList& values)
{
    if (values.size() < 2) return;
    const QString state = values.at(0).toString();
    if (state == QStringLiteral("online"))
        QTimer::singleShot(0, this, [this] {
            if (!online_) return;
            offerTimer_.start();
            subscribeOfferTopic();
        });
    else if (state == QStringLiteral("offline")) {
        offerSubscribed_ = false;
        offerTimer_.stop();
    }
}

void LogosChatBridge::subscribeOfferTopic()
{
    if (!online_ || offerSubscribed_ || !subscribeOffers_) return;
    const auto outcome = subscribeOffers_(kOfferContentTopic);
    if (!outcome.first) {
        state_ = QStringLiteral("error: offer topic subscription failed");
        return;
    }
    offerSubscribed_ = true;
    if (role_ == QStringLiteral("maker")) {
        QTimer::singleShot(0, this, [this] { broadcastOffers(); });
    }
}

void LogosChatBridge::broadcastOffers()
{
    if (role_ != QStringLiteral("maker") || !online_ || !offerSubscribed_
        || !validText(localAddress_, kMaximumAddressBytes) || !sendOffer_
        || broadcastingOffers_) return;
    QScopedValueRollback<bool> broadcastingGuard(broadcastingOffers_, true);
    QString cursor = offerBroadcastCursor_;
    bool hadAnnouncementFailure = false;
    for (int page = 0; page < kMaximumOfferSnapshotPagesPerTurn; ++page) {
        QJsonObject request{{QStringLiteral("schema_version"), 1},
                            {QStringLiteral("maker_chat_address"), localAddress_}};
        if (!cursor.isEmpty()) request.insert(QStringLiteral("after_offer_id"), cursor);
        const QJsonObject envelope = parseSuccess(makerRpc_.call(
            QStringLiteral("maker_offer_announcement_snapshot_v1"), compact(request)));
        const QJsonObject snapshot = envelope.value(QStringLiteral("result")).toObject();
        if (snapshot.value(QStringLiteral("content_topic")).toString() != kOfferContentTopic
            || !snapshot.value(QStringLiteral("announcements_base64")).isArray()) {
            state_ = QStringLiteral("error: offer snapshot unavailable");
            return;
        }
        const QJsonArray announcements =
            snapshot.value(QStringLiteral("announcements_base64")).toArray();
        for (const QJsonValue& value : announcements) {
            const QByteArray canonical = value.toString().toLatin1();
            const QByteArray payload = QByteArray::fromBase64(
                canonical, QByteArray::AbortOnBase64DecodingErrors);
            if (payload.isEmpty() || payload.size() > kMaximumOfferAnnouncementBytes
                || payload.toBase64() != canonical) {
                hadAnnouncementFailure = true;
                continue;
            }
            const auto outcome = sendOffer_(kOfferContentTopic, payload);
            if (!outcome.first) {
                hadAnnouncementFailure = true;
                continue;
            }
        }
        const QString next = snapshot.value(QStringLiteral("next_after_offer_id")).toString();
        if (next.isEmpty()) {
            offerBroadcastCursor_.clear();
            if (hadAnnouncementFailure)
                state_ = QStringLiteral("offer rebroadcast completed with retryable omissions");
            return;
        }
        if (announcements.isEmpty() || !validText(next, kMaximumOfferIdBytes)
            || (!cursor.isEmpty() && next <= cursor)) {
            state_ = QStringLiteral("error: invalid offer snapshot cursor");
            return;
        }
        cursor = next;
        offerBroadcastCursor_ = cursor;
    }
    state_ = hadAnnouncementFailure
        ? QStringLiteral("offer rebroadcast continuing after retryable omissions")
        : QStringLiteral("offer rebroadcast sweep will continue");
    QTimer::singleShot(0, this, [this] { broadcastOffers(); });
}

bool LogosChatBridge::bindSession(const QString& conversationId, const QString& peerAddress)
{
    if (!validText(localAddress_, kMaximumAddressBytes)
        || !validText(peerAddress, kMaximumAddressBytes)
        || !validText(conversationId, kMaximumConversationIdBytes)) return false;
    const QString encoded = gatewayRpc_.call(
        QStringLiteral("logos_chat_bind_session_v1"),
        compact({{QStringLiteral("schema_version"), 1},
                 {QStringLiteral("conversation_id"), conversationId},
                 {QStringLiteral("local_address"), localAddress_},
                 {QStringLiteral("peer_address"), peerAddress}}));
    const bool bound = !parseSuccess(encoded).isEmpty();
    if (bound) {
        boundConversations_.insert(conversationId);
        sessionBound_ = true;
        bindRetryTicks_ = 0;
    } else state_ = QStringLiteral("error: local gateway unavailable");
    return bound;
}

void LogosChatBridge::pollOutbox()
{
    if (role_ == QStringLiteral("taker") && online_ && !sessionBound_
        && !peerAddress_.isEmpty() && ++bindRetryTicks_ >= 20) {
        bindRetryTicks_ = 0;
        if (conversationId_.isEmpty()) createPeerConversation();
        else (void)bindSession(conversationId_, peerAddress_);
    }
    if (sendRetryTicks_ > 0) {
        --sendRetryTicks_;
        return;
    }
    if (polling_ || !online_ || !sendMessage_
        || (role_ == QStringLiteral("taker")
            && (!sessionBound_ || conversationId_.isEmpty()))) return;
    polling_ = true;
    const QJsonObject envelope = parseSuccess(gatewayRpc_.call(
        QStringLiteral("logos_chat_outbox_peek_v1"),
        QStringLiteral("{\"schema_version\":1}")));
    if (envelope.isEmpty()) {
        sessionBound_ = false;
        boundConversations_.clear();
        if (role_ == QStringLiteral("maker")) makerPeers_.clear();
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
    const QString targetConversation = item.value(QStringLiteral("conversation_id")).toString();
    const QString content = item.value(QStringLiteral("content")).toString();
    const bool targetKnown = role_ == QStringLiteral("maker")
        ? validText(targetConversation, kMaximumConversationIdBytes)
        : targetConversation == conversationId_;
    if (frameId.size() != 64 || !targetKnown || content.isEmpty()
        || content.toUtf8().size() > kMaximumGatewayFrameBytes) {
        state_ = QStringLiteral("error: invalid local gateway frame");
        polling_ = false;
        return;
    }
    if (role_ == QStringLiteral("maker")) {
        boundConversations_.insert(targetConversation);
        sessionBound_ = true;
    }
    const auto outcome = sendMessage_(targetConversation, content);
    if (!outcome.first) {
        const QString deferred = gatewayRpc_.call(
            QStringLiteral("logos_chat_outbox_defer_v1"),
            compact({{QStringLiteral("schema_version"), 1},
                     {QStringLiteral("frame_id"), frameId},
                     {QStringLiteral("conversation_id"), targetConversation}}));
        state_ = parseSuccess(deferred).isEmpty()
            ? QStringLiteral("error: local gateway deferral failed")
            : QStringLiteral("error: Chat send failed");
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
                 {QStringLiteral("frame_id"), frameId},
                 {QStringLiteral("conversation_id"), targetConversation}}));
    if (parseSuccess(acknowledged).isEmpty())
        state_ = QStringLiteral("error: local gateway acknowledgement failed");
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
              || (value >= 'A' && value <= 'F'))) return false;
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
    if (ok) envelope.insert(QStringLiteral("result"), result);
    else {
        envelope.insert(QStringLiteral("code"), code);
        envelope.insert(QStringLiteral("message"), QStringLiteral("Logos Chat operation failed"));
    }
    return compact(envelope);
}
