#include "stub_backends.h"

#include <QHash>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLatin1String>
#include <limits>

namespace {

QString invalidInput()
{
    return QStringLiteral(
        "{\"ok\":false,\"code\":\"invalid_input\",\"message\":\"Enter canonical unsigned integers\"}");
}

bool exactUnsigned(const QString& value, qulonglong& result)
{
    bool ok = false;
    result = value.toULongLong(&ok, 10);
    return ok && result <= static_cast<qulonglong>(std::numeric_limits<qint64>::max())
        && QString::number(result) == value;
}

bool exactGeneration(const QString& value, qint64& result)
{
    qulonglong parsed = 0;
    if (!exactUnsigned(value, parsed))
        return false;
    result = static_cast<qint64>(parsed);
    return true;
}

QString ok(QJsonObject payload)
{
    payload.insert(QStringLiteral("ok"), true);
    return QString::fromUtf8(QJsonDocument(payload).toJson(QJsonDocument::Compact));
}

QString fail(const QLatin1String& code, const QString& message)
{
    const QJsonObject payload{{"ok", false}, {"code", code}, {"message", message}};
    return QString::fromUtf8(QJsonDocument(payload).toJson(QJsonDocument::Compact));
}

struct SampleSwap {
    QString id;
    QString pair;
    QString direction;
    QString stage;
    QString outcome;
    QString foreignUnits;
    QString lezUnits;
};

QJsonObject swapJson(const SampleSwap& swap, qint64 generation)
{
    return QJsonObject{{"id", swap.id},
                       {"pair", swap.pair},
                       {"direction", swap.direction},
                       {"stage", swap.stage},
                       {"outcome", swap.outcome},
                       {"foreign_units", swap.foreignUnits},
                       {"lez_units", swap.lezUnits},
                       {"generation", generation}};
}

QHash<QString, SampleSwap> sampleSwaps()
{
    return {{"swap-zec-0001",
             {"swap-zec-0001", "Zcash", "TakerSellsForeign", "settled", "claimed",
              "100000000", "50000"}},
            {"swap-btc-0002",
             {"swap-btc-0002", "Bitcoin", "TakerSellsLez", "settled", "refunded",
              "150000", "900000"}},
            {"swap-xmr-0003",
             {"swap-xmr-0003", "Monero", "TakerSellsForeign", "both_legs_locked",
              "in_flight", "4215000000000", "120000"}}};
}

} // namespace

MakerStubBackend::MakerStubBackend(QObject* parent) : QObject(parent) {}

QString MakerStubBackend::health()
{
    return ok(QJsonObject{{"service", "maker"},
                          {"version", "0.1.0-preview"},
                          {"chains", QJsonArray{"Zcash", "Bitcoin", "Monero"}},
                          {"price_source", "local"}});
}

QString MakerStubBackend::saveRoute(
    QString requestId, QString pair, QString direction, QString minimumForeignUnits,
    QString maximumForeignUnits, QString offerTtlSeconds, QString lezUnitsPerLot,
    QString foreignUnitsPerLot)
{
    qulonglong minimum = 0, maximum = 0, ttl = 0, lezLot = 0, foreignLot = 0;
    if (!exactUnsigned(minimumForeignUnits, minimum) || !exactUnsigned(maximumForeignUnits, maximum)
        || !exactUnsigned(offerTtlSeconds, ttl) || !exactUnsigned(lezUnitsPerLot, lezLot)
        || !exactUnsigned(foreignUnitsPerLot, foreignLot)) {
        return invalidInput();
    }
    if (minimum > maximum)
        return fail(QLatin1String("invalid_range"),
                    QStringLiteral("Minimum foreign units exceed the maximum"));
    return ok(QJsonObject{{"request_id", requestId},
                          {"route", QJsonObject{{"pair", pair}, {"direction", direction}}},
                          {"pair_revision", 1},
                          {"price_revision", 1},
                          {"enabled", true}});
}

