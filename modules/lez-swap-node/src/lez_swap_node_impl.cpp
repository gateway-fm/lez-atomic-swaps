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
#include <dirent.h>
#include <fstream>
#include <functional>
#include <future>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <sstream>
#include <thread>
#include <vector>

#include <fcntl.h>
#include <signal.h>
#include <sys/file.h>
#if defined(__APPLE__)
#include <crt_externs.h>
#include <libproc.h>
#include <mach/mach_time.h>
#include <poll.h>
#include <sys/event.h>
#include <sys/sysctl.h>
#else
#include <sys/prctl.h>
#endif
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

#if defined(__APPLE__)
// A dylib on macOS has no `environ` symbol to link against.
#define environ (*_NSGetEnviron())
#else
extern char** environ;
#endif

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

#if defined(__APPLE__)
    // macOS has no /proc: the same two questions are asked of libproc and
    // sysctl, which answer them for the caller's own processes.

    // The members of process group `group`, as the kernel lists them; each is
    // asked for its group again (the list is a snapshot and may hold zeroes).
    // With `running_only`, not those already ended and waiting to be reaped
    // (which libproc does not list in the first place).
    std::vector<pid_t> members_of(pid_t group, bool running_only = false) {
        std::vector<pid_t> members;
        const int wanted = ::proc_listpids(PROC_PGRP_ONLY, static_cast<uint32_t>(group), nullptr, 0);
        if (wanted <= 0) return members;
        std::vector<pid_t> listed(static_cast<size_t>(wanted) / sizeof(pid_t) + 64);
        const int bytes = ::proc_listpids(PROC_PGRP_ONLY, static_cast<uint32_t>(group), listed.data(),
                                          static_cast<int>(listed.size() * sizeof(pid_t)));
        for (size_t i = 0; bytes > 0 && i < static_cast<size_t>(bytes) / sizeof(pid_t); ++i) {
            proc_bsdinfo process{};
            if (listed[i] > 0 &&
                ::proc_pidinfo(listed[i], PROC_PIDTBSDINFO, 0, &process, sizeof(process)) == static_cast<int>(sizeof(process)) &&
                static_cast<pid_t>(process.pbi_pgid) == group && !(running_only && process.pbi_status == SZOMB)) {
                members.push_back(listed[i]);
            }
        }
        return members;
    }

    // Whether process `pid` belongs to a Node this module started: it carries
    // the role's `marker` in the environment it was executed with, or its
    // executable lies in the payload `directory`, or one of its arguments
    // names a file there. sysctl KERN_PROCARGS2 gives the executable, the
    // arguments, then the environment of a process of the same user, but no
    // environment for the system's own binaries (/bin/bash, /bin/sleep):
    // the launcher run by the system's shell is known by its argument, and
    // that is the one test Linux does not make. Paths come back with their
    // links resolved (/var is /private/var here), so the directory is
    // compared both as given and resolved.
    bool started_by_us(pid_t pid, const std::string& marker, const std::string& directory) {
        std::string resolved;
        if (char buffer[PATH_MAX]; !directory.empty() && ::realpath(directory.c_str(), buffer) != nullptr) resolved = buffer;
        const auto in_payload = [&](const char* path, size_t length) {
            const std::string file(path, length);
            return (!directory.empty() && file.rfind(directory + "/", 0) == 0) ||
                   (!resolved.empty() && file.rfind(resolved + "/", 0) == 0);
        };
        int limit_name[2] = {CTL_KERN, KERN_ARGMAX};
        int limit = 0;
        size_t limit_size = sizeof(limit);
        if (::sysctl(limit_name, 2, &limit, &limit_size, nullptr, 0) != 0 || limit <= 0) limit = 1 << 20;
        std::vector<char> image(static_cast<size_t>(limit));
        int name[3] = {CTL_KERN, KERN_PROCARGS2, pid};
        size_t size = image.size();
        int arguments = 0;
        if (::sysctl(name, 3, image.data(), &size, nullptr, 0) == 0 && size > sizeof(arguments)) {
            std::memcpy(&arguments, image.data(), sizeof(arguments));
            const char* at = image.data() + sizeof(arguments);
            const char* const end = image.data() + size;
            const auto length = [&at, end] { return ::strnlen(at, static_cast<size_t>(end - at)); };
            const auto next = [&at, end, &length] {
                at += length();
                if (at < end) ++at;
            };
            next(); // the executable's path, then padding to a word
            while (at < end && *at == '\0') ++at;
            for (int i = 0; i < arguments && at < end; ++i, next()) {
                if (in_payload(at, length())) return true;
            }
            for (; at < end && *at != '\0'; next()) {
                if (marker.compare(0, std::string::npos, at, length()) == 0) return true;
            }
        }
        char exe[PROC_PIDPATHINFO_MAXSIZE];
        const int length = ::proc_pidpath(pid, exe, sizeof(exe));
        return length > 0 && in_payload(exe, static_cast<size_t>(length));
    }

    // Close this process's descriptors from `lowest` up, between fork and
    // exec: `list` (sized before the fork, nothing may allocate here) takes
    // the kernel's list of them; without it every possible number is closed.
    void close_descriptors(proc_fdinfo* list, int capacity, int lowest) {
        const int bytes = ::proc_pidinfo(::getpid(), PROC_PIDLISTFDS, 0, list, capacity);
        if (bytes > 0 && bytes < capacity) {
            for (size_t i = 0; i < static_cast<size_t>(bytes) / sizeof(proc_fdinfo); ++i) {
                if (list[i].proc_fd >= lowest) ::close(list[i].proc_fd);
            }
            return;
        }
        for (int fd = lowest, limit = ::getdtablesize(); fd < limit; ++fd) ::close(fd);
    }

    // What PR_SET_PDEATHSIG does on Linux, as a process: forked by the Node's
    // process before it executes the launcher, so no Node exists without one.
    // It waits on the kernel's exit notification (kqueue EVFILT_PROC) for
    // `host` and for the Node, and when the host goes it ends the Node's
    // whole process group (SIGTERM, SIGKILL to what is left after STOP_GRACE).
    // A host already gone when the watch is set cannot be watched (ESRCH):
    // that ends the group too, so there is no window. When the Node goes
    // first the host ends the rest of the group itself; the watch stays until
    // the group is empty, then this process exits. It sits in a session of
    // its own, outside the Node's group (the host's end_group neither signals
    // nor waits for it), holds none of the host's descriptors (a copy of the
    // role lock would keep the next host waiting), and makes only system
    // calls: it is the fork of a threaded process.
    [[noreturn]] void watch_host(pid_t host, pid_t node) {
        ::setsid();
        for (int fd = 0; fd < 3; ++fd) ::close(fd);
        const int queue = ::kqueue();
        struct kevent watch {};
        EV_SET(&watch, host, EVFILT_PROC, EV_ADD, NOTE_EXIT, 0, nullptr);
        bool host_gone = queue < 0 || ::kevent(queue, &watch, 1, nullptr, 0, nullptr) != 0;
        EV_SET(&watch, node, EVFILT_PROC, EV_ADD, NOTE_EXIT, 0, nullptr);
        bool node_gone = !host_gone && ::kevent(queue, &watch, 1, nullptr, 0, nullptr) != 0;
        while (!host_gone) {
            // Once the Node is gone: look at its group five times a second.
            if (node_gone && ::kill(-node, 0) != 0) ::_exit(0);
            const timespec soon{0, 200 * 1000 * 1000};
            struct kevent event {};
            const int events = ::kevent(queue, nullptr, 0, &event, 1, node_gone ? &soon : nullptr);
            if (events < 0 && errno != EINTR) host_gone = true;
            if (events == 1 && event.ident == static_cast<uintptr_t>(host)) host_gone = true;
            if (events == 1 && event.ident == static_cast<uintptr_t>(node)) node_gone = true;
        }
        if (::kill(-node, SIGTERM) == 0) {
            const int checks = static_cast<int>(std::chrono::seconds(STOP_GRACE).count()) * 10;
            for (int i = 0; i < checks && ::kill(-node, 0) == 0; ++i) ::usleep(100 * 1000);
            ::kill(-node, SIGKILL);
        }
        ::_exit(0);
    }

    // macOS has neither accept4 nor SOCK_CLOEXEC; a descriptor a concurrent
    // spawn() copies before this runs is closed there (close_descriptors).
    int close_on_exec(int fd) {
        if (fd >= 0) ::fcntl(fd, F_SETFD, FD_CLOEXEC);
        return fd;
    }
