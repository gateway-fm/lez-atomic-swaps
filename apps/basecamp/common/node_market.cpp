#include "node_market.h"

#include "local_json_rpc_client.h"

#include <QDateTime>
#include <QJsonArray>
#include <QJsonDocument>
#include <QLocale>

namespace node_market {
namespace {

constexpr qint64 kFixedBitcoinSats = 1000000;
constexpr qint64 kFixedLezUnits = 1000;

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

// Quote of `bitcoinSats` at the offer's local price; both fixed amounts today.
qint64 quoteLez(const QJsonObject& price, qint64 bitcoinSats)
{
    const qint64 lezPerLot = static_cast<qint64>(price.value("lez_units_per_lot").toDouble(1));
    const qint64 satsPerLot = static_cast<qint64>(price.value("foreign_units_per_lot").toDouble(1000));
    return satsPerLot > 0 ? bitcoinSats * lezPerLot / satsPerLot : 0;
}

QJsonObject offerRow(const QJsonObject& offer, const QString& state, const QString& makerLabel)
{
    const QJsonObject configuration = offer.value("pair_configuration").toObject();
    const QString direction = directionName(configuration.value("route").toObject().value("direction").toString());
    const qint64 sats = static_cast<qint64>(configuration.value("maximum_foreign_units").toDouble(kFixedBitcoinSats));
    const qint64 lez = quoteLez(offer.value("price").toObject(), sats);
    const bool takerPaysBitcoin = direction == QStringLiteral("taker_sells_foreign");
    return QJsonObject{
        {"offer_id", offer.value("id").toString()},
        {"maker_wallet_label", makerLabel},
        {"state", state},
        {"bitcoin_sats", sats},
        {"bitcoin_display", formatBtc(sats)},
        {"lez_units", lez},
        {"lez_display", formatLez(lez)},
        {"direction", direction},
        {"direction_display", directionDisplay(direction)},
        {"taker_pays_display", takerPaysBitcoin ? formatBtc(sats) : formatLez(lez)},
        {"taker_receives_display", takerPaysBitcoin ? formatLez(lez) : formatBtc(sats)},
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

// The Taker's desk states from the Taker Node's swap view.
SwapRow takerRow(const QString& nodeState, bool locked, const QString& direction)
{
    const bool fundsBitcoin = direction == QStringLiteral("taker_sells_foreign");
    if (nodeState == "not_activated" || nodeState == "initiating")
        return {"preparing", "Preparing the swap", 10,
                "Reservation, funding plan, signing ceremony and actor activation run inside your Node", "", ""};
    if (nodeState == "awaiting_first_lock") {
        if (fundsBitcoin && !locked)
            return {"lock_ready", "Your Bitcoin lock is ready", 20,
                    "Your move — Lock 0.01 BTC broadcasts the exact funding transaction your wallet signed",
                    "lock_btc", "Lock 0.01000000 BTC"};
        return {"locking_btc", "Bitcoin lock confirming", 35,
                "Your Node observes the lock; the Maker funds LEZ once it is confirmed", "", ""};
    }
    if (nodeState == "awaiting_second_lock")
        return {"awaiting_maker_lock", "Waiting for the Maker's LEZ escrow", 50,
                "The Maker's Node funds the escrow automatically after your lock confirms", "", ""};
    if (nodeState == "claim_available")
        return {"claim_ready", "Your LEZ claim is ready", 70,
                "Your move — Claim 1,000 LEZ reveals the adaptor secret the Maker needs for its Bitcoin claim",
                "claim_lez", "Claim 1,000 LEZ"};
    if (nodeState == "claim_in_progress")
        return {"claiming_lez", "LEZ claim submitted", 85,
                "Your Node observes the claim; the Maker's follow-up Bitcoin claim completes the swap", "", ""};
    if (nodeState == "completed")
        return {"completed", "Completed", 100, "Both legs settled on chain", "", ""};
    if (nodeState == "refund_available")
        return {"refund_ready", "Refund available", 60,
                "The Maker did not lock in time; you may recover your Bitcoin", "refund_btc", "Refund Bitcoin"};
    if (nodeState == "refund_in_progress")
        return {"refunding", "Refund submitted", 80, "Your Node observes the refund", "", ""};
    if (nodeState == "refunded")
        return {"refunded", "Refunded", 100, "Your Bitcoin came back", "", ""};
    return {"attention_required", "Needs attention", 0,
            "The actor reports a state the desk cannot advance; inspect it from the CLI", "", ""};
}

// The Maker's desk states from its supervised actor's observation.
SwapRow makerRow(const QString& phase, const QString& scheduleState)
{
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

QJsonObject swapRowObject(const SwapRow& row, const QString& swapId, const QString& offerId,
                          const QString& direction, const QString& makerLabel,
                          const QString& takerLabel, const QString& role, qint64 generation)
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

QJsonObject presetObject()
{
    return QJsonObject{{"bitcoin_sats", kFixedBitcoinSats}, {"bitcoin_display", formatBtc(kFixedBitcoinSats)},
                       {"lez_units", kFixedLezUnits}, {"lez_display", formatLez(kFixedLezUnits)},
                       {"direction", "BTC → LEZ"}};
}

QJsonArray directionCatalog()
{
    return QJsonArray{
        QJsonObject{{"direction", "taker_sells_foreign"}, {"display", "BTC → LEZ"},
                    {"ui_direction", "TakerSellsForeign"}, {"bitcoin_sats", kFixedBitcoinSats},
                    {"bitcoin_display", formatBtc(kFixedBitcoinSats)}, {"lez_units", kFixedLezUnits},
                    {"lez_display", formatLez(kFixedLezUnits)},
                    {"maker_label", "Sell 1,000 LEZ for 0.01000000 BTC"},
                    {"maker_actions", QJsonArray{"Fund 1,000 LEZ (automatic)", "Claim Bitcoin (automatic)"}}, {"taker_actions", QJsonArray{"Lock 0.01000000 BTC", "Claim 1,000 LEZ"}}},
        QJsonObject{{"direction", "taker_sells_lez"}, {"display", "LEZ → BTC"},
                    {"ui_direction", "TakerSellsLez"}, {"bitcoin_sats", kFixedBitcoinSats},
                    {"bitcoin_display", formatBtc(kFixedBitcoinSats)}, {"lez_units", kFixedLezUnits},
                    {"lez_display", formatLez(kFixedLezUnits)},
                    {"maker_label", "Sell 0.01000000 BTC for 1,000 LEZ"},
                    {"maker_actions", QJsonArray{}}, {"taker_actions", QJsonArray{"Claim 0.01 BTC"}}},
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
            const SwapRow row = takerRow(swap.value("state").toString(), lockedSwaps.contains(swapId), direction);
            swaps.append(swapRowObject(row, swapId, swap.value("offer_id").toString(), direction,
                                       QStringLiteral("Munich Vault 01"), wallet.label, QStringLiteral("taker"),
                                       static_cast<qint64>(swap.value("progress_generation").toDouble())));
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
        {"preset", presetObject()},
        {"directions", directionCatalog()},
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
                  const QSet<QString>& lockedSwaps)
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
    const qint64 sats = static_cast<qint64>(configuration.value("maximum_foreign_units").toDouble(kFixedBitcoinSats));
    const qint64 lez = quoteLez(offer.value("price").toObject(), sats);
    const Reply initiated = decode(slowRpc.call("taker_swap_initiate_v1", compact({
        {"schema_version", 1}, {"request_id", requestId}, {"offer_id", offerId},
        {"route", configuration.value("route")}, {"maker_identity", selected.value("maker_identity")},
        {"signed_envelope_sha256", selected.value("signed_envelope_sha256")},
        {"foreign_units", sats}, {"expected_lez_units", lez}})));
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

QJsonObject makerSnapshotObject(const LocalJsonRpcClient& rpc, const MakerWallet& wallet, Reply* error)
{
    QJsonArray inventory;
    int pending = 0;
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
            QString phase, schedule;
            const Reply monitored = decode(rpc.call("maker_actor_monitor_v1", compact({{"id", swapId}})));
            if (monitored.ok) {
                const QJsonObject result = monitored.result.toObject();
                schedule = result.value("schedule_state").toString();
                phase = result.value("progress").toObject().value("observation").toObject().value("phase").toString();
            }
            const SwapRow row = makerRow(phase, schedule);
            swaps.append(swapRowObject(row, swapId, QString(), direction, wallet.label,
                                       QStringLiteral("Zurich Wallet 01"), QStringLiteral("maker"), 0));
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
        {"preset", presetObject()},
        {"directions", directionCatalog()},
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
                     const QString& requestId, const QString& direction)
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
                                      {"minimum_foreign_units", kFixedBitcoinSats},
                                      {"maximum_foreign_units", kFixedBitcoinSats},
                                      {"offer_ttl_seconds", 3600}}},
        {"price", QJsonObject{{"route", route}, {"lez_units_per_lot", 1}, {"foreign_units_per_lot", 1000}}},
    };
    if (pairRevision >= 0) routeRequest.insert("expected_pair_revision", pairRevision);
    if (priceRevision >= 0) routeRequest.insert("expected_price_revision", priceRevision);
    const Reply saved = decode(rpc.call("maker_local_route_save_v1", compact(routeRequest)));
    if (!saved.ok) return nodeFailure(saved, QStringLiteral("The Bitcoin route could not be enabled"));
    const QString offerId = QStringLiteral("offer-%1-%2")
                                .arg(direction == QStringLiteral("taker_sells_lez") ? "sell-btc" : "sell-lez")
                                .arg(QDateTime::currentSecsSinceEpoch());
    const Reply published = decode(rpc.call("maker_offer_publish", compact({
        {"request_id", requestId}, {"offer_id", offerId}, {"route", route}})));
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
