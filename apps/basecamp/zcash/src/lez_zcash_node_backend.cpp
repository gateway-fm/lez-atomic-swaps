#include "lez_zcash_node_backend.h"
#include "logos_sdk.h"

#include "lez_zcash_host.h"

void LezZcashNodeBackend::onContextReady()
{
    // The Zcash node this Basecamp hosts (lez_zcash_host.h): configured and
    // started when the stack names its network.
    started_ = lez_zcash_host::start(modules().zebrad_module);
}

QString LezZcashNodeBackend::status()
{
    // What start reported, until the node has a status of its own.
    const QString node = lez_zcash_host::status(modules().zebrad_module);
    return node.contains(QStringLiteral("\"state\"")) ? node : started_;
}

QString LezZcashNodeBackend::resetChain()
{
    // A new Regtest chain under the same settings (a worn chain, or one on
    // another consensus branch: tests/fixtures/zec-catalog-fixture.sh).
    started_ = lez_zcash_host::resetChain(modules().zebrad_module);
    return started_;
}
