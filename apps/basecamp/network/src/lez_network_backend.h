#pragma once

#include "rep_lez_network_source.h"
#include "logos_ui_plugin_context.h"

class LezNetworkBackend : public LezNetworkSimpleSource,
                          public LogosUiPluginContext
{
public:
    QString status() override;
    QString reindex() override;
    void onContextReady() override;

private:
    QString status_;
};
