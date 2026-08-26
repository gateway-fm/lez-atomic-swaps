#pragma once

#include "local_json_rpc_client.h"

#include <QByteArray>
#include <QJsonObject>
#include <QHash>
#include <QObject>
#include <QPointer>
#include <QSet>
#include <QString>
#include <QTimer>
#include <QVariantList>
#include <QVariantMap>

#include <functional>
#include <utility>

class LogosChatBridge final : public QObject
{
public:
    explicit LogosChatBridge(QString role, QObject* parent = nullptr);
    ~LogosChatBridge() override;

    template <typename ChatClient, typename DeliveryClient>
    void initialise(ChatClient& chat, DeliveryClient& delivery)
    {
        if (initialised_) return;
        QPointer<LogosChatBridge> self(this);
        chat.on("delivery_state_changed", [self](const QVariantList& values) {
            if (self) self->deliveryStateChanged(values);
        });
        chat.on("conversation_created", [self](const QVariantList& values) {
            if (self) self->conversationCreated(values);
        });
        chat.on("message_received", [self](const QVariantList& values) {
            if (self) self->messageReceived(values);
        });
        delivery.on("messageReceived", [self](const QVariantList& values) {
            if (self) self->offerMessageReceived(values);
        });
        delivery.on("connectionStateChanged", [self](const QVariantList& values) {
            if (self) self->offerConnectionStateChanged(values);
        });

        sendMessage_ = [&chat](const QString& conversationId, const QString& content) {
            const auto result = chat.send_message(conversationId, content);
            return std::pair<bool, QString>{result.success,
                result.success ? QString() : result.template getError<QString>()};
        };
        createConversation_ = [&chat](const QString& peerAddress) {
            const auto result = chat.create_conversation(peerAddress);
            return std::pair<bool, QString>{result.success,
                result.success ? result.template getValue<QString>()
                               : result.template getError<QString>()};
        };
        getAddress_ = [&chat] { return chat.get_address(); };
        shutdown_ = [&chat] { (void)chat.shutdown(); };
        subscribeOffers_ = [&delivery](const QString& topic) {
            const auto result = delivery.subscribe(topic);
            return std::pair<bool, QString>{result.success,
                result.success ? QString() : result.template getError<QString>()};
        };
        sendOffer_ = [&delivery](const QString& topic, const QByteArray& payload) {
            const auto result = delivery.send(topic, payload);
            return std::pair<bool, QString>{result.success,
                result.success ? QString() : result.template getError<QString>()};
        };

        QString preset = qEnvironmentVariable("LEZ_LOGOS_CHAT_PRESET");
        if (preset.isEmpty()) preset = QStringLiteral("logos.test");
        if (preset != QStringLiteral("logos.test") && preset != QStringLiteral("logos.dev")) {
            state_ = QStringLiteral("error: invalid LEZ_LOGOS_CHAT_PRESET");
            return;
        }
        const QVariantMap config{{QStringLiteral("delivery_preset"), preset},
                                 {QStringLiteral("log_level"), QStringLiteral("info")}};
        const auto result = chat.init(config);
        if (!result.success) {
            state_ = QStringLiteral("error: chat init failed");
            return;
        }
        initialised_ = true;
        localAddress_ = getAddress_();
        state_ = QStringLiteral("initialising");
        pollTimer_.start();
    }

    [[nodiscard]] QString statusJson() const;
    [[nodiscard]] QString connectPeer(const QString& peerAddress);
    [[nodiscard]] QString connectOffer(const QString& makerIdentity, const QString& offerId);
    [[nodiscard]] QString selectOffer(const QString& makerIdentity, const QString& offerId);
    [[nodiscard]] QString listOffers(const QString& pair, const QString& direction);
    [[nodiscard]] QString resetSession();

private:
    void deliveryStateChanged(const QVariantList& values);
    void conversationCreated(const QVariantList& values);
    void messageReceived(const QVariantList& values);
    void offerMessageReceived(const QVariantList& values);
    void offerConnectionStateChanged(const QVariantList& values);
    void subscribeOfferTopic();
    void broadcastOffers();
    void createPeerConversation();
    [[nodiscard]] bool bindSession(const QString& conversationId, const QString& peerAddress);
    void pollOutbox();
    void ingest(const QString& conversationId, const QString& sender, const QString& content);
    [[nodiscard]] bool validInboundFrameEnvelope(const QString& content) const;
    [[nodiscard]] bool validText(const QString& value, qsizetype maximum) const;
    [[nodiscard]] QString response(bool ok, const QString& code, const QJsonObject& result) const;

    QString role_;
    LocalJsonRpcClient gatewayRpc_;
    LocalJsonRpcClient makerRpc_;
    QTimer pollTimer_;
    QTimer offerTimer_;
    QString state_ = QStringLiteral("not_initialised");
    QString localAddress_;
    QString peerAddress_;
    QString conversationId_;
    QString offerBroadcastCursor_;
    bool initialised_ = false;
    bool online_ = false;
    bool sessionBound_ = false;
    bool offerSubscribed_ = false;
    bool polling_ = false;
    bool broadcastingOffers_ = false;
    int pendingOfferIngests_ = 0;
    int bindRetryTicks_ = 0;
    int sendRetryTicks_ = 0;
    int sendFailureCount_ = 0;
    QHash<QString, QString> makerPeers_;
    QSet<QString> boundConversations_;
    std::function<std::pair<bool, QString>(const QString&, const QString&)> sendMessage_;
    std::function<std::pair<bool, QString>(const QString&)> createConversation_;
    std::function<QString()> getAddress_;
    std::function<std::pair<bool, QString>(const QString&)> subscribeOffers_;
    std::function<std::pair<bool, QString>(const QString&, const QByteArray&)> sendOffer_;
    // The owning backend destroys this member before its LogosUiPluginContext
    // base, so these generated-module closures remain valid through shutdown.
    std::function<void()> shutdown_;
};