#else
    // The members of process group `group`: every process whose stat names it;
    // with `running_only`, not those already ended and waiting to be reaped.
    std::vector<pid_t> members_of(pid_t group, bool running_only = false) {
        std::vector<pid_t> members;
        DIR* proc = ::opendir("/proc");
        if (proc == nullptr) return members;
        while (const dirent* entry = ::readdir(proc)) {
            const pid_t pid = static_cast<pid_t>(std::atoi(entry->d_name));
            if (pid <= 0) continue;
            std::ifstream stat("/proc/" + std::string(entry->d_name) + "/stat");
            std::string line;
            std::getline(stat, line);
            // pid (comm) state ppid pgrp ...: comm may hold spaces and parentheses.
            const auto close = line.rfind(')');
            if (close == std::string::npos) continue;
            std::istringstream fields(line.substr(close + 1));
            std::string state;
            long ppid = 0, pgrp = 0;
            if (fields >> state >> ppid >> pgrp && pgrp == group && !(running_only && state == "Z")) members.push_back(pid);
        }
        ::closedir(proc);
        return members;
    }

    // Whether process `pid` belongs to a Node this module started: it carries
    // the role's `marker` in its environment (every descendant of the Node
    // inherits it), or its executable lies in the payload `directory`.
    bool started_by_us(pid_t pid, const std::string& marker, const std::string& directory) {
        std::ifstream environment("/proc/" + std::to_string(pid) + "/environ");
        std::string entry;
        while (std::getline(environment, entry, '\0')) {
            if (entry == marker) return true;
        }
        char exe[4096];
        const ssize_t length = ::readlink(("/proc/" + std::to_string(pid) + "/exe").c_str(), exe, sizeof(exe) - 1);
        return !directory.empty() && length > 0 && std::string(exe, static_cast<size_t>(length)).rfind(directory + "/", 0) == 0;
    }
