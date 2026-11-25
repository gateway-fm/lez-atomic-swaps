cmake_minimum_required(VERSION 3.14)
project(LezZcashNodePlugin LANGUAGES CXX)

if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LogosModule.cmake not found. Set LOGOS_MODULE_BUILDER_ROOT.")
endif()

file(READ "${CMAKE_CURRENT_SOURCE_DIR}/metadata.json" METADATA_JSON)
string(JSON MODULE_NAME GET ${METADATA_JSON} name)

logos_module(
    NAME ${MODULE_NAME}
    REP_FILE src/lez_zcash_node.rep
    SOURCES
        src/lez_zcash_node_backend.h
        src/lez_zcash_node_backend.cpp
        src/lez_zcash_host.h
    INCLUDE_DIRS src
)
