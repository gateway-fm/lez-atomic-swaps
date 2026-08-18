#include "lez_atomic_swap_maker_backend.h"

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

bool makerWallet(const QString& value)
{
    return value == QStringLiteral("maker-munich-01")
        || value == QStringLiteral("maker-basel-02");
}

bool marketRequest(const QString& value)
{
    static const QRegularExpression pattern(
        QStringLiteral("^ui-maker-[a-z-]{2,24}-[0-9]{13}$"));
    return pattern.match(value).hasMatch();
}

struct RevisionLookup
{
    bool valid = false;
    bool found = false;
    QJsonValue revision = QJsonValue(QJsonValue::Null);
    QJsonObject value;
};

RevisionLookup revisionForRoute(const QString& response, const QString& pair,
                                const QString& direction)
{
    QJsonParseError error;
    const QJsonDocument document = QJsonDocument::fromJson(response.toUtf8(), &error);
    if (error.error != QJsonParseError::NoError || !document.isObject()) return {};
    const QJsonObject envelope = document.object();
    if (!envelope.value(QStringLiteral("ok")).toBool(false)
        || !envelope.value(QStringLiteral("result")).isArray()) {
        return {};
    }
    for (const QJsonValue& entryValue : envelope.value(QStringLiteral("result")).toArray()) {
        const QJsonObject entry = entryValue.toObject();
        const QJsonObject route = entry.value(QStringLiteral("value"))
                                      .toObject()
                                      .value(QStringLiteral("route"))
                                      .toObject();
        if (route.value(QStringLiteral("pair")).toString() != pair
            || route.value(QStringLiteral("direction")).toString() != direction) {
            continue;
        }
        const QJsonValue revision = entry.value(QStringLiteral("revision"));
        if (!revision.isDouble() || revision.toDouble() < 0) return {};
        return {true, true, revision, entry.value(QStringLiteral("value")).toObject()};
    }
    return {true, false, QJsonValue(QJsonValue::Null), {}};
}
}

LezAtomicSwapMakerBackend::LezAtomicSwapMakerBackend()
    : rpc_(QStringLiteral("LEZ_MAKER_RPC_SOCKET"))
    , demoRpc_(QStringLiteral("LEZ_BTC_DEMO_RPC_SOCKET"))
{
    (void)qEnvironmentVariable("LEZ_MAKER_RPC_SOCKET");
    (void)qEnvironmentVariable("LEZ_BTC_DEMO_RPC_SOCKET");
}

QString LezAtomicSwapMakerBackend::health()
{
    return rpc_.call("maker_health", "{}");
}

QString LezAtomicSwapMakerBackend::btcMarket(QString walletId)
{
    if (!makerWallet(walletId)) return invalidMarket(QStringLiteral("Select a known Maker wallet"));
    return demoRpc_.call("btc_market_snapshot_v1", compact({
        {"schema_version", 2}, {"role", "maker"}, {"wallet_id", walletId}}));
}

QString LezAtomicSwapMakerBackend::btcCreateOffers(
    QString requestId, QString walletId, QString count, QString bitcoinSats, QString lezUnits)
{
    qulonglong parsedCount = 0, bitcoin = 0, lez = 0;
    if (!marketRequest(requestId) || !makerWallet(walletId)
        || !exactUnsigned(count, parsedCount) || parsedCount != 1
        || !exactUnsigned(bitcoinSats, bitcoin) || bitcoin != 1000000
        || !exactUnsigned(lezUnits, lez) || lez != 1000) {
        return invalidMarket(QStringLiteral("Review the fixed offer preset and wallet"));
    }
    return demoRpc_.call("btc_offer_create_v1", compact({
        {"schema_version", 2}, {"request_id", requestId}, {"wallet_id", walletId},
        {"count", static_cast<qint64>(parsedCount)},
        {"bitcoin_sats", static_cast<qint64>(bitcoin)},
        {"lez_units", static_cast<qint64>(lez)}}));
}

