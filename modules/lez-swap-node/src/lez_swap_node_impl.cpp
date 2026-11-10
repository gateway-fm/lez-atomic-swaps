#include "lez_swap_node_impl.h"

#include <atomic>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <thread>

#include <fcntl.h>
#include <signal.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

namespace {
    // Module diagnostics go to stderr; logos_host routes each line through its
    // logger, classifying by an "Error:"/"Warning:" token (plain lines are info).
    void info(const std::string& role, const std::string& msg) {
        std::fprintf(stderr, "lez_swap_node: %s: %s\n", role.c_str(), msg.c_str());
    }
    void warn(const std::string& role, const std::string& msg) {
        std::fprintf(stderr, "lez_swap_node: Warning: %s: %s\n", role.c_str(), msg.c_str());
    }

    constexpr auto RESTART_DELAY = std::chrono::seconds(2);
    constexpr auto HOLD_POLL = std::chrono::seconds(1);
    constexpr auto STOP_GRACE = std::chrono::seconds(10);

    bool exists(const std::string& path) {
        struct stat st {};
        return !path.empty() && ::stat(path.c_str(), &st) == 0;
    }

    std::string describe(int status) {
        if (WIFEXITED(status)) return "exit " + std::to_string(WEXITSTATUS(status));
        if (WIFSIGNALED(status)) return "signal " + std::to_string(WTERMSIG(status));
        return "status " + std::to_string(status);
    }

    // One supervised role. `pid` is the running Node's process (group leader),
    // 0 while none runs; everything else is guarded by `mutex`.
    struct Role {
        std::string name, launcher, log_file, hold_file;
        std::atomic<pid_t> pid{0};
        std::mutex mutex;
        std::condition_variable wake;
        bool stopping = false;
        std::string state = "restarting";
        uint64_t starts = 0;
        std::string last_exit;
        std::thread thread;

        // fork + exec, with only async-signal-safe calls in the child: every
        // string the child needs is prepared before the fork.
        pid_t spawn() {
            const char* argv[] = {launcher.c_str(), name.c_str(), nullptr};
            const pid_t child = ::fork();
            if (child != 0) return child;
            ::setpgid(0, 0);
            const int out = ::open(log_file.c_str(), O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0640);
            const int in = ::open("/dev/null", O_RDONLY | O_CLOEXEC);
            if (out >= 0) { ::dup2(out, STDOUT_FILENO); ::dup2(out, STDERR_FILENO); }
            if (in >= 0) ::dup2(in, STDIN_FILENO);
            ::execv(argv[0], const_cast<char* const*>(argv));
            ::_exit(127);
        }

        void run() {
            std::unique_lock lock(mutex);
            while (!stopping) {
                if (exists(hold_file)) {
                    state = "held";
                    wake.wait_for(lock, HOLD_POLL);
                    continue;
                }
                const pid_t child = spawn();
                if (child < 0) {
                    last_exit = std::string("fork failed: ") + std::strerror(errno);
                    warn(name, last_exit);
                } else {
                    // The parent sets the group too, so a kill(-pid) right after
                    // the fork cannot miss a child that has not run setpgid yet.
                    ::setpgid(child, child);
                    pid = child;
                    ++starts;
                    state = "running";
                    info(name, "Node started (pid " + std::to_string(child) + ", start " + std::to_string(starts) + ")");
                    lock.unlock();
                    int status = 0;
                    while (::waitpid(child, &status, 0) < 0 && errno == EINTR) {}
                    lock.lock();
                    pid = 0;
                    last_exit = describe(status);
                    info(name, "Node ended (" + last_exit + ")");
                }
                if (stopping) break;
                state = "restarting";
                wake.wait_for(lock, RESTART_DELAY, [this] { return stopping; });
            }
            state = "stopped";
        }

