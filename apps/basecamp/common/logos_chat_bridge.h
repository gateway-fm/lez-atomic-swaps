#pragma once

#include "local_json_rpc_client.h"

#include <QJsonObject>
#include <QObject>
#include <QPointer>
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

    template <typename ChatClient>
    void initialise(ChatClient& chat)
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
    [[nodiscard]] QString resetSession();

private:
    void deliveryStateChanged(const QVariantList& values);
    void conversationCreated(const QVariantList& values);
    void messageReceived(const QVariantList& values);
    void createPeerConversation();
    void bindSession();
    void pollOutbox();
    void ingest(const QString& conversationId, const QString& sender, const QString& content);
    [[nodiscard]] bool validInboundFrameEnvelope(const QString& content) const;
    [[nodiscard]] bool validText(const QString& value, qsizetype maximum) const;
    [[nodiscard]] QString response(bool ok, const QString& code, const QJsonObject& result) const;

    QString role_;
    LocalJsonRpcClient gatewayRpc_;
    QTimer pollTimer_;
    QString state_ = QStringLiteral("not_initialised");
    QString localAddress_;
    QString peerAddress_;
    QString conversationId_;
    bool initialised_ = false;
    bool online_ = false;
    bool sessionBound_ = false;
    bool polling_ = false;
    int bindRetryTicks_ = 0;
    int sendRetryTicks_ = 0;
    int sendFailureCount_ = 0;
    std::function<std::pair<bool, QString>(const QString&, const QString&)> sendMessage_;
    std::function<std::pair<bool, QString>(const QString&)> createConversation_;
    std::function<QString()> getAddress_;
    // The owning backend destroys this member before its LogosUiPluginContext
    // base, so these generated-module closures remain valid through shutdown.
    std::function<void()> shutdown_;
};
