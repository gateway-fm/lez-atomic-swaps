#include "lez_atomic_swap_maker_backend.h"
#include "logos_sdk.h"
#include "node_market.h"

#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QRegularExpression>

#include <limits>

namespace {
QString compact(const QJsonObject& value)
{
    return QString::fromUtf8(QJsonDocument(value).toJson(QJsonDocument::Compact));
}

bool exactUnsigned(const QString& value, qulonglong& result)
{
    bool ok = false;
    result = value.toULongLong(&ok, 10);
    return ok && result <= static_cast<qulonglong>(std::numeric_limits<qint64>::max())
        && QString::number(result) == value;
}

QString invalid()
{
    return QStringLiteral("{\"ok\":false,\"code\":\"invalid_input\",\"message\":\"Enter canonical unsigned integers\"}");
}

QString invalidMarket(const QString& message)
{
    return compact({{"ok", false}, {"code", "invalid_btc_market_request"},
                    {"message", message}});
}

// The Maker Node settles as one identity (the wallet `maker-munich-01` of the
// local market); the desk shows it as its only wallet.
const node_market::MakerWallet kMakerWallet{QStringLiteral("maker-munich-01"),
                                            QStringLiteral("Munich Vault 01")};

bool makerWallet(const QString& value)
{
    return value == kMakerWallet.id;
}

bool marketRequest(const QString& value)
{
    static const QRegularExpression pattern(
        QStringLiteral("^ui-maker-[a-z-]{2,24}-[0-9]{13}$"));
    return pattern.match(value).hasMatch();
}

}

LezAtomicSwapMakerBackend::LezAtomicSwapMakerBackend()
    : rpc_(QStringLiteral("LEZ_MAKER_RPC_SOCKET"))
    , snapshotRpc_(QStringLiteral("LEZ_MAKER_RPC_SOCKET"), LocalJsonRpcClient::kMarketSnapshotBytes)
    , chat_(std::make_unique<LogosChatBridge>(QStringLiteral("maker"), this))
{
    (void)qEnvironmentVariable("LEZ_MAKER_RPC_SOCKET");
}

LezAtomicSwapMakerBackend::~LezAtomicSwapMakerBackend() = default;

void LezAtomicSwapMakerBackend::onContextReady()
{
    chat_->initialise(modules().chat_module, modules().delivery_module);
}

QString LezAtomicSwapMakerBackend::health()
{
    return rpc_.call("maker_health", "{}");
}

QString LezAtomicSwapMakerBackend::chatStatus()
{
    return chat_->statusJson();
}

QString LezAtomicSwapMakerBackend::resetChat()
{
    return chat_->resetSession();
}

QString LezAtomicSwapMakerBackend::btcMarket(QString walletId)
{
    if (!makerWallet(walletId)) return invalidMarket(QStringLiteral("This desk settles as the Node's own identity"));
    return node_market::makerSnapshot(snapshotRpc_, kMakerWallet);
}

QString LezAtomicSwapMakerBackend::btcPublishOffer(
    QString requestId, QString walletId, QString direction, QString minimumForeignUnits,
    QString maximumForeignUnits, QString offerTtlSeconds, QString lezUnitsPerLot,
    QString foreignUnitsPerLot)
{
    // The desk forwards the Maker's own terms; the Node validates them.
    qulonglong minimum = 0, maximum = 0, ttl = 0, lezLot = 0, foreignLot = 0;
    if (!marketRequest(requestId) || !makerWallet(walletId)
        || (direction != QStringLiteral("taker_sells_foreign")
            && direction != QStringLiteral("taker_sells_lez"))) {
        return invalidMarket(QStringLiteral("Review the direction and wallet"));
    }
    if (!exactUnsigned(minimumForeignUnits, minimum) || !exactUnsigned(maximumForeignUnits, maximum)
        || !exactUnsigned(offerTtlSeconds, ttl) || !exactUnsigned(lezUnitsPerLot, lezLot)
        || !exactUnsigned(foreignUnitsPerLot, foreignLot)) {
        return invalid();
    }
    const node_market::RouteTerms terms{
        static_cast<qint64>(minimum), static_cast<qint64>(maximum), static_cast<qint64>(ttl),
        static_cast<qint64>(lezLot), static_cast<qint64>(foreignLot)};
    // The reply embeds a market snapshot: the snapshot budget applies.
    return node_market::makerPublish(snapshotRpc_, kMakerWallet, requestId, direction, terms);
}

QString LezAtomicSwapMakerBackend::btcWithdrawOffer(
    QString requestId, QString walletId, QString offerId)
{
    static const QRegularExpression offerPattern(QStringLiteral("^[A-Za-z0-9._-]{8,64}$"));
    if (!marketRequest(requestId) || !makerWallet(walletId)
        || !offerPattern.match(offerId).hasMatch()) {
        return invalidMarket(QStringLiteral("The pending offer selection is invalid"));
    }
    return node_market::makerWithdraw(snapshotRpc_, kMakerWallet, requestId, offerId);
}

QString LezAtomicSwapMakerBackend::btcSwapAction(
    QString requestId, QString walletId, QString swapId, QString action)
{
    // The Maker Node's supervisor funds LEZ and claims Bitcoin itself; the
    // desk only refreshes. Manual claim and refund stay on the swap panel.
    static const QRegularExpression swapPattern(QStringLiteral("^[0-9a-f]{64}$"));
    if (!marketRequest(requestId) || !makerWallet(walletId)
        || !swapPattern.match(swapId).hasMatch()
        || (action != QStringLiteral("fund_lez") && action != QStringLiteral("claim_btc")
            && action != QStringLiteral("lock_btc")
            && action != QStringLiteral("claim_lez"))) {
        return invalidMarket(QStringLiteral("That Maker action is not available"));
    }
    return node_market::makerSnapshot(snapshotRpc_, kMakerWallet);
}

QString LezAtomicSwapMakerBackend::history()
{
    return snapshotRpc_.call("swap_history", "{}");
}

QString LezAtomicSwapMakerBackend::monitor(QString swapId)
{
    return rpc_.call("maker_actor_monitor_v1", compact({{"id", swapId}}));
}

QString LezAtomicSwapMakerBackend::claim(QString requestId, QString swapId,
                                         QString expectedGeneration)
{
    qulonglong generation = 0;
    if (!exactUnsigned(expectedGeneration, generation)) return invalid();
    return rpc_.call("maker_actor_claim_v1", compact({{"request_id", requestId}, {"id", swapId},
        {"expected_generation", static_cast<qint64>(generation)}}));
}

QString LezAtomicSwapMakerBackend::refund(QString requestId, QString swapId,
                                          QString expectedGeneration)
{
    qulonglong generation = 0;
    if (!exactUnsigned(expectedGeneration, generation)) return invalid();
    return rpc_.call("maker_actor_refund_v1", compact({{"request_id", requestId}, {"id", swapId},
        {"expected_generation", static_cast<qint64>(generation)}}));
}