#endif

    // End process group `group`: SIGTERM, SIGKILL to whatever is left after
    // STOP_GRACE. Returns once no member runs or after the SIGKILL. A member
    // that ended stays in the group until its parent reaps it, and an orphan's
    // parent is whatever init the host runs under, which may never reap (a
    // container whose pid 1 is not an init): such zombies do not count.
    void end_group(pid_t group) {
        if (group <= 0 || ::kill(-group, SIGTERM) != 0) return;
        const auto deadline = std::chrono::steady_clock::now() + STOP_GRACE;
        while (!members_of(group, true).empty() && std::chrono::steady_clock::now() < deadline) {
            std::this_thread::sleep_for(std::chrono::milliseconds(100));
        }
        if (::kill(-group, SIGKILL) == 0) std::this_thread::sleep_for(std::chrono::milliseconds(200));
    }

    // One process as /proc shows it (node_stats). `ok` is false when the
    // process is gone (or never was).
    struct Usage {
        bool ok = false;
        pid_t pid = 0, ppid = 0;
        std::string name, state;
        double cpu_seconds = 0, age_seconds = 0, memory_mb = 0;
        long threads = 0, fds = 0;
    };

#if defined(__APPLE__)
    // The same record from libproc, in /proc's terms: name is the command
    // name (15 characters, as Linux's comm), state "T" stopped, "R" with a
    // thread running, else "S"; memory_mb is the resident size. libproc does
    // not answer for a process that ended and was not reaped (seen on macOS
    // 26: no BSD info, and the group listing leaves it out), so a zombie is
    // absent here where Linux lists it with state "Z"; the SZOMB tests stand
    // for a system that does report one.
    Usage usage_of(pid_t pid) {
        Usage u;
        u.pid = pid;
        proc_bsdinfo process{};
        if (::proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &process, sizeof(process)) != static_cast<int>(sizeof(process))) return u;
        u.name = std::string(process.pbi_comm, ::strnlen(process.pbi_comm, sizeof(process.pbi_comm)));
        u.ppid = static_cast<pid_t>(process.pbi_ppid);
        timeval now{};
        ::gettimeofday(&now, nullptr);
        u.age_seconds = std::max(0.0, static_cast<double>(now.tv_sec) - static_cast<double>(process.pbi_start_tvsec) +
                                          (static_cast<double>(now.tv_usec) - static_cast<double>(process.pbi_start_tvusec)) / 1e6);
        u.state = process.pbi_status == SZOMB ? "Z" : process.pbi_status == SSTOP ? "T" : "S";
        proc_taskinfo task{};
        if (process.pbi_status != SZOMB &&
            ::proc_pidinfo(pid, PROC_PIDTASKINFO, 0, &task, sizeof(task)) == static_cast<int>(sizeof(task))) {
            // The task's CPU times are in Mach time units (not nanoseconds on Apple silicon).
            mach_timebase_info_data_t base{};
            if (::mach_timebase_info(&base) != KERN_SUCCESS || base.denom == 0) base = {1, 1};
            u.cpu_seconds = static_cast<double>(task.pti_total_user + task.pti_total_system) *
                            static_cast<double>(base.numer) / static_cast<double>(base.denom) / 1e9;
            u.memory_mb = static_cast<double>(task.pti_resident_size) / (1024.0 * 1024.0);
            u.threads = task.pti_threadnum;
            if (u.state == "S" && task.pti_numrunning > 0) u.state = "R";
            const int wanted = ::proc_pidinfo(pid, PROC_PIDLISTFDS, 0, nullptr, 0);
            std::vector<proc_fdinfo> open(static_cast<size_t>(std::max(wanted, 0)) / sizeof(proc_fdinfo) + 64);
            const int bytes = ::proc_pidinfo(pid, PROC_PIDLISTFDS, 0, open.data(), static_cast<int>(open.size() * sizeof(proc_fdinfo)));
            if (bytes > 0) u.fds = static_cast<long>(static_cast<size_t>(bytes) / sizeof(proc_fdinfo));
        }
        u.ok = true;
        return u;
    }
