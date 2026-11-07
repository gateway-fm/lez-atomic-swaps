#pragma once

#include <cstdint>
#include <string>

#include "logos_module_context.h"

/**
 * @brief The swap engine inside Basecamp: launches each swap role's Node and
 * keeps it running.
 *
 * Universal authoring model: this impl class is the whole module; its public
 * methods are the API (the desks call them through Basecamp). The Qt plugin
 * glue is generated from this header by logos-module-builder.
 *
 * A role's Node is `<launcher> <role>` (the Node entrypoint), run in its own
 * process group with its output appended to the role's log file. When it
 * exits, it is started again after two seconds, unless the role's hold file
 * exists: then it stays down until the file is removed. That is how a test
 * takes a Node offline without stopping Basecamp.
 *
 * Module code is Qt-free. The supervisor state is opaque here (the universal
 * codegen parses this header) and lives in the .cpp.
 */
class LezSwapNodeImpl : public LogosModuleContext {
public:
    ~LezSwapNodeImpl();

    /// Start supervising `role`'s Node: `launcher` (absolute, executable) run
    /// with `role` as its one argument, output appended to `log_file`, held
    /// down while `hold_file` exists. Idempotent: a role already supervised is
    /// left as it is. Returns 0 when supervised, else non-zero (1 invalid
    /// arguments). int64_t: the universal codegen marshals it as a scalar.
    // clang-format off
    int64_t start_node(const std::string& role, const std::string& launcher, const std::string& log_file, const std::string& hold_file);
    // clang-format on

    /// Stop `role`'s Node (SIGTERM to its process group, SIGKILL after 10 s)
    /// and stop supervising it. Returns 0, also for a role not supervised.
    int64_t stop_node(const std::string& role);

    /// `role`'s supervision as compact JSON: {role, state (running, held,
    /// restarting, stopped), pid, starts, last_exit}; empty when the role was
    /// never started.
    std::string node_status(const std::string& role);

private:
    // Supervisor* — see the .cpp.
    void* supervisor = nullptr;
};
