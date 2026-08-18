#include "lez_atomic_swap_taker_backend.h"

#include <QFile>
#include <QFileInfo>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QRegularExpression>
#include <QSet>

namespace {
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

QString evidenceFailure(const QString& code, const QString& message)
{
    return compact({{"ok", false}, {"code", code}, {"message", message}});
}
}

LezAtomicSwapTakerBackend::LezAtomicSwapTakerBackend()
    : rpc_(QStringLiteral("LEZ_TAKER_RPC_SOCKET"))
{
    (void)qEnvironmentVariable("LEZ_TAKER_RPC_SOCKET");
}

QString LezAtomicSwapTakerBackend::health()
{
    return rpc_.call("taker_health", "{\"schema_version\":1}");
}

QString LezAtomicSwapTakerBackend::btcEvidence()
{
    const QString configured = qEnvironmentVariable("LEZ_M3_BTC_EVIDENCE_FILE");
    const QString path = configured.isEmpty()
        ? QStringLiteral("/run/lez-evidence/m3-btc-ui-evidence.json")
        : configured;
    const QFileInfo info(path);
    if (!info.isAbsolute() || !info.exists() || !info.isFile() || info.isSymLink()
        || info.size() <= 0 || info.size() > 262144) {
        return evidenceFailure(QStringLiteral("btc_evidence_unavailable"),
            QStringLiteral("Certified Bitcoin evidence is unavailable or unsafe"));
    }

    QFile file(path);
    if (!file.open(QIODevice::ReadOnly)) {
        return evidenceFailure(QStringLiteral("btc_evidence_unavailable"),
            QStringLiteral("Certified Bitcoin evidence cannot be opened"));
    }
    QJsonParseError error;
    const QJsonDocument document = QJsonDocument::fromJson(file.readAll(), &error);
    if (error.error != QJsonParseError::NoError || !document.isObject()) {
        return evidenceFailure(QStringLiteral("btc_evidence_invalid"),
            QStringLiteral("Certified Bitcoin evidence is not valid JSON"));
    }
    const QJsonObject evidence = document.object();
    const QJsonObject terminal = evidence.value(QStringLiteral("terminal")).toObject();
    const QJsonArray effects = evidence.value(QStringLiteral("effects")).toArray();
    const QRegularExpression transactionId(QStringLiteral("^[0-9a-f]{64}$"));
    QSet<QString> transactionIds;
    int bitcoinEffects = 0;
    int lezEffects = 0;
    bool effectsValid = effects.size() == 5;
    for (const QJsonValue& value : effects) {
        const QJsonObject effect = value.toObject();
        const QString id = effect.value(QStringLiteral("transaction_id")).toString();
        const QString chain = effect.value(QStringLiteral("chain")).toString();
        const QString finality = effect.value(QStringLiteral("finality")).toString();
        effectsValid = effectsValid && transactionId.match(id).hasMatch()
            && (finality == QStringLiteral("Confirmed")
                || finality == QStringLiteral("Finalized"));
        transactionIds.insert(id);
        bitcoinEffects += chain == QStringLiteral("Bitcoin") ? 1 : 0;
        lezEffects += chain == QStringLiteral("LEZ") ? 1 : 0;
    }
    if (evidence.value(QStringLiteral("schema_version")).toInt() != 1
        || evidence.value(QStringLiteral("kind")).toString()
            != QStringLiteral("m3_btc_ui_evidence")
        || evidence.value(QStringLiteral("pair")).toString() != QStringLiteral("Bitcoin")
        || evidence.value(QStringLiteral("direction")).toString()
            != QStringLiteral("TakerSellsForeign")
        || evidence.value(QStringLiteral("result")).toString() != QStringLiteral("passed")
        || terminal.value(QStringLiteral("phase")).toString() != QStringLiteral("completed")
        || terminal.value(QStringLiteral("revision")).toInt() != 4 || !effectsValid
        || transactionIds.size() != 5 || bitcoinEffects != 2 || lezEffects != 3
        || evidence.value(QStringLiteral("private_material_disclosed")).toBool(true)) {
        return evidenceFailure(QStringLiteral("btc_evidence_invalid"),
            QStringLiteral("Certified Bitcoin evidence failed its public schema checks"));
    }
    return compact({{"ok", true}, {"result", evidence}});
}

QString LezAtomicSwapTakerBackend::listOffers(QString pair, QString direction)
{
    return rpc_.call("taker_offer_list_v1", compact({{"schema_version", 1},
        {"route", QJsonObject{{"pair", pair}, {"direction", direction}}}}));
}

QString LezAtomicSwapTakerBackend::initiate(
    QString requestId, QString offerId, QString pair, QString direction, QString makerIdentity,
    QString signedEnvelopeSha256, QString foreignUnits, QString expectedLezUnits)
{
    qulonglong foreign = 0, lez = 0;
    if (!exactUnsigned(foreignUnits, foreign) || !exactUnsigned(expectedLezUnits, lez)) return invalid();
    const QByteArray digest = QByteArray::fromHex(signedEnvelopeSha256.toLatin1());
    if (digest.size() != 32 || QString::fromLatin1(digest.toHex()) != signedEnvelopeSha256) return invalid();
    QJsonArray digestBytes;
    for (const char byte : digest) digestBytes.append(static_cast<unsigned char>(byte));
    return rpc_.call("taker_swap_initiate_v1", compact({{"schema_version", 1},
        {"request_id", requestId}, {"offer_id", offerId},
        {"route", QJsonObject{{"pair", pair}, {"direction", direction}}},
        {"maker_identity", makerIdentity}, {"signed_envelope_sha256", digestBytes},
        {"foreign_units", static_cast<qint64>(foreign)}, {"expected_lez_units", static_cast<qint64>(lez)}}));
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
