#include "lez_taker_node_impl.h"

#include <fstream>
#include <sstream>

#include <unistd.h>

std::string LezTakerNodeImpl::launcher() {
    const std::string path = modulePath() + "/bin/node-entrypoint.sh";
    return !modulePath().empty() && ::access(path.c_str(), X_OK) == 0 ? path : std::string();
}

std::string LezTakerNodeImpl::payload() {
    std::ifstream manifest(modulePath() + "/bin/payload.json");
    std::ostringstream content;
    content << manifest.rdbuf();
    return manifest ? content.str() : std::string();
}
