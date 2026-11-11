#pragma once

#include <string>

#include "logos_module_context.h"

/**
 * @brief The Taker's Node payload: the package that carries the Taker Node.
 *
 * The package's variant holds, besides this plugin, `bin/`: the launcher
 * (`node-entrypoint.sh`, which renders the Node's configuration and execs
 * it), the Taker Node, its CLI, its registry initialiser, its Bitcoin and
 * Zcash actors, the LEZ v0.2 sidecar the Node runs per swap, the health
 * probe, and `payload.json`
 * (each file's sha256 and the sources it was built from). Installing the
 * Taker desk installs this package; the desk asks it for the launcher and
 * has lez_swap_node, the Node's one lifecycle owner, start the Node from it.
 * This module starts nothing itself.
 */
class LezTakerNodeImpl : public LogosModuleContext {
public:
    /// The installed launcher's absolute path; empty when this package has
    /// no executable launcher (a damaged install).
    std::string launcher();

    /// The installed payload's `bin/payload.json`; empty when absent.
    std::string payload();
};
