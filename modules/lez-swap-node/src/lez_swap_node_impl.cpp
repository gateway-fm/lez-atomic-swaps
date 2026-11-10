#include "lez_swap_node_impl.h"

#include "logos_sdk.h"

#include <QCoreApplication>
#include <QMetaObject>

#include <algorithm>
#include <atomic>
#include <cerrno>
#include <chrono>
#include <cctype>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <functional>
#include <future>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <thread>

#include <fcntl.h>
#include <signal.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/un.h>
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
    constexpr auto SIGN_TIMEOUT = std::chrono::seconds(30);

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

    bool hex64(const std::string& s) {
        return s.size() == 64 && std::all_of(s.begin(), s.end(), [](unsigned char c) { return std::isxdigit(c) != 0; });
    }

    // The string value of `"key":"…"` in a flat JSON object of hex strings (the
    // whole signer protocol); empty when absent.
    std::string field(const std::string& json, const std::string& key) {
        const auto at = json.find('"' + key + '"');
        if (at == std::string::npos) return {};
        const auto colon = json.find(':', at + key.size() + 2);
        const auto open = colon == std::string::npos ? colon : json.find('"', colon);
        const auto close = open == std::string::npos ? open : json.find('"', open + 1);
        return close == std::string::npos ? std::string() : json.substr(open + 1, close - open - 1);
    }

    // One signer socket: `account`'s signatures for whoever may open the
    // owner-only socket (the role's Node, same user). `sign` asks the wallet.
    // The thread shares ownership, so a thread stop() leaves behind (below)
    // never outlives the state it reads.
    struct Signer : std::enable_shared_from_this<Signer> {
        std::string socket_path, account;
        std::function<std::string(const std::string&)> sign;
        int listener = -1;
        std::atomic<bool> stopping{false};
        std::thread thread;
        std::mutex mutex;
        std::condition_variable ended;
        bool running = false;

        ~Signer() {
            if (listener >= 0) ::close(listener);
        }

        // One signing request per connection: read one line, answer one line, close.
        void answer(int connection) const {
            timeval timeout{10, 0};
            ::setsockopt(connection, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
            ::setsockopt(connection, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout));
            std::string line;
            char byte = 0;
            while (line.size() < 1024 && ::read(connection, &byte, 1) == 1 && byte != '\n') line.push_back(byte);
            std::string requested = field(line, "account_id");
            std::transform(requested.begin(), requested.end(), requested.begin(), [](unsigned char c) { return std::tolower(c); });
            const std::string hash = field(line, "message_hash");
            std::string reply;
            if (!hex64(requested) || !hex64(hash)) {
                reply = R"({"error":"the request must be {\"account_id\", \"message_hash\"}, 64 hex characters each"})";
            } else if (requested != account) {
                reply = R"({"error":"this signer holds another account"})";
            } else if (stopping) {
                reply = R"({"error":"the signer is stopping"})";
            } else {
                reply = sign(hash);
                if (reply.empty() || reply.front() != '{') reply = R"({"error":"the wallet did not sign"})";
            }
            reply.push_back('\n');
            (void)!::write(connection, reply.data(), reply.size());
            ::close(connection);
        }

        void start() {
            running = true;
            thread = std::thread([self = shared_from_this()] {
                while (!self->stopping) {
                    const int connection = ::accept4(self->listener, nullptr, nullptr, SOCK_CLOEXEC);
                    if (connection >= 0) self->answer(connection);
                    else if (errno != EINTR && errno != ECONNABORTED) break;
                }
                std::lock_guard lock(self->mutex);
                self->running = false;
                self->ended.notify_all();
            });
        }

        // A signature in flight waits for this module's main thread, which
        // may be the thread stopping this signer: wait a little for the
        // thread, then leave it (it ends by itself within SIGN_TIMEOUT).
        void stop() {
            stopping = true;
            if (listener >= 0) ::shutdown(listener, SHUT_RDWR);
            ::unlink(socket_path.c_str());
            std::unique_lock lock(mutex);
            const bool ended_in_time = ended.wait_for(lock, std::chrono::seconds(2), [this] { return !running; });
            lock.unlock();
            if (ended_in_time) thread.join();
            else thread.detach();
        }
    };

    struct Supervisor {
        std::mutex mutex;
        std::map<std::string, std::shared_ptr<Role>> roles;
        std::map<std::string, std::shared_ptr<Signer>> signers;
        // False once the module is going away; read and written on the main thread only.
        std::shared_ptr<bool> alive = std::make_shared<bool>(true);
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
    *s->alive = false;
    for (auto& [name, role] : s->roles) role->stop();
    for (auto& [path, signer] : s->signers) signer->stop();
    delete s;
}

int64_t LezSwapNodeImpl::serve_signer(const std::string& socket_path, const std::string& account_id_hex) {
    std::string account = account_id_hex;
    std::transform(account.begin(), account.end(), account.begin(), [](unsigned char c) { return std::tolower(c); });
    if (!hex64(account) || socket_path.empty() || socket_path[0] != '/' || socket_path.size() >= sizeof(sockaddr_un::sun_path)) {
        warn("signer", "serve_signer: an invalid account id or an unusable socket path");
        return 1;
    }
    auto& s = supervisor_of(supervisor);
    std::lock_guard lock(s.mutex);
    if (s.signers.count(socket_path) != 0) return s.signers[socket_path]->account == account ? 0 : 1;
    auto signer = std::make_shared<Signer>();
    signer->socket_path = socket_path;
    signer->account = account;
    // The wallet is another module of this Basecamp, and a call to another
    // module works only from this module's own (main) thread: from the signer
    // thread, synchronous or through the generated async client, it found no
    // lez_core (object_unavailable after 40 s on the devnet). So the signer
    // thread posts each call to the main thread and waits for its answer; a
    // call still queued when the module goes away is answered empty.
    signer->sign = [this, account, alive = s.alive](const std::string& hash) {
        auto answer = std::make_shared<std::promise<std::string>>();
        auto signature = answer->get_future();
        const bool posted = QMetaObject::invokeMethod(
            QCoreApplication::instance(),
            [this, account, hash, alive, answer] {
                answer->set_value(*alive ? modules().lez_core.sign_public_message_hash(account, hash) : std::string());
            },
            Qt::QueuedConnection);
        if (!posted || signature.wait_for(SIGN_TIMEOUT) != std::future_status::ready) return std::string();
        return signature.get();
    };
    signer->listener = ::socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (signer->listener < 0) return 1;
    sockaddr_un address{};
    address.sun_family = AF_UNIX;
    std::memcpy(address.sun_path, socket_path.c_str(), socket_path.size());
    ::unlink(socket_path.c_str());
    const mode_t previous = ::umask(077);
    const bool bound = ::bind(signer->listener, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0 &&
                       ::listen(signer->listener, 16) == 0;
    ::umask(previous);
    if (!bound) {
        warn("signer", "serve_signer: cannot listen on " + socket_path);
        return 1;
    }
    signer->start();
    s.signers.emplace(socket_path, std::move(signer));
    info("signer", "signing for " + account + " on " + socket_path);
    return 0;
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
