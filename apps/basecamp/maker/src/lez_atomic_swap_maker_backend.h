#pragma once

#include "rep_lez_atomic_swap_maker_source.h"
#include "logos_ui_plugin_context.h"
#include "local_json_rpc_client.h"

class LezAtomicSwapMakerBackend : public LezAtomicSwapMakerSimpleSource,
                                  public LogosUiPluginContext
{
public:
    LezAtomicSwapMakerBackend();
    QString health() override;
    QString saveRoute(QString requestId, QString pair, QString direction,
                      QString minimumForeignUnits, QString maximumForeignUnits,
                      QString offerTtlSeconds, QString lezUnitsPerLot,
                      QString foreignUnitsPerLot) override;
    QString history() override;
    QString monitor(QString swapId) override;
    QString claim(QString requestId, QString swapId, QString expectedGeneration) override;
    QString refund(QString requestId, QString swapId, QString expectedGeneration) override;
    void onContextReady() override {}

private:
    LocalJsonRpcClient rpc_;
};