#else
    Usage usage_of(pid_t pid) {
        Usage u;
        u.pid = pid;
        const std::string base = "/proc/" + std::to_string(pid);
        std::ifstream stat(base + "/stat");
        std::string line;
        std::getline(stat, line);
        // pid (comm) state ppid pgrp ...: comm may hold spaces and parentheses.
        const auto open = line.find('('), close = line.rfind(')');
        if (open == std::string::npos || close == std::string::npos || close < open) return u;
        u.name = line.substr(open + 1, close - open - 1);
        std::istringstream rest(line.substr(close + 1));
        std::vector<std::string> fields; // fields[0] is stat's field 3 (state)
        for (std::string f; rest >> f;) fields.push_back(f);
        if (fields.size() < 20) return u;
        const double tick = static_cast<double>(::sysconf(_SC_CLK_TCK));
        if (tick <= 0) return u;
        u.state = fields[0];
        u.ppid = static_cast<pid_t>(std::atol(fields[1].c_str()));
        // utime + stime (fields 14, 15): the process's own CPU, as `module
        // stats` counts it; num_threads (20); starttime (22), in ticks since boot.
        u.cpu_seconds = (std::strtod(fields[11].c_str(), nullptr) + std::strtod(fields[12].c_str(), nullptr)) / tick;
        u.threads = std::atol(fields[17].c_str());
        double boot_uptime = 0;
        std::ifstream("/proc/uptime") >> boot_uptime;
        u.age_seconds = std::max(0.0, boot_uptime - std::strtod(fields[19].c_str(), nullptr) / tick);
        std::ifstream status(base + "/status");
        for (std::string entry; std::getline(status, entry);) {
            if (entry.rfind("VmRSS:", 0) == 0) { u.memory_mb = std::strtod(entry.c_str() + 6, nullptr) / 1024.0; break; }
        }
        if (DIR* fd = ::opendir((base + "/fd").c_str())) {
            while (const dirent* entry = ::readdir(fd)) {
                if (entry->d_name[0] != '.') ++u.fds;
            }
            ::closedir(fd);
        }
        u.ok = true;
        return u;
    }
