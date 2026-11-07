#include "lez_network_backend.h"
#include "logos_sdk.h"

#include "lez_network_host.h"

void LezNetworkBackend::onContextReady()
{
    // The LEZ network node and/or the LEZ indexer this Basecamp hosts
    // (lez_network_host.h): each starts when the stack configures it.
    status_ = lez_network_host::start(modules().blockchain_module, modules().lez_indexer_module);
}

QString LezNetworkBackend::status()
{
    return status_;
}
