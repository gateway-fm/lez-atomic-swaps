#pragma once

#include "rep_lez_zcash_node_source.h"
#include "logos_ui_plugin_context.h"

class LezZcashNodeBackend : public LezZcashNodeSimpleSource,
                            public LogosUiPluginContext
{
public:
    QString status() override;
    QString restartNode() override;
    QString resetChain() override;
    void onContextReady() override;

private:
    QString started_;
};