QString MakerStubBackend::history()
{
    QJsonArray entries;
    const auto swaps = sampleSwaps();
    for (const auto& swap : swaps)
        entries.append(swapJson(swap, 2));
    return ok(QJsonObject{{"swaps", entries}, {"count", entries.size()}});
}

QString MakerStubBackend::monitor(QString swapId)
{
    const auto swaps = sampleSwaps();
    if (!swaps.contains(swapId))
        return fail(QLatin1String("not_found"),
                    QStringLiteral("No durable swap with id '%1'").arg(swapId));
    return ok(QJsonObject{{"swap", swapJson(swaps.value(swapId), 2)}});
}

QString MakerStubBackend::claim(QString requestId, QString swapId, QString expectedGeneration)
{
    qint64 generation = 0;
    if (!exactGeneration(expectedGeneration, generation))
        return invalidInput();
    const auto swaps = sampleSwaps();
    if (!swaps.contains(swapId))
        return fail(QLatin1String("not_found"),
                    QStringLiteral("No durable swap with id '%1'").arg(swapId));
    const SampleSwap& swap = swaps.value(swapId);
    if (swap.stage != QLatin1String("both_legs_locked"))
        return fail(QLatin1String("invalid_state"),
                    QStringLiteral("Swap '%1' is not claimable at stage '%2'")
                        .arg(swapId, swap.stage));
    return ok(QJsonObject{{"request_id", requestId},
                          {"id", swapId},
                          {"action", "claim"},
                          {"generation", generation},
                          {"next", "reveal adaptor witness and settle the LEZ claim"}});
}

QString MakerStubBackend::refund(QString requestId, QString swapId, QString expectedGeneration)
{
    qint64 generation = 0;
    if (!exactGeneration(expectedGeneration, generation))
        return invalidInput();
    const auto swaps = sampleSwaps();
    if (!swaps.contains(swapId))
        return fail(QLatin1String("not_found"),
                    QStringLiteral("No durable swap with id '%1'").arg(swapId));
    return ok(QJsonObject{{"request_id", requestId},
                          {"id", swapId},
                          {"action", "refund"},
                          {"generation", generation},
                          {"next", "wait for the cutoff, then submit the durable refund"}});
}

TakerStubBackend::TakerStubBackend(QObject* parent) : QObject(parent) {}

QString TakerStubBackend::health()
{
    return ok(QJsonObject{{"service", "taker"},
                          {"version", "0.1.0-preview"},
                          {"authenticated_offer_sources", 1}});
}

QString TakerStubBackend::listOffers(QString pair, QString direction)
{
    const QJsonArray offers{
        QJsonObject{{"offer_id", "offer-zec-4f9a12"},
                    {"pair", "Zcash"},
                    {"direction", "TakerSellsForeign"},
                    {"maker_identity",
                     "02b1e2aa7d8ba0be3f0b1c9c25e4a1d87f3b6c45d0a98fe4477c1230bdc9a65f1e"},
                    {"signed_envelope_sha256",
                     "9c2f7a41d8e0b6c35f1a92d7e480b3c6d5a1f8e27b04c9d3a6f18e5b2c7d94a01"},
                    {"foreign_units", "100000000"},
                    {"lez_units", "50000"},
                    {"ttl_seconds", 240}},
        QJsonObject{{"offer_id", "offer-btc-8d33c7"},
                    {"pair", "Bitcoin"},
                    {"direction", "TakerSellsLez"},
                    {"maker_identity",
                     "0274c9d1e0a35b8f62c7d94e1a05f3b6c8d27e4a9f01b3c5d6e7890a1b2c3d4e5f"},
                    {"signed_envelope_sha256",
                     "4a1d92c7b6e0f3a58d2c94b1e7f0a3d6c9b2e5f817a4d0c3b69e2f5a8d1c4b7e03"},
                    {"foreign_units", "150000"},
                    {"lez_units", "900000"},
                    {"ttl_seconds", 300}}};
    QJsonArray filtered;
    for (const auto& offer : offers) {
        const QJsonObject object = offer.toObject();
        if (object.value(QLatin1String("pair")).toString() == pair
            && object.value(QLatin1String("direction")).toString() == direction) {
            filtered.append(object);
        }
    }
    if (filtered.isEmpty()) {
        return ok(QJsonObject{
            {"offers", QJsonArray()},
            {"note", QStringLiteral("No authenticated offers for %1 / %2; the maker daemon may "
                                    "not advertise this route yet")
                                   .arg(pair, direction)}});
    }
    return ok(QJsonObject{{"offers", filtered}});
}