#endif

    std::string fixed(double value, int decimals) {
        char text[64];
        std::snprintf(text, sizeof(text), "%.*f", decimals, value);
        return text;
    }

    // One supervised role. `pid` is the running Node's process (group leader),
    // 0 while none runs; everything else is guarded by `mutex`.
    struct Role {
        // group_file: where the running Node's process group is recorded (in
        // the module's instance directory; empty outside a Logos host).
        // marker: the environment entry every process of this role's Node
        // carries (LEZ_SWAP_NODE=<instance directory>:<role>).
        std::string name, launcher, log_file, hold_file, group_file, marker;
        int lock_fd = -1;
        std::atomic<pid_t> pid{0};
        std::mutex mutex;
        std::condition_variable wake;
        bool stopping = false;
        std::string state = "restarting";
        uint64_t starts = 0;
        std::string last_exit;
        std::thread thread;
        // When the running Node was started; node_stats' previous CPU reading
        // of each process (CPU seconds, when), for cpu_percent.
        std::chrono::steady_clock::time_point started;
        std::map<pid_t, std::pair<double, std::chrono::steady_clock::time_point>> cpu_seen;

        // fork + exec, with only async-signal-safe calls in the child: every
        // string the child needs is prepared before the fork.
        pid_t spawn() {
            const char* argv[] = {launcher.c_str(), name.c_str(), nullptr};
            std::vector<const char*> envp;
            for (char** entry = environ; *entry != nullptr; ++entry) {
                if (std::strncmp(*entry, "LEZ_SWAP_NODE=", 14) != 0) envp.push_back(*entry);
            }
            envp.push_back(marker.c_str());
            envp.push_back(nullptr);
            const pid_t parent = ::getpid();
#if defined(__APPLE__)
            const int listed = ::proc_pidinfo(parent, PROC_PIDLISTFDS, 0, nullptr, 0);
            std::vector<proc_fdinfo> descriptors(static_cast<size_t>(std::max(listed, 0)) / sizeof(proc_fdinfo) + 64);
#endif
            const pid_t child = ::fork();
            if (child != 0) return child;
            ::setpgid(0, 0);
#if defined(__APPLE__)
            // The Node dies with this module's host: macOS has no parent-death
            // signal, so a watcher process ends the Node's group when the
            // host goes (watch_host). No watcher, no Node.
            close_descriptors(descriptors.data(), static_cast<int>(descriptors.size() * sizeof(proc_fdinfo)), 3);
            const pid_t node = ::getpid();
            const pid_t watcher = ::fork();
            if (watcher < 0) ::_exit(1);
            if (watcher == 0) watch_host(parent, node);
#else
            // The Node dies with this module's host: the kernel sends SIGTERM
            // when the forking thread (this role's, alive exactly as long as
            // the role is supervised) or its process goes. A host that went
            // before this line leaves the child to init: do not start then.
            ::prctl(PR_SET_PDEATHSIG, SIGTERM);
#endif
            if (::getppid() != parent) ::_exit(1);
            const int out = ::open(log_file.c_str(), O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0640);
            const int in = ::open("/dev/null", O_RDONLY | O_CLOEXEC);
            if (out >= 0) { ::dup2(out, STDOUT_FILENO); ::dup2(out, STDERR_FILENO); }
            if (in >= 0) ::dup2(in, STDIN_FILENO);
            ::execve(argv[0], const_cast<char* const*>(argv), const_cast<char* const*>(envp.data()));
            ::_exit(127);
        }

        std::string payload_directory() const {
            const auto slash = launcher.rfind('/');
            return slash == std::string::npos ? std::string() : launcher.substr(0, slash);
        }

        // A Node group a previous host of this module left behind (the host
        // crashed or was killed, and its Node or the Node's sidecars outlived
        // it): ended before a new Node starts. Only a recorded group with a
        // process of this role's Node in it is touched (a group number can be
        // reused once the group is gone).
        void end_recorded_group() {
            if (group_file.empty()) return;
            std::ifstream recorded(group_file);
            long group = 0;
            if (!(recorded >> group) || group <= 0) return;
            const std::string directory = payload_directory();
            bool ours = false;
            for (const pid_t member : members_of(static_cast<pid_t>(group))) {
                if (started_by_us(member, marker, directory)) { ours = true; break; }
            }
            if (ours) {
                warn(name, "ending the Node group " + std::to_string(group) + " a previous host left running");
                end_group(static_cast<pid_t>(group));
            }
            ::unlink(group_file.c_str());
        }

        void record_group(pid_t group) const {
            if (group_file.empty()) return;
            std::ofstream(group_file, std::ios::trunc) << group << "\n";
        }

        // The role's lock (node-<role>.lock beside the group record), held by
        // this module's host for as long as it supervises the role: a host
        // that outlived its Basecamp (and still runs its Node) keeps it, and
        // this one waits instead of starting a second Node. The kernel drops
        // it with the host. Not inherited by the Node (close-on-exec).
        void hold_role_lock(std::unique_lock<std::mutex>& lock) {
            if (group_file.empty()) return;
            const std::string lock_file = group_file.substr(0, group_file.size() - 6) + ".lock";
            if (lock_fd < 0) lock_fd = ::open(lock_file.c_str(), O_RDWR | O_CREAT | O_CLOEXEC, 0600);
            if (lock_fd < 0) return;
            bool told = false;
            while (!stopping && ::flock(lock_fd, LOCK_EX | LOCK_NB) != 0) {
                if (!told) warn(name, "another host of this module supervises the Node; waiting for it to go");
                told = true;
                state = "waiting";
                wake.wait_for(lock, HOLD_POLL);
            }
        }

        void run() {
            std::unique_lock lock(mutex);
            hold_role_lock(lock);
            while (!stopping) {
                if (exists(hold_file)) {
                    state = "held";
                    wake.wait_for(lock, HOLD_POLL);
                    continue;
                }
                lock.unlock();
                end_recorded_group();
                lock.lock();
                if (stopping) break;
                const pid_t child = spawn();
                if (child < 0) {
                    last_exit = std::string("fork failed: ") + std::strerror(errno);
                    warn(name, last_exit);
                } else {
                    // The parent sets the group too, so a kill(-pid) right after
                    // the fork cannot miss a child that has not run setpgid yet.
                    ::setpgid(child, child);
                    record_group(child);
                    pid = child;
                    started = std::chrono::steady_clock::now();
                    ++starts;
                    state = "running";
                    info(name, "Node started (pid " + std::to_string(child) + ", start " + std::to_string(starts) + ")");
                    lock.unlock();
                    int status = 0;
                    while (::waitpid(child, &status, 0) < 0 && errno == EINTR) {}
                    // What the Node leaves in its group (the launcher's
                    // forwards, sidecars it does not wait for) goes with it,
                    // so the next start finds its ports and sockets free.
                    end_group(child);
                    if (!group_file.empty()) ::unlink(group_file.c_str());
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
            if (lock_fd >= 0) ::close(lock_fd);
            lock_fd = -1;
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
#if defined(__APPLE__)
        // shutdown() does not wake an accept() on macOS: stop() writes to
        // this pipe and the thread waits on it together with the listener.
        int wake[2] = {-1, -1};
#endif
        std::atomic<bool> stopping{false};
        std::thread thread;
        std::mutex mutex;
        std::condition_variable ended;
        bool running = false;

        ~Signer() {
            if (listener >= 0) ::close(listener);
#if defined(__APPLE__)
            for (const int end : wake) {
                if (end >= 0) ::close(end);
            }
#endif
        }
#if defined(__APPLE__)

        // accept4(listener, SOCK_CLOEXEC) for macOS, ended by stop(): the next
        // connection, or -1 with errno EINTR (look again) or ESHUTDOWN
        // (stopped). The listener does not block, so a connection that went
        // between poll and accept cannot hold the thread; the connection
        // itself blocks as on Linux.
        int accept_connection() const {
            pollfd waiting[2] = {{listener, POLLIN, 0}, {wake[0], POLLIN, 0}};
            if (::poll(waiting, 2, -1) < 0) return -1;
            if (stopping || waiting[1].revents != 0 || (waiting[0].revents & (POLLERR | POLLNVAL)) != 0) {
                errno = ESHUTDOWN;
                return -1;
            }
            const int connection = close_on_exec(::accept(listener, nullptr, nullptr));
            if (connection < 0) {
                if (errno == EAGAIN || errno == EWOULDBLOCK) errno = EINTR;
                return -1;
            }
            ::fcntl(connection, F_SETFL, ::fcntl(connection, F_GETFL) & ~O_NONBLOCK);
            return connection;
        }
#endif

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
#if defined(__APPLE__)
                    const int connection = self->accept_connection();
#else
                    const int connection = ::accept4(self->listener, nullptr, nullptr, SOCK_CLOEXEC);
#endif
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
#if defined(__APPLE__)
            if (wake[1] >= 0) (void)!::write(wake[1], "", 1);
#endif
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
        // The module's instance directory (empty outside a Logos host): what
        // the desks asked for is recorded here and resumed on the next load.
        std::string records;
    };

    // Recorded requests: node-<role>.start holds the launcher, log and hold
    // file of a supervised role (one per line), node-<role>.group its running
    // Node's process group, signers one "<socket> <account>" line per signer.
    std::string start_record(const Supervisor& s, const std::string& role) {
        return s.records.empty() ? std::string() : s.records + "/node-" + role + ".start";
    }
    std::string group_record(const Supervisor& s, const std::string& role) {
        return s.records.empty() ? std::string() : s.records + "/node-" + role + ".group";
    }
    std::string signer_record(const Supervisor& s) {
        return s.records.empty() ? std::string() : s.records + "/signers";
    }

    // Written whole to a temporary file and renamed, so a host that dies
    // mid-write leaves the previous record.
    void write_record(const std::string& path, const std::string& content) {
        if (path.empty()) return;
        const std::string temporary = path + ".tmp";
        {
            std::ofstream out(temporary, std::ios::trunc);
            out << content;
            if (!out) return;
        }
        ::rename(temporary.c_str(), path.c_str());
    }

    std::vector<std::string> read_lines(const std::string& path) {
        std::vector<std::string> lines;
        std::ifstream in(path);
        for (std::string line; std::getline(in, line);) lines.push_back(line);
        return lines;
    }

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

void LezSwapNodeImpl::onContextReady() {
    auto& s = supervisor_of(supervisor);
    {
        std::lock_guard lock(s.mutex);
        s.records = instancePersistencePath();
    }
    if (s.records.empty()) return;
    // The signers first: a resumed Node waits for its signer's socket.
    for (const auto& line : read_lines(signer_record(s))) {
        const auto space = line.find(' ');
        if (space == std::string::npos) continue;
        info("signer", "resuming the signer on " + line.substr(0, space));
        serve_signer(line.substr(0, space), line.substr(space + 1));
    }
    DIR* directory = ::opendir(s.records.c_str());
    if (directory == nullptr) return;
    std::vector<std::string> roles;
    while (const dirent* entry = ::readdir(directory)) {
        const std::string file = entry->d_name;
        const std::string prefix = "node-", suffix = ".start";
        if (file.size() > prefix.size() + suffix.size() && file.rfind(prefix, 0) == 0 &&
            file.compare(file.size() - suffix.size(), suffix.size(), suffix) == 0) {
            roles.push_back(file.substr(prefix.size(), file.size() - prefix.size() - suffix.size()));
        }
    }
    ::closedir(directory);
    for (const auto& role : roles) {
        const auto request = read_lines(start_record(s, role));
        if (request.size() != 3) continue;
        info(role, "resuming the Node a previous host of this module supervised");
        start_node(role, request[0], request[1], request[2]);
    }
}

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
#if defined(__APPLE__)
    signer->listener = close_on_exec(::socket(AF_UNIX, SOCK_STREAM, 0));
    if (signer->listener < 0 || ::pipe(signer->wake) != 0) return 1;
    close_on_exec(signer->wake[0]);
    close_on_exec(signer->wake[1]);
    ::fcntl(signer->listener, F_SETFL, ::fcntl(signer->listener, F_GETFL) | O_NONBLOCK);
    // Answering a client that already hung up must not raise SIGPIPE in the
    // host; set on the listener, every connection inherits it (it cannot be
    // set on a connection whose peer is gone).
    const int no_sigpipe = 1;
    ::setsockopt(signer->listener, SOL_SOCKET, SO_NOSIGPIPE, &no_sigpipe, sizeof(no_sigpipe));
#else
    signer->listener = ::socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (signer->listener < 0) return 1;
#endif
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
    std::string served;
    for (const auto& [path, each] : s.signers) served += path + " " + each->account + "\n";
    write_record(signer_record(s), served);
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
    supervised->group_file = group_record(s, role);
    supervised->marker = "LEZ_SWAP_NODE=" + s.records + ":" + role;
    write_record(start_record(s, role), launcher + "\n" + log_file + "\n" + hold_file + "\n");
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
        if (const auto record = start_record(s, role); valid_role(role) && !record.empty()) ::unlink(record.c_str());
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

std::string LezSwapNodeImpl::node_stats(const std::string& role) {
    std::shared_ptr<Role> supervised;
    {
        auto& s = supervisor_of(supervisor);
        std::lock_guard lock(s.mutex);
        if (auto it = s.roles.find(role); it != s.roles.end()) supervised = it->second;
    }
    if (!supervised) return "";
    std::lock_guard lock(supervised->mutex);
    const auto now = std::chrono::steady_clock::now();
    const pid_t leader = supervised->pid.load();
    // The Node's group, the Node first; nothing while no Node runs.
    std::vector<pid_t> members;
    if (leader > 0) {
        members = members_of(leader);
        std::sort(members.begin(), members.end());
        members.erase(std::remove(members.begin(), members.end(), leader), members.end());
        members.insert(members.begin(), leader);
    }
    std::map<pid_t, std::pair<double, std::chrono::steady_clock::time_point>> seen;
    auto percent = [&](const Usage& u) {
        double p = u.age_seconds > 0 ? u.cpu_seconds / u.age_seconds * 100.0 : 0.0;
        if (auto it = supervised->cpu_seen.find(u.pid); it != supervised->cpu_seen.end()) {
            const double elapsed = std::chrono::duration<double>(now - it->second.second).count();
            if (elapsed > 0) p = std::max(0.0, (u.cpu_seconds - it->second.first) / elapsed * 100.0);
        }
        seen[u.pid] = {u.cpu_seconds, now};
        return p;
    };
    auto object = [](const Usage& u, double cpu_percent) {
        return "{\"pid\":" + std::to_string(u.pid) + ",\"ppid\":" + std::to_string(u.ppid) + ",\"name\":" + json_string(u.name) +
               ",\"state\":" + json_string(u.state) + ",\"cpu_percent\":" + fixed(cpu_percent, 1) +
               ",\"cpu_time_seconds\":" + fixed(u.cpu_seconds, 2) + ",\"memory_mb\":" + fixed(u.memory_mb, 3) +
               ",\"threads\":" + std::to_string(u.threads) + ",\"fds\":" + std::to_string(u.fds) + "}";
    };
    std::string processes;
    Usage total;
    double total_percent = 0;
    long count = 0;
    for (const pid_t member : members) {
        const Usage u = usage_of(member);
        if (!u.ok) continue;
        const double p = percent(u);
        processes += (processes.empty() ? "" : ",") + object(u, p);
        ++count;
        total_percent += p;
        total.cpu_seconds += u.cpu_seconds;
        total.memory_mb += u.memory_mb;
        total.threads += u.threads;
        total.fds += u.fds;
    }
    const Usage host = usage_of(::getpid());
    const double host_percent = host.ok ? percent(host) : 0.0;
    supervised->cpu_seen.swap(seen);
    const double uptime = leader > 0 ? std::chrono::duration<double>(now - supervised->started).count() : 0.0;
    const uint64_t starts = supervised->starts;
    return "{\"role\":" + json_string(supervised->name) + ",\"state\":" + json_string(supervised->state) +
           ",\"pid\":" + std::to_string(leader) + ",\"uptime_seconds\":" + fixed(uptime, 1) +
           ",\"starts\":" + std::to_string(starts) + ",\"restarts\":" + std::to_string(starts > 0 ? starts - 1 : 0) +
           ",\"last_exit\":" + json_string(supervised->last_exit) + ",\"processes\":[" + processes + "]" +
           ",\"group\":{\"processes\":" + std::to_string(count) + ",\"cpu_percent\":" + fixed(total_percent, 1) +
           ",\"cpu_time_seconds\":" + fixed(total.cpu_seconds, 2) + ",\"memory_mb\":" + fixed(total.memory_mb, 3) +
           ",\"threads\":" + std::to_string(total.threads) + ",\"fds\":" + std::to_string(total.fds) + "}" +
           ",\"host\":" + (host.ok ? object(host, host_percent) : std::string("null")) + "}";
}
