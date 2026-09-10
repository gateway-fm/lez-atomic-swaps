#include "node_market.h"

#include "local_json_rpc_client.h"

#include <QDateTime>
#include <QHash>
#include <QJsonArray>
#include <QJsonDocument>
#include <QLocale>

namespace node_market {
namespace {

QString compact(const QJsonObject& value)
{
    return QString::fromUtf8(QJsonDocument(value).toJson(QJsonDocument::Compact));
}

QString formatBtc(qint64 sats)
{
    return QString::number(static_cast<double>(sats) / 100000000.0, 'f', 8) + " BTC";
}

QString formatLez(qint64 units)
{
    // "1,000 LEZ": the grouped form the desks and their suites expect.
    return QLocale(QLocale::English, QLocale::UnitedStates).toString(units) + " LEZ";
}

// Route direction as the Node serializes it → the desk's snake-case name.
QString directionName(const QString& routeDirection)
{
    return routeDirection == QStringLiteral("TakerSellsLez") ? QStringLiteral("taker_sells_lez")
                                                             : QStringLiteral("taker_sells_foreign");
}

QString directionDisplay(const QString& name)
{
    return name == QStringLiteral("taker_sells_lez") ? QStringLiteral("LEZ → BTC")
                                                     : QStringLiteral("BTC → LEZ");
}

QJsonObject routeObject(const QString& direction)
{
    return QJsonObject{{"pair", "Bitcoin"},
                       {"direction", direction == QStringLiteral("taker_sells_lez")
                                         ? QStringLiteral("TakerSellsLez")
                                         : QStringLiteral("TakerSellsForeign")}};
}

qint64 integerField(const QJsonObject& object, const char* key)
{
    const double value = object.value(QLatin1String(key)).toDouble(-1);
    return value >= 0 && value <= 9007199254740991.0 ? static_cast<qint64>(value) : -1;
}

// The Node's exact quote, `sats * lez_units_per_lot / foreign_units_per_lot`
// with no rounding; false when it would need fractional LEZ units.
bool quoteLez(const QJsonObject& price, qint64 bitcoinSats, qint64& lez)
{
    const qint64 lezPerLot = integerField(price, "lez_units_per_lot");
    const qint64 satsPerLot = integerField(price, "foreign_units_per_lot");
    if (lezPerLot <= 0 || satsPerLot <= 0 || bitcoinSats < 0) return false;
    qint64 numerator = 0;
    if (__builtin_mul_overflow(bitcoinSats, lezPerLot, &numerator)) return false;
    if (numerator % satsPerLot != 0) return false;
    lez = numerator / satsPerLot;
    return lez <= 9007199254740991;
}

// "0.00100000–0.01000000 BTC" for a range, or the single amount.
QString rangeDisplay(qint64 minimum, qint64 maximum, QString (*format)(qint64))
{
    return minimum == maximum ? format(maximum)
                              : format(minimum).section(' ', 0, 0) + QStringLiteral("–") + format(maximum);
}

QJsonObject offerRow(const QJsonObject& offer, const QString& state, const QString& makerLabel)
{
    const QJsonObject configuration = offer.value("pair_configuration").toObject();
    const QJsonObject price = offer.value("price").toObject();
    const QString direction = directionName(configuration.value("route").toObject().value("direction").toString());
    const qint64 minimumSats = integerField(configuration, "minimum_foreign_units");
    const qint64 maximumSats = integerField(configuration, "maximum_foreign_units");
    qint64 minimumLez = 0, maximumLez = 0;
    const bool quotable = quoteLez(price, minimumSats, minimumLez) && quoteLez(price, maximumSats, maximumLez);
    const bool takerPaysBitcoin = direction == QStringLiteral("taker_sells_foreign");
    const QString bitcoin = rangeDisplay(minimumSats, maximumSats, formatBtc);
    const QString lez = quotable ? rangeDisplay(minimumLez, maximumLez, formatLez) : QStringLiteral("unquotable");
    return QJsonObject{
        {"offer_id", offer.value("id").toString()},
        {"maker_wallet_label", makerLabel},
        {"state", state},
        {"minimum_foreign_units", minimumSats},
        {"maximum_foreign_units", maximumSats},
        {"lez_units_per_lot", integerField(price, "lez_units_per_lot")},
        {"foreign_units_per_lot", integerField(price, "foreign_units_per_lot")},
        {"bitcoin_sats", maximumSats},
        {"bitcoin_display", bitcoin},
        {"lez_units", maximumLez},
        {"lez_display", lez},
        {"direction", direction},
        {"direction_display", directionDisplay(direction)},
        {"taker_pays_display", takerPaysBitcoin ? bitcoin : lez},
        {"taker_receives_display", takerPaysBitcoin ? lez : bitcoin},
        {"expires_at_unix_seconds", offer.value("expires_at_unix_seconds")},
        {"created_at_unix_seconds", offer.value("created_at_unix_seconds")},
    };
}

struct SwapRow {
    QString state;
    QString label;
    int percent = 0;
    QString detail;
    QString action;      // empty when the desk has nothing to do
    QString actionLabel;
};

// The Taker's desk states from the Taker Node's swap view; `bitcoin` and
// `lez` are that swap's exact amounts as the desk displays them. The Node's
// `available_action` wins over the lifecycle state: a refund it offers while
// the Maker's lock is still nominally awaited means the Maker missed its
// window.
SwapRow takerRow(const QString& nodeState, const QString& availableAction, bool locked,
                 const QString& direction, const QString& bitcoin, const QString& lez)
{
    const bool fundsBitcoin = direction == QStringLiteral("taker_sells_foreign");
    if (availableAction == QStringLiteral("refund") && nodeState != "refund_available")
        return {"refund_ready", "The Maker missed its lock window", 60,
                "Your Node offers the recovery path; nothing else can happen on this swap",
                "refund_btc", "Refund " + bitcoin};
    if (nodeState == "not_activated" || nodeState == "initiating")
        return {"preparing", "Preparing the swap", 10,
                "Reservation, funding plan, signing ceremony and actor activation run inside your Node", "", ""};
    if (nodeState == "awaiting_first_lock") {
        if (fundsBitcoin && !locked)
            return {"lock_ready", "Your Bitcoin lock is ready", 20,
                    "Your move — Lock " + bitcoin + " broadcasts the exact funding transaction your wallet signed",
                    "lock_btc", "Lock " + bitcoin};
        return {"locking_btc", "Bitcoin lock confirming", 35,
                "Your Node observes the lock; the Maker funds LEZ once it is confirmed", "", ""};
    }
    if (nodeState == "awaiting_second_lock")
        return {"awaiting_maker_lock", "Waiting for the Maker's LEZ escrow", 50,
                "The Maker's Node funds the escrow automatically after your lock confirms", "", ""};
    if (nodeState == "claim_available")
        return {"claim_ready", "Your LEZ claim is ready", 70,
                "Your move — Claim " + lez + " reveals the adaptor secret the Maker needs for its Bitcoin claim",
                "claim_lez", "Claim " + lez};
    if (nodeState == "claim_in_progress")
        return {"claiming_lez", "LEZ claim submitted", 85,
                "Your Node observes the claim; the Maker's follow-up Bitcoin claim completes the swap", "", ""};
    if (nodeState == "completed")
        return {"completed", "Completed", 100, "Both legs settled on chain", "", ""};
    if (nodeState == "refund_available")
        return {"refund_ready", "Refund available", 60,
                "The Maker did not lock in time; you may recover your Bitcoin", "refund_btc", "Refund " + bitcoin};
    if (nodeState == "refund_in_progress")
        return {"refunding", "Refund submitted", 80, "Your Node observes the refund", "", ""};
    if (nodeState == "refunded")
        return {"refunded", "Refunded", 100, "Your Bitcoin came back", "", ""};
    return {"attention_required", "Needs attention", 0,
            "The actor reports a state the desk cannot advance; inspect it from the CLI", "", ""};
}

// The Maker's desk states from its supervised actor's observation. The
// actor's `next_action` names recovery once the Maker's lock window is gone.
SwapRow makerRow(const QString& phase, const QString& nextAction, const QString& scheduleState)
{
    if (nextAction == QStringLiteral("recover_taker_leg"))
        return {"recovering", "Lock window missed", 55,
                "Your Node could not lock in time; it recovers once the Taker's refund is final", "", ""};
    if (phase == "offered" || phase == "awaiting_taker_confirmations")
        return {"awaiting_taker_lock", "Waiting for the Taker's Bitcoin lock", 20,
                "Your Node observes Bitcoin; nothing to click", "", ""};
    if (phase == "taker_lock_confirmed" || phase == "awaiting_maker_confirmations")
        return {"funding_lez", "Funding the LEZ escrow", 45,
                "Your Node funds the escrow automatically now that the Bitcoin lock is confirmed", "", ""};
    if (phase == "both_legs_locked")
        return {"awaiting_taker_claim", "Waiting for the Taker's LEZ claim", 65,
                "The Taker's revealing claim is the next step", "", ""};
    if (phase == "claim_evidence_available")
        return {"claiming_btc", "Claiming Bitcoin", 85,
                "Your Node claims the Bitcoin with the revealed secret", "", ""};
    if (phase == "completed")
        return {"completed", "Completed", 100, "Both legs settled on chain", "", ""};
    if (phase == "maker_leg_refunded" || phase == "taker_leg_refunded" || phase == "refunded")
        return {"refunded", "Refunded", 100, "The swap was unwound", "", ""};
    if (scheduleState == "failed")
        return {"failed", "Actor failed", 0, "The supervisor gave up on this actor; inspect it from the CLI", "", ""};
    return {"preparing", "Preparing", 10, "The actor has not observed a chain yet", "", ""};
}

// The countersigned schedule as the desk shows it beside a running swap,
// named from this role's side. The Maker's leg is locked second and refunds
// first; the Taker's leg refunds later, and the Bitcoin refund script path
// also waits for a height.
QJsonArray timelineFor(const QJsonObject& terms, const QString& role)
{
    if (terms.isEmpty()) return {};
    const bool maker = role == QStringLiteral("maker");
    const auto moment = [&terms](const char* label, const char* key) {
        return QJsonObject{{"label", QString::fromLatin1(label)},
                           {"at_unix_seconds", terms.value(QLatin1String(key))}};
    };
    return QJsonArray{
        moment(maker ? "Your lock by" : "Maker locks by", "maker_second_lock_cutoff_unix_seconds"),
        moment(maker ? "Your refund by" : "Maker refunds by", "earlier_refund_latest_unix_seconds"),
        moment(maker ? "Taker refund from" : "Your refund from", "later_refund_earliest_unix_seconds"),
        QJsonObject{{"label", "BTC refund height"}, {"height", terms.value("bitcoin_refund_height")}},
    };
}

// "0.02300000 BTC ↔ 2,300 LEZ" from the agreement's amounts.
QString amountsDisplay(const QJsonObject& terms)
{
    if (terms.isEmpty()) return {};
    return formatBtc(integerField(terms, "bitcoin_value_sat")) + QStringLiteral(" ↔ ")
        + formatLez(integerField(terms, "lez_amount"));
}

QJsonObject swapRowObject(const SwapRow& row, const QString& swapId, const QString& offerId,
                          const QString& direction, const QString& makerLabel,
                          const QString& takerLabel, const QString& role, qint64 generation,
                          const QJsonObject& terms, const QString& fill)
{
    const bool canAct = !row.action.isEmpty();
    return QJsonObject{
        {"ui_swap_id", swapId},
        {"protocol_swap_id", swapId},
        {"offer_id", offerId},
        {"maker_wallet_label", makerLabel},
        {"taker_wallet_label", takerLabel},
        {"direction", direction},
        {"direction_display", directionDisplay(direction)},
        {"state", row.state},
        {"state_label", row.label},
        {"progress_percent", row.percent},
        {"progress_detail", row.detail},
        {"eta_display", QJsonValue()},
        {"action_required", canAct ? QJsonValue(row.action) : QJsonValue()},
        {"action_role", canAct ? QJsonValue(role) : QJsonValue()},
        {"action_label", canAct ? QJsonValue(row.actionLabel) : QJsonValue()},
        {"can_act", canAct},
        {"progress_generation", generation},
        {"amounts_display", amountsDisplay(terms)},
        {"fill_display", fill},
        {"required_bitcoin_confirmations", terms.value("required_bitcoin_confirmations")},
        {"timeline", timelineFor(terms, role)},
        {"run_id", QJsonValue()},
        {"completed_at", QJsonValue()},
        {"effects", QJsonArray{}},
    };
}

QJsonObject walletEntry(const QString& id, const QString& label, const QString& role,
                        int pending, int active, int needsAction)
{
    return QJsonObject{
        {"id", id}, {"label", label}, {"role", role},
        {"network", role == "maker" ? "LEZ private local" : "Bitcoin Core regtest"},
        {"accent", role == "maker" ? "violet" : "green"},
        {"pending_offers", pending}, {"active_swaps", active}, {"needs_action", needsAction},
    };
}

bool exactUnsigned(const QString& value, qulonglong& result)
{
    bool ok = false;
    result = value.toULongLong(&ok, 10);
    return ok && result <= 9007199254740991ULL && QString::number(result) == value;
}

QString nodeFailure(const Reply& reply, const QString& fallback)
{
    return failure(reply.code.isEmpty() ? QStringLiteral("node_failure") : reply.code,
                   reply.message.isEmpty() ? fallback : reply.message);
}

} // namespace

Reply decode(const QString& envelope)
{
    Reply reply;
    QJsonParseError error;
    const QJsonDocument document = QJsonDocument::fromJson(envelope.toUtf8(), &error);
    if (error.error != QJsonParseError::NoError || !document.isObject()) {
        reply.code = QStringLiteral("invalid_response");
        reply.message = QStringLiteral("The Node returned an unreadable reply");
        return reply;
    }
    const QJsonObject object = document.object();
    reply.ok = object.value("ok").toBool(false);
    reply.result = object.value("result");
    reply.code = object.value("code").toString();
    reply.message = object.value("message").toString();
    return reply;
}

QString failure(const QString& code, const QString& message)
{
    return compact({{"ok", false}, {"code", code}, {"message", message}});
}

QString success(const QJsonValue& result)
{
    return compact({{"ok", true}, {"result", result}});
}

// ---------------------------------------------------------------- Taker ---

namespace {

QJsonArray takerOffers(const LocalJsonRpcClient& rpc, Reply* error)
{
    const Reply listed = decode(rpc.call("taker_offer_list_v1", "{\"schema_version\":1}"));
    if (!listed.ok) {
        if (error) *error = listed;
        return {};
    }
    return listed.result.toObject().value("offers").toArray();
}

QJsonObject takerSnapshotObject(const LocalJsonRpcClient& rpc, const TakerWallet& wallet,
                                const QSet<QString>& lockedSwaps, Reply* error)
{
    QJsonArray orderBook;
    QJsonObject offersById;
    for (const QJsonValue& candidate : takerOffers(rpc, error)) {
        const QJsonObject view = candidate.toObject();
        const QJsonObject offer = view.value("offer").toObject();
        if (offer.value("pair_configuration").toObject().value("route").toObject().value("pair").toString()
            != QStringLiteral("Bitcoin")) continue;
        offersById.insert(offer.value("id").toString(), view);
        orderBook.append(offerRow(offer, QStringLiteral("pending"), QStringLiteral("Munich Vault 01")));
    }
    if (error && !error->code.isEmpty() && !error->ok) return {};

    QJsonArray swaps;
    int active = 0, needsAction = 0, completed = 0;
    const Reply listed = decode(rpc.call("taker_swap_list_v1", "{\"schema_version\":1}"));
    if (listed.ok) {
        for (const QJsonValue& candidate : listed.result.toObject().value("swaps").toArray()) {
            const QJsonObject swap = candidate.toObject();
            if (swap.value("route").toObject().value("pair").toString() != QStringLiteral("Bitcoin")) continue;
            const QString swapId = swap.value("swap_id").toString();
            const QString direction = directionName(swap.value("route").toObject().value("direction").toString());
            const SwapRow row = takerRow(swap.value("state").toString(), swap.value("available_action").toString(),
                                         lockedSwaps.contains(swapId), direction,
                                         formatBtc(integerField(swap, "foreign_units")),
                                         formatLez(integerField(swap, "lez_units")));
            swaps.append(swapRowObject(row, swapId, swap.value("offer_id").toString(), direction,
                                       QStringLiteral("Munich Vault 01"), wallet.label, QStringLiteral("taker"),
                                       static_cast<qint64>(swap.value("progress_generation").toDouble()),
                                       swap.value("terms").toObject(), QString()));
            if (row.state == "completed") ++completed;
            else if (row.state != "refunded") ++active;
            if (!row.action.isEmpty()) ++needsAction;
        }
    }
    return QJsonObject{
        {"schema_version", 2},
        {"kind", "node_btc_market"},
        {"role", "taker"},
        {"selected_wallet_id", wallet.id},
        {"wallets", QJsonArray{walletEntry(wallet.id, wallet.label, "taker", 0, active, needsAction)}},
        {"inventory", QJsonArray{}},
        {"order_book", orderBook},
        {"swaps", swaps},
        {"latest_balance_evidence", QJsonValue()},
        {"summary", QJsonObject{{"pending_offers", orderBook.size()}, {"accepted_swaps", swaps.size()},
                                {"completed_swaps", completed}}},
        {"runner_ready", true},
        {"runner_busy", false},
        {"runner_detail", "Both Nodes settle swaps themselves; no runner is involved"},
    };
}

} // namespace

QString takerSnapshot(const LocalJsonRpcClient& rpc, const TakerWallet& wallet,
                      const QSet<QString>& lockedSwaps)
{
    Reply error;
    const QJsonObject snapshot = takerSnapshotObject(rpc, wallet, lockedSwaps, &error);
    if (!error.code.isEmpty() && !error.ok) return nodeFailure(error, QStringLiteral("The Taker Node did not answer"));
    return success(snapshot);
}

QString takerTake(const LocalJsonRpcClient& rpc, const LocalJsonRpcClient& slowRpc,
                  const TakerWallet& wallet, const QString& requestId, const QString& offerId,
                  qint64 foreignUnits, const QSet<QString>& lockedSwaps)
{
    Reply error;
    QJsonObject selected;
    for (const QJsonValue& candidate : takerOffers(rpc, &error)) {
        const QJsonObject view = candidate.toObject();
        if (view.value("offer").toObject().value("id").toString() == offerId) selected = view;
    }
    if (!error.code.isEmpty() && !error.ok) return nodeFailure(error, QStringLiteral("The Taker Node did not answer"));
    if (selected.isEmpty()) return failure(QStringLiteral("offer_unavailable"), QStringLiteral("That offer is no longer live"));
    const QJsonObject offer = selected.value("offer").toObject();
    const QJsonObject configuration = offer.value("pair_configuration").toObject();
    // The Node enforces the bounds at reservation; the exact quote is what it
    // expects to be told.
    qint64 lez = 0;
    if (!quoteLez(offer.value("price").toObject(), foreignUnits, lez)) {
        return failure(QStringLiteral("nonintegral_quote"),
                       QStringLiteral("That amount does not quote to whole LEZ units at the offer's price"));
    }
    const Reply initiated = decode(slowRpc.call("taker_swap_initiate_v1", compact({
        {"schema_version", 1}, {"request_id", requestId}, {"offer_id", offerId},
        {"route", configuration.value("route")}, {"maker_identity", selected.value("maker_identity")},
        {"signed_envelope_sha256", selected.value("signed_envelope_sha256")},
        {"foreign_units", foreignUnits}, {"expected_lez_units", lez}})));
    if (!initiated.ok) return nodeFailure(initiated, QStringLiteral("The Taker Node could not take the offer"));
    Reply refreshError;
    QJsonObject snapshot = takerSnapshotObject(rpc, wallet, lockedSwaps, &refreshError);
    snapshot.insert("taken", initiated.result);
    return success(snapshot);
}

QString takerAction(const LocalJsonRpcClient& rpc, const LocalJsonRpcClient& slowRpc,
                    const TakerWallet& wallet, const QString& requestId, const QString& swapId,
                    const QString& action, QSet<QString>& lockedSwaps)
{
    Reply outcome;
    if (action == QStringLiteral("lock_btc")) {
        outcome = decode(slowRpc.call("taker_swap_lock_v1", compact({{"schema_version", 1}, {"swap_id", swapId}})));
        if (outcome.ok) lockedSwaps.insert(swapId);
    } else if (action == QStringLiteral("claim_lez") || action == QStringLiteral("refund_btc")) {
        const Reply monitored = decode(rpc.call("taker_swap_monitor_v1", compact({{"schema_version", 1}, {"swap_id", swapId}})));
        if (!monitored.ok) return nodeFailure(monitored, QStringLiteral("The swap could not be read"));
        const qint64 generation = static_cast<qint64>(monitored.result.toObject().value("progress_generation").toDouble());
        const char* method = action == QStringLiteral("claim_lez") ? "taker_swap_claim_v1" : "taker_swap_refund_v1";
        outcome = decode(slowRpc.call(QString::fromLatin1(method), compact({
            {"schema_version", 1}, {"request_id", requestId}, {"swap_id", swapId},
            {"expected_generation", generation}})));
    } else {
        return failure(QStringLiteral("invalid_btc_market_request"), QStringLiteral("That Taker action is not available"));
    }
    if (!outcome.ok) return nodeFailure(outcome, QStringLiteral("The Taker Node refused the action"));
    Reply refreshError;
    QJsonObject snapshot = takerSnapshotObject(rpc, wallet, lockedSwaps, &refreshError);
    snapshot.insert("action_result", outcome.result);
    return success(snapshot);
}

// ---------------------------------------------------------------- Maker ---

namespace {

QString makerOfferState(const QString& status)
{
    if (status == "active") return QStringLiteral("pending");
    if (status == "withdrawn") return QStringLiteral("withdrawn");
    if (status == "consumed" || status == "reserved") return QStringLiteral("taken");
    return status;
}

// One `{value, revision}` list entry per Bitcoin direction, keyed by the
// desk's direction name.
QHash<QString, QJsonObject> bitcoinRows(const Reply& listed)
{
    QHash<QString, QJsonObject> rows;
    if (!listed.ok) return rows;
    for (const QJsonValue& candidate : listed.result.toArray()) {
        const QJsonObject entry = candidate.toObject();
        const QJsonObject route = entry.value("value").toObject().value("route").toObject();
        if (route.value("pair").toString() == QStringLiteral("Bitcoin"))
            rows.insert(directionName(route.value("direction").toString()), entry);
    }
    return rows;
}

// The Node's stored terms per direction: what the compose card starts from.
// A direction without a stored price reports -1 lots, which the card treats
// as unset.
QJsonArray makerRoutes(const LocalJsonRpcClient& rpc)
{
    const QHash<QString, QJsonObject> pairs = bitcoinRows(decode(rpc.call("maker_pair_list", "{}")));
    const QHash<QString, QJsonObject> prices = bitcoinRows(decode(rpc.call("maker_local_price_list", "{}")));
    QJsonArray routes;
    for (auto it = pairs.cbegin(); it != pairs.cend(); ++it) {
        const QString& direction = it.key();
        const QJsonObject configuration = it.value().value("value").toObject();
        const QJsonObject price = prices.value(direction).value("value").toObject();
        routes.append(QJsonObject{
            {"direction", direction},
            {"minimum_foreign_units", integerField(configuration, "minimum_foreign_units")},
            {"maximum_foreign_units", integerField(configuration, "maximum_foreign_units")},
            {"offer_ttl_seconds", integerField(configuration, "offer_ttl_seconds")},
            {"lez_units_per_lot", integerField(price, "lez_units_per_lot")},
            {"foreign_units_per_lot", integerField(price, "foreign_units_per_lot")},
        });
    }
    return routes;
}

QJsonObject makerSnapshotObject(const LocalJsonRpcClient& rpc, const MakerWallet& wallet, Reply* error)
{
    QJsonArray inventory;
    int pending = 0;
    // The maximum each consumed offer allowed, by the swap that took it: how
    // much of the offer the Taker actually filled.
    QHash<QString, qint64> offeredBySwap;
    const Reply offers = decode(rpc.call("maker_offer_list", "{}"));
    if (!offers.ok) {
        if (error) *error = offers;
        return {};
    }
    for (const QJsonValue& candidate : offers.result.toArray()) {
        const QJsonObject record = candidate.toObject();
        const QJsonObject offer = record.value("offer").toObject();
        if (offer.value("pair_configuration").toObject().value("route").toObject().value("pair").toString()
            != QStringLiteral("Bitcoin")) continue;
        const QString state = makerOfferState(record.value("status").toString());
        QJsonObject row = offerRow(offer, state, wallet.label);
        row.insert("revision", record.value("revision"));
        row.insert("ui_swap_id", record.value("swap_id"));
        inventory.append(row);
        if (state == "pending") ++pending;
        if (state != "pending") offeredBySwap.insert(record.value("swap_id").toString(), integerField(offer.value("pair_configuration").toObject(), "maximum_foreign_units"));
    }
    QJsonArray swaps;
    int active = 0, completed = 0;
    const Reply history = decode(rpc.call("swap_history", "{}"));
    if (history.ok) {
        for (const QJsonValue& candidate : history.result.toArray()) {
            const QJsonObject swap = candidate.toObject();
            if (swap.value("pair").toString() != QStringLiteral("Bitcoin")) continue;
            const QString swapId = swap.value("id").toString();
            const QString direction = directionName(swap.value("direction").toString());
            QString phase, nextAction, schedule;
            QJsonObject terms;
            const Reply monitored = decode(rpc.call("maker_actor_monitor_v1", compact({{"id", swapId}})));
            if (monitored.ok) {
                const QJsonObject result = monitored.result.toObject();
                schedule = result.value("schedule_state").toString();
                terms = result.value("terms").toObject();
                const QJsonObject observation = result.value("progress").toObject().value("observation").toObject();
                phase = observation.value("phase").toString();
                nextAction = observation.value("next_action").toString();
            }
            const qint64 taken = integerField(terms, "bitcoin_value_sat");
            const qint64 offered = offeredBySwap.value(swapId, -1);
            const QString fill = taken > 0 && offered > 0
                ? QString::number(100 * taken / offered) + "% of the " + formatBtc(offered) + " offered" : QString();
            const SwapRow row = makerRow(phase, nextAction, schedule);
            swaps.append(swapRowObject(row, swapId, QString(), direction, wallet.label,
                                       QStringLiteral("Zurich Wallet 01"), QStringLiteral("maker"), 0, terms, fill));
            if (row.state == "completed") ++completed;
            else if (row.state != "refunded" && row.state != "failed") ++active;
        }
    }
    return QJsonObject{
        {"schema_version", 2},
        {"kind", "node_btc_market"},
        {"role", "maker"},
        {"selected_wallet_id", wallet.id},
        {"wallets", QJsonArray{walletEntry(wallet.id, wallet.label, "maker", pending, active, 0)}},
        {"inventory", inventory},
        {"order_book", QJsonArray{}},
        {"swaps", swaps},
        {"latest_balance_evidence", QJsonValue()},
        {"summary", QJsonObject{{"pending_offers", pending}, {"accepted_swaps", swaps.size()},
                                {"completed_swaps", completed}}},
        {"routes", makerRoutes(rpc)},
        {"runner_ready", true},
        {"runner_busy", false},
        {"runner_detail", "Your Node's supervisor funds LEZ and claims Bitcoin itself"},
    };
}

qint64 revisionFor(const Reply& listed, const QString& direction)
{
    for (const QJsonValue& candidate : listed.result.toArray()) {
        const QJsonObject entry = candidate.toObject();
        const QJsonObject route = entry.value("value").toObject().value("route").toObject();
        if (route.value("pair").toString() == QStringLiteral("Bitcoin")
            && directionName(route.value("direction").toString()) == direction)
            return static_cast<qint64>(entry.value("revision").toDouble());
    }
    return -1;
}

} // namespace

QString makerSnapshot(const LocalJsonRpcClient& rpc, const MakerWallet& wallet)
{
    Reply error;
    const QJsonObject snapshot = makerSnapshotObject(rpc, wallet, &error);
    if (!error.code.isEmpty() && !error.ok) return nodeFailure(error, QStringLiteral("The Maker Node did not answer"));
    return success(snapshot);
}

QString makerPublish(const LocalJsonRpcClient& rpc, const MakerWallet& wallet,
                     const QString& requestId, const QString& direction, const RouteTerms& terms)
{
    const QJsonObject route = routeObject(direction);
    const Reply pairs = decode(rpc.call("maker_pair_list", "{}"));
    const Reply prices = decode(rpc.call("maker_local_price_list", "{}"));
    if (!pairs.ok || !prices.ok) return failure(QStringLiteral("node_failure"), QStringLiteral("The Maker Node's routes could not be read"));
    const qint64 pairRevision = revisionFor(pairs, direction);
    const qint64 priceRevision = revisionFor(prices, direction);
    QJsonObject routeRequest{
        {"request_id", requestId + "-route"},
        {"configuration", QJsonObject{{"route", route}, {"enabled", true}, {"price_source", "local"},
                                      {"minimum_foreign_units", terms.minimumForeignUnits},
                                      {"maximum_foreign_units", terms.maximumForeignUnits},
                                      {"offer_ttl_seconds", terms.offerTtlSeconds}}},
        {"price", QJsonObject{{"route", route}, {"lez_units_per_lot", terms.lezUnitsPerLot},
                              {"foreign_units_per_lot", terms.foreignUnitsPerLot}}},
    };
    if (pairRevision >= 0) routeRequest.insert("expected_pair_revision", pairRevision);
    if (priceRevision >= 0) routeRequest.insert("expected_price_revision", priceRevision);
    const Reply saved = decode(rpc.call("maker_local_route_save_v1", compact(routeRequest)));
    if (!saved.ok) return nodeFailure(saved, QStringLiteral("The Bitcoin route could not be enabled"));
    const QString offerId = QStringLiteral("offer-%1-%2")
                                .arg(direction == QStringLiteral("taker_sells_lez") ? "sell-btc" : "sell-lez")
                                .arg(QDateTime::currentSecsSinceEpoch());
    // Guarded publication: exactly the revisions the save returned, or a
    // conflict if anything changed them in between.
    const QJsonObject revisions = saved.result.toObject();
    const Reply published = decode(rpc.call("maker_offer_publish_v1", compact({
        {"schema_version", 1}, {"request_id", requestId}, {"offer_id", offerId}, {"route", route},
        {"expected_pair_revision", revisions.value("pair_revision")},
        {"expected_price_revision", revisions.value("price_revision")}})));
    if (!published.ok) return nodeFailure(published, QStringLiteral("The offer could not be published"));
    Reply refreshError;
    QJsonObject snapshot = makerSnapshotObject(rpc, wallet, &refreshError);
    snapshot.insert("published_offer_id", offerId);
    return success(snapshot);
}

QString makerWithdraw(const LocalJsonRpcClient& rpc, const MakerWallet& wallet,
                      const QString& requestId, const QString& offerId)
{
    const Reply offers = decode(rpc.call("maker_offer_list", "{}"));
    if (!offers.ok) return nodeFailure(offers, QStringLiteral("The Maker Node's offers could not be read"));
    qint64 revision = -1;
    for (const QJsonValue& candidate : offers.result.toArray()) {
        const QJsonObject record = candidate.toObject();
        if (record.value("offer").toObject().value("id").toString() == offerId)
            revision = static_cast<qint64>(record.value("revision").toDouble());
    }
    if (revision < 0) return failure(QStringLiteral("offer_unavailable"), QStringLiteral("That offer is not in this Node's inventory"));
    const Reply withdrawn = decode(rpc.call("maker_offer_withdraw", compact({
        {"request_id", requestId}, {"offer_id", offerId}, {"expected_revision", revision}})));
    if (!withdrawn.ok) return nodeFailure(withdrawn, QStringLiteral("The offer could not be withdrawn"));
    Reply refreshError;
    return success(makerSnapshotObject(rpc, wallet, &refreshError));
}

} // namespace node_market
