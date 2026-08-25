#pragma once

#include "rep_lez_atomic_swap_taker_source.h"
#include "logos_ui_plugin_context.h"
#include "local_json_rpc_client.h"
#include "logos_chat_bridge.h"

#include <memory>

class LezAtomicSwapTakerBackend : public LezAtomicSwapTakerSimpleSource,
                                  public LogosUiPluginContext
{
public:
    LezAtomicSwapTakerBackend();
    ~LezAtomicSwapTakerBackend() override;
    QString health() override;
    QString chatStatus() override;
    QString connectChat(QString peerAddress) override;
    QString resetChat() override;
    QString btcEvidence() override;
    QString btcMarket(QString walletId) override;
    QString btcTakeOffer(QString requestId, QString walletId, QString offerId) override;
    QString btcSwapAction(QString requestId, QString walletId, QString swapId,
                          QString action) override;
    QString listOffers(QString pair, QString direction) override;
    QString initiate(QString requestId, QString offerId, QString pair, QString direction,
                     QString makerIdentity, QString signedEnvelopeSha256, QString foreignUnits,
                     QString expectedLezUnits) override;
    QString listSwaps() override;
    QString monitor(QString swapId) override;
    QString claim(QString requestId, QString swapId, QString expectedGeneration) override;
    QString refund(QString requestId, QString swapId, QString expectedGeneration) override;
    void onContextReady() override;

private:
    LocalJsonRpcClient rpc_;
    LocalJsonRpcClient demoRpc_;
    std::unique_ptr<LogosChatBridge> chat_;
};