        void stop() {
            {
                std::lock_guard lock(mutex);
                if (stopping) return;
                stopping = true;
            }
            wake.notify_all();
            if (const pid_t child = pid; child > 0) {
                ::kill(-child, SIGTERM);
                const auto deadline = std::chrono::steady_clock::now() + STOP_GRACE;
                while (pid != 0 && std::chrono::steady_clock::now() < deadline) {
                    std::this_thread::sleep_for(std::chrono::milliseconds(100));
                }
                if (const pid_t left = pid; left > 0) ::kill(-left, SIGKILL);
            }
            if (thread.joinable()) thread.join();
        }
    };

    struct Supervisor {
        std::mutex mutex;
        std::map<std::string, std::shared_ptr<Role>> roles;
    };

    inline Supervisor& supervisor_of(void*& p) {
        if (p == nullptr) p = new Supervisor();
        return *static_cast<Supervisor*>(p);
    }

    bool valid_role(const std::string& role) {
        if (role.empty() || role.size() > 32) return false;
        for (const char c : role) {
            if (c < 'a' || c > 'z') return false;
        }
        return true;
    }

    std::string json_string(const std::string& s) {
        std::string out = "\"";
        for (const char c : s) {
            if (c == '"' || c == '\\') { out += '\\'; out += c; }
            else if (static_cast<unsigned char>(c) < 0x20) out += ' ';
            else out += c;
        }
        return out + "\"";
    }
} // namespace

LezSwapNodeImpl::~LezSwapNodeImpl() {
    if (supervisor == nullptr) return;
    auto* s = static_cast<Supervisor*>(supervisor);
    for (auto& [name, role] : s->roles) role->stop();
    delete s;
}

int64_t LezSwapNodeImpl::start_node(const std::string& role, const std::string& launcher, const std::string& log_file, const std::string& hold_file) {
    if (!valid_role(role) || launcher.empty() || launcher[0] != '/' || ::access(launcher.c_str(), X_OK) != 0 ||
        log_file.empty() || log_file[0] != '/' || hold_file.empty() || hold_file[0] != '/') {
        warn(role, "start_node: invalid arguments");
        return 1;
    }
    auto& s = supervisor_of(supervisor);
    std::lock_guard lock(s.mutex);
    if (auto it = s.roles.find(role); it != s.roles.end()) {
        std::lock_guard role_lock(it->second->mutex);
        if (!it->second->stopping) return 0;
    }
    auto supervised = std::make_shared<Role>();
    supervised->name = role;
    supervised->launcher = launcher;
    supervised->log_file = log_file;
    supervised->hold_file = hold_file;
    supervised->thread = std::thread([r = supervised.get()] { r->run(); });
    if (auto old = s.roles.find(role); old != s.roles.end() && old->second->thread.joinable()) old->second->thread.join();
    s.roles[role] = supervised;
    info(role, "supervising " + launcher + " (log " + log_file + ", hold file " + hold_file + ")");
    return 0;
}

int64_t LezSwapNodeImpl::stop_node(const std::string& role) {
    std::shared_ptr<Role> supervised;
    {
        auto& s = supervisor_of(supervisor);
        std::lock_guard lock(s.mutex);
        if (auto it = s.roles.find(role); it != s.roles.end()) supervised = it->second;
    }
    if (supervised) supervised->stop();
    return 0;
}

std::string LezSwapNodeImpl::node_status(const std::string& role) {
    std::shared_ptr<Role> supervised;
    {
        auto& s = supervisor_of(supervisor);
        std::lock_guard lock(s.mutex);
        if (auto it = s.roles.find(role); it != s.roles.end()) supervised = it->second;
    }
    if (!supervised) return "";
    std::lock_guard lock(supervised->mutex);
    return "{\"role\":" + json_string(supervised->name) + ",\"state\":" + json_string(supervised->state) +
           ",\"pid\":" + std::to_string(supervised->pid.load()) + ",\"starts\":" + std::to_string(supervised->starts) +
           ",\"last_exit\":" + json_string(supervised->last_exit) + "}";
}