QString TakerStubBackend::initiate(
    QString requestId, QString offerId, QString pair, QString direction, QString makerIdentity,
    QString signedEnvelopeSha256, QString foreignUnits, QString expectedLezUnits)
{
    qulonglong foreign = 0, lez = 0;
    if (!exactUnsigned(foreignUnits, foreign) || !exactUnsigned(expectedLezUnits, lez))
        return invalidInput();
    if (offerId.isEmpty() || makerIdentity.isEmpty() || signedEnvelopeSha256.isEmpty())
        return fail(QLatin1String("invalid_input"),
                    QStringLiteral("Offer ID, maker identity, and envelope digest are required"));
    return ok(QJsonObject{{"request_id", requestId},
                          {"swap_id", "swap-taker-0001"},
                          {"offer_id", offerId},
                          {"pair", pair},
                          {"direction", direction},
                          {"stage", "agreement_signed"},
                          {"next", "taker submits the first on-chain lock"}});
}

QString TakerStubBackend::listSwaps()
{
    QJsonArray entries;
    const auto swaps = sampleSwaps();
    for (const auto& swap : swaps)
        entries.append(swapJson(swap, 2));
    return ok(QJsonObject{{"swaps", entries}, {"count", entries.size()}});
}

QString TakerStubBackend::monitor(QString swapId)
{
    const auto swaps = sampleSwaps();
    if (!swaps.contains(swapId))
        return fail(QLatin1String("not_found"),
                    QStringLiteral("No durable swap with id '%1'").arg(swapId));
    return ok(QJsonObject{{"swap", swapJson(swaps.value(swapId), 2)}});
}

QString TakerStubBackend::claim(QString requestId, QString swapId, QString expectedGeneration)
{
    qint64 generation = 0;
    if (!exactGeneration(expectedGeneration, generation))
        return invalidInput();
    const auto swaps = sampleSwaps();
    if (!swaps.contains(swapId))
        return fail(QLatin1String("not_found"),
                    QStringLiteral("No durable swap with id '%1'").arg(swapId));
    const SampleSwap& swap = swaps.value(swapId);
    if (swap.stage != QLatin1String("both_legs_locked"))
        return fail(QLatin1String("invalid_state"),
                    QStringLiteral("Swap '%1' is not claimable at stage '%2'")
                        .arg(swapId, swap.stage));
    return ok(QJsonObject{{"request_id", requestId},
                          {"id", swapId},
                          {"action", "claim"},
                          {"generation", generation},
                          {"next", "extract the adaptor witness from the revealed LEZ claim"}});
}

QString TakerStubBackend::refund(QString requestId, QString swapId, QString expectedGeneration)
{
    qint64 generation = 0;
    if (!exactGeneration(expectedGeneration, generation))
        return invalidInput();
    const auto swaps = sampleSwaps();
    if (!swaps.contains(swapId))
        return fail(QLatin1String("not_found"),
                    QStringLiteral("No durable swap with id '%1'").arg(swapId));
    return ok(QJsonObject{{"request_id", requestId},
                          {"id", swapId},
                          {"action", "refund"},
                          {"generation", generation},
                          {"next", "wait for the cutoff, then reclaim from the timelocked branch"}});
}
