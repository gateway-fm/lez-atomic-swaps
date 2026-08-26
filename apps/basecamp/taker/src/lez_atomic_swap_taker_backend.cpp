#include "lez_atomic_swap_taker_backend.h"
#include "logos_sdk.h"

#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonParseError>

namespace {
constexpr qsizetype kMaximumOfferAnnouncementBytes = 32 * 1024;

QString compact(const QJsonObject& value)
{
    return QString::fromUtf8(QJsonDocument(value).toJson(QJsonDocument::Compact));
}

bool exactUnsigned(const QString& value, qulonglong& result)
{
    bool ok = false;
    result = value.toULongLong(&ok, 10);
    return ok && result <= 9007199254740991ULL && QString::number(result) == value;
}

QString invalid()
{
    return QStringLiteral("{\"ok\":false,\"code\":\"invalid_input\",\"message\":\"Review fields are invalid or exceed the exact UI range\"}");
}

QString unavailable()
{
    return QStringLiteral("{\"ok\":false,\"code\":\"offer_unavailable\",\"message\":\"The selected signed offer is no longer live\"}");
}
}

LezAtomicSwapTakerBackend::LezAtomicSwapTakerBackend()
    : rpc_(QStringLiteral("LEZ_TAKER_RPC_SOCKET"))
    , chat_(std::make_unique<LogosChatBridge>(QStringLiteral("taker"), this))
{
    (void)qEnvironmentVariable("LEZ_TAKER_RPC_SOCKET");
}

LezAtomicSwapTakerBackend::~LezAtomicSwapTakerBackend() = default;

void LezAtomicSwapTakerBackend::onContextReady()
{
    chat_->initialise(modules().chat_module, modules().delivery_module);
}

QString LezAtomicSwapTakerBackend::health()
{
    return rpc_.call("taker_health", "{\"schema_version\":1}");
}

QString LezAtomicSwapTakerBackend::chatStatus()
{
    return chat_->statusJson();
}

QString LezAtomicSwapTakerBackend::connectChat(QString peerAddress)
{
    return chat_->connectPeer(peerAddress);
}

QString LezAtomicSwapTakerBackend::connectOffer(QString makerIdentity, QString offerId)
{
    return chat_->connectOffer(makerIdentity, offerId);
}

QString LezAtomicSwapTakerBackend::resetChat()
{
    return chat_->resetSession();
}

QString LezAtomicSwapTakerBackend::listOffers(QString pair, QString direction)
{
    return chat_->listOffers(pair, direction);
}

QString LezAtomicSwapTakerBackend::initiate(
    QString requestId, QString offerId, QString pair, QString direction, QString makerIdentity,
    QString signedEnvelopeSha256, QString foreignUnits, QString expectedLezUnits,
    QString logosOfferAnnouncementBase64)
{
    qulonglong foreign = 0, lez = 0;
    if (!exactUnsigned(foreignUnits, foreign) || !exactUnsigned(expectedLezUnits, lez)) return invalid();
    const QByteArray digest = QByteArray::fromHex(signedEnvelopeSha256.toLatin1());
    if (digest.size() != 32 || QString::fromLatin1(digest.toHex()) != signedEnvelopeSha256) return invalid();
    QJsonParseError selectionError;
    const QJsonDocument selectionDocument = QJsonDocument::fromJson(
        chat_->selectOffer(makerIdentity, offerId).toUtf8(), &selectionError);
    if (selectionError.error != QJsonParseError::NoError || !selectionDocument.isObject())
        return unavailable();
    const QJsonObject selectionEnvelope = selectionDocument.object();
    const QString refreshed = selectionEnvelope.value(QStringLiteral("result"))
                                  .toObject()
                                  .value(QStringLiteral("selected"))
                                  .toObject()
                                  .value(QStringLiteral("announcement_base64"))
                                  .toString();
    if (!selectionEnvelope.value(QStringLiteral("ok")).toBool(false) || refreshed.isEmpty())
        return unavailable();
    logosOfferAnnouncementBase64 = refreshed;
    const QByteArray announcement = QByteArray::fromBase64(
        logosOfferAnnouncementBase64.toLatin1(), QByteArray::AbortOnBase64DecodingErrors);
    if (announcement.isEmpty() || announcement.size() > kMaximumOfferAnnouncementBytes
        || announcement.toBase64() != logosOfferAnnouncementBase64.toLatin1()) return invalid();
    QJsonArray digestBytes;
    for (const char byte : digest) digestBytes.append(static_cast<unsigned char>(byte));
    return rpc_.call("taker_swap_initiate_v1", compact({{"schema_version", 1},
        {"request_id", requestId}, {"offer_id", offerId},
        {"route", QJsonObject{{"pair", pair}, {"direction", direction}}},
        {"maker_identity", makerIdentity}, {"signed_envelope_sha256", digestBytes},
        {"foreign_units", static_cast<qint64>(foreign)}, {"expected_lez_units", static_cast<qint64>(lez)},
        {"logos_offer_announcement_base64", logosOfferAnnouncementBase64}}));
}

QString LezAtomicSwapTakerBackend::listSwaps()
{
    return rpc_.call("taker_swap_list_v1", "{\"schema_version\":1}");
}

QString LezAtomicSwapTakerBackend::monitor(QString swapId)
{
    return rpc_.call("taker_swap_monitor_v1", compact({{"schema_version", 1}, {"swap_id", swapId}}));
}

QString LezAtomicSwapTakerBackend::claim(QString requestId, QString swapId,
                                         QString expectedGeneration)
{
    qulonglong generation = 0;
    if (!exactUnsigned(expectedGeneration, generation)) return invalid();
    return rpc_.call("taker_swap_claim_v1", compact({{"schema_version", 1}, {"request_id", requestId},
        {"swap_id", swapId}, {"expected_generation", static_cast<qint64>(generation)}}));
}

QString LezAtomicSwapTakerBackend::refund(QString requestId, QString swapId,
                                          QString expectedGeneration)
{
    qulonglong generation = 0;
    if (!exactUnsigned(expectedGeneration, generation)) return invalid();
    return rpc_.call("taker_swap_refund_v1", compact({{"schema_version", 1}, {"request_id", requestId},
        {"swap_id", swapId}, {"expected_generation", static_cast<qint64>(generation)}}));
}
