cmake_minimum_required(VERSION 3.14)
project(LezAtomicSwapMakerPlugin LANGUAGES CXX)

if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LogosModule.cmake not found. Set LOGOS_MODULE_BUILDER_ROOT.")
endif()

file(READ "${CMAKE_CURRENT_SOURCE_DIR}/metadata.json" METADATA_JSON)
string(JSON MODULE_NAME GET ${METADATA_JSON} name)

logos_module(
    NAME ${MODULE_NAME}
    REP_FILE src/lez_atomic_swap_maker.rep
    SOURCES
        src/lez_atomic_swap_maker_backend.h
        src/lez_atomic_swap_maker_backend.cpp
        src/local_json_rpc_client.h
        src/local_json_rpc_client.cpp
    INCLUDE_DIRS src
)

find_package(Qt6 REQUIRED COMPONENTS Network)
target_link_libraries(${MODULE_NAME}_module_plugin PRIVATE Qt6::Network)
