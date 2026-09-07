// The BTC desk's market view, built from the two Nodes' own methods.
//
// The desks used to read a "wallet market" from the local demo controller,
// which delegated every step to an external runner. Both roles now settle a
// swap themselves (ADR 0213), so this adapter assembles the same view model
// (wallets, order book / inventory, swaps with the one action the desk may
// take) from `taker_*` and `maker_*` calls, and performs the desk's actions
// through them. Nothing here holds keys or signs anything.
#pragma once

#include <QJsonObject>
#include <QSet>
#include <QString>

class LocalJsonRpcClient;

namespace node_market {

// A decoded backend envelope: the `result` of an `{ok:true}` reply, or a code
// and message the desk can show.
struct Reply {
    bool ok = false;
    QJsonValue result;
    QString code;
    QString message;
};
Reply decode(const QString& envelope);
QString failure(const QString& code, const QString& message);
QString success(const QJsonValue& result);

// Taker desk --------------------------------------------------------------
struct TakerWallet {
    QString id;
    QString label;
};
QString takerSnapshot(const LocalJsonRpcClient& rpc, const TakerWallet& wallet,
                      const QSet<QString>& lockedSwaps);
// Takes `offerId` through `taker_swap_initiate_v1` (long-running: reservation,
// funding plan, ceremony, actor activation) and returns the refreshed snapshot.
QString takerTake(const LocalJsonRpcClient& rpc, const LocalJsonRpcClient& slowRpc,
                  const TakerWallet& wallet, const QString& requestId, const QString& offerId,
                  const QSet<QString>& lockedSwaps);
// `lock_btc` broadcasts the funding transaction; `claim_lez` claims against the
// generation the desk last saw; `refund_btc` asks for the recovery path.
QString takerAction(const LocalJsonRpcClient& rpc, const LocalJsonRpcClient& slowRpc,
                    const TakerWallet& wallet, const QString& requestId, const QString& swapId,
                    const QString& action, QSet<QString>& lockedSwaps);

// Maker desk --------------------------------------------------------------
struct MakerWallet {
    QString id;
    QString label;
};
QString makerSnapshot(const LocalJsonRpcClient& rpc, const MakerWallet& wallet);
// Enables the Bitcoin route at the fixed local preset and publishes one offer.
QString makerPublish(const LocalJsonRpcClient& rpc, const MakerWallet& wallet,
                     const QString& requestId, const QString& direction);
QString makerWithdraw(const LocalJsonRpcClient& rpc, const MakerWallet& wallet,
                      const QString& requestId, const QString& offerId);

} // namespace node_market