QString LezAtomicSwapMakerBackend::btcWithdrawOffer(
    QString requestId, QString walletId, QString offerId)
{
    static const QRegularExpression offerPattern(QStringLiteral("^[A-Za-z0-9._-]{8,64}$"));
    if (!marketRequest(requestId) || !makerWallet(walletId)
        || !offerPattern.match(offerId).hasMatch()) {
        return invalidMarket(QStringLiteral("The pending offer selection is invalid"));
    }
    return demoRpc_.call("btc_offer_withdraw_v1", compact({
        {"schema_version", 2}, {"request_id", requestId}, {"wallet_id", walletId},
        {"offer_id", offerId}}));
}

QString LezAtomicSwapMakerBackend::btcSwapAction(
    QString requestId, QString walletId, QString swapId, QString action)
{
    static const QRegularExpression swapPattern(QStringLiteral("^swap-[0-9a-f]{16}$"));
    if (!marketRequest(requestId) || !makerWallet(walletId)
        || !swapPattern.match(swapId).hasMatch()
        || (action != QStringLiteral("fund_lez") && action != QStringLiteral("claim_btc"))) {
        return invalidMarket(QStringLiteral("That Maker action is not available"));
    }
    return demoRpc_.call("btc_swap_action_v1", compact({
        {"schema_version", 2}, {"request_id", requestId}, {"role", "maker"},
        {"wallet_id", walletId}, {"ui_swap_id", swapId}, {"action", action}}));
}

QString LezAtomicSwapMakerBackend::saveRoute(
    QString requestId, QString pair, QString direction, QString minimumForeignUnits,
    QString maximumForeignUnits, QString offerTtlSeconds, QString lezUnitsPerLot,
    QString foreignUnitsPerLot)
{
    qulonglong minimum = 0, maximum = 0, ttl = 0, lezLot = 0, foreignLot = 0;
    if (!exactUnsigned(minimumForeignUnits, minimum) || !exactUnsigned(maximumForeignUnits, maximum)
        || !exactUnsigned(offerTtlSeconds, ttl) || !exactUnsigned(lezUnitsPerLot, lezLot)
        || !exactUnsigned(foreignUnitsPerLot, foreignLot)) {
        return invalid();
    }
    const QJsonObject route{{"pair", pair}, {"direction", direction}};
    const QJsonObject configuration{{"route", route}, {"enabled", true},
                                    {"price_source", "local"},
                                    {"minimum_foreign_units", static_cast<qint64>(minimum)},
                                    {"maximum_foreign_units", static_cast<qint64>(maximum)},
                                    {"offer_ttl_seconds", static_cast<qint64>(ttl)}};
    const QJsonObject price{{"route", route}, {"lez_units_per_lot", static_cast<qint64>(lezLot)},
                            {"foreign_units_per_lot", static_cast<qint64>(foreignLot)}};

    const QString pairList = rpc_.call("maker_pair_list", "{}");
    const RevisionLookup pairRevision = revisionForRoute(pairList, pair, direction);
    if (!pairRevision.valid) return pairList;
    const QString priceList = rpc_.call("maker_local_price_list", "{}");
    const RevisionLookup priceRevision = revisionForRoute(priceList, pair, direction);
    if (!priceRevision.valid) return priceList;

    if (pairRevision.found && priceRevision.found && pairRevision.value == configuration
        && priceRevision.value == price) {
        return compact({{"ok", true},
            {"result", QJsonObject{{"pair_revision", pairRevision.revision},
                           {"price_revision", priceRevision.revision}, {"unchanged", true}}}});
    }

    return rpc_.call("maker_local_route_save_v1", compact({{"request_id", requestId},
        {"expected_pair_revision", pairRevision.revision},
        {"expected_price_revision", priceRevision.revision},
        {"configuration", configuration}, {"price", price}}));
}

QString LezAtomicSwapMakerBackend::history()
{
    return rpc_.call("swap_history", "{}");
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
