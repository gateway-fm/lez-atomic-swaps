cmake_minimum_required(VERSION 3.14)
project(LezSwapNodePlugin LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

if(DEFINED ENV{LOGOS_MODULE_BUILDER_ROOT})
    include($ENV{LOGOS_MODULE_BUILDER_ROOT}/cmake/LogosModule.cmake)
elseif(EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/cmake/LogosModule.cmake")
    include(cmake/LogosModule.cmake)
else()
    message(FATAL_ERROR "LogosModule.cmake not found. Set LOGOS_MODULE_BUILDER_ROOT.")
endif()

logos_module(
    NAME lez_swap_node
    SOURCES
        src/lez_swap_node_impl.h
        src/lez_swap_node_impl.cpp
    INCLUDE_DIRS
        src
)
