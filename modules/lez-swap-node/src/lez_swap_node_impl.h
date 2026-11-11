#pragma once

#include <cstdint>
#include <string>

#include "logos_module_context.h"

/**
 * @brief The swap engine inside Basecamp: launches each swap role's Node,
 * keeps it running, and relays its LEZ signing requests to Basecamp's wallet.
 *
 * Universal authoring model: this impl class is the whole module; its public
 * methods are the API (the desks call them through Basecamp). The Qt plugin
 * glue is generated from this header by logos-module-builder.
 *
 * A role's Node is `<launcher> <role>` (the launcher the role's payload
 * package, lez_maker_node or lez_taker_node, installs beside the Node), run
 * in its own process group with its output appended to the role's log file.
 * When it exits, the rest of its group (loopback forwards, sidecars) is
 * ended and it is started again after two seconds, unless the role's hold
 * file exists: then it stays down until the file is removed. That is how a
 * test takes a Node offline without stopping Basecamp.
 *
 * This module is the Node's one lifecycle owner. The Node dies with the
 * module's host (the kernel signals it when the host goes: module unload or
 * reload, Basecamp exit, a crash), and each Node's process group is recorded
 * under the module's instance directory, so a group a dead host left
 * behind is ended before a Node is started again: never two Nodes. What the
 * desks asked for (start_node, serve_signer) is recorded there too and
 * resumed when the module is loaded again, so a reload restarts the role's
 * signer and Node; stop_node forgets the role.
 *
 * The header is Qt-free (the universal codegen parses it); the supervisor
 * state is opaque here and lives in the .cpp, which uses QtCore only to run
 * the signer's calls to lez_core on the module's main thread.
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

    /// Stop `role`'s Node (SIGTERM to its process group, SIGKILL after 10 s),
    /// stop supervising it and forget it (a reload does not start it again).
    /// Returns 0, also for a role not supervised.
    int64_t stop_node(const std::string& role);

    /// `role`'s supervision as compact JSON: {role, state (running, held,
    /// restarting, stopped), pid, starts, last_exit}; empty when the role was
    /// never started.
    std::string node_status(const std::string& role);

    /// `role`'s Node's resource use as compact JSON, read from /proc: the
    /// supervision (role, state, pid, uptime_seconds, starts, restarts,
    /// last_exit); `processes`, one object per member of the Node's process
    /// group (the Node first, then its forwards and sidecars): pid, ppid,
    /// name, state, cpu_percent, cpu_time_seconds, memory_mb (VmRSS),
    /// threads, fds; `group`, their sums; `host`, the same for this module's
    /// own host process. cpu_percent is measured since this role's previous
    /// node_stats call, or over the process's life on the first. Empty when
    /// the role was never started. The runtime's `module stats` samples only
    /// the module host's pid, so this is where the Node's own use is seen.
    std::string node_stats(const std::string& role);

    /// Serve `account_id_hex`'s LEZ signatures to the role's Node on the
    /// owner-only Unix socket `socket_path`: one JSON line per connection,
    /// {"account_id", "message_hash"} in, the wallet's {"signature",
    /// "public_key"} (or {"error"}) out. Every signature is made by Basecamp's
    /// LEZ wallet (lez_core, sign_public_message_hash); the key never reaches
    /// this module or the Node. Only that one account is served, and a socket
    /// already served is left as it is. Returns 0 when serving, else non-zero.
    int64_t serve_signer(const std::string& socket_path, const std::string& account_id_hex);

protected:
    /// Resumes the recorded signers and Nodes (see the class comment).
    void onContextReady() override;

private:
    // Supervisor* — see the .cpp.
    void* supervisor = nullptr;
};
