#include "lez_atomic_swap_maker_backend.h"

#include <QJsonDocument>
#include <QJsonObject>

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
}

LezAtomicSwapMakerBackend::LezAtomicSwapMakerBackend()
    : rpc_(QStringLiteral("LEZ_MAKER_RPC_SOCKET"))
{
    (void)qEnvironmentVariable("LEZ_MAKER_RPC_SOCKET");
}

QString LezAtomicSwapMakerBackend::health()
{
    return rpc_.call("maker_health", "{}");
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
    return rpc_.call("maker_local_route_save_v1", compact({{"request_id", requestId},
        {"expected_pair_revision", QJsonValue::Null}, {"expected_price_revision", QJsonValue::Null},
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
