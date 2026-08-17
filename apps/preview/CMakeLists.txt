cmake_minimum_required(VERSION 3.19)

project(lez-atomic-swap-preview
    VERSION 0.1.0
    DESCRIPTION "Standalone preview host for the real Maker/Taker Basecamp QML views"
    LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

find_package(Qt6 REQUIRED COMPONENTS Quick QuickControls2 Qml)

qt_standard_project_setup()

qt_add_executable(lez-swap-preview
    src/main.cpp
    src/logos_host.h
    src/logos_host.cpp
    src/stub_backends.h
    src/stub_backends.cpp)

qt_add_qml_module(lez-swap-preview
    URI LezSwapPreview
    VERSION 1.0
    QML_FILES qml/Preview.qml)

target_link_libraries(lez-swap-preview PRIVATE
    Qt6::Quick
    Qt6::QuickControls2
    Qt6::Qml)

target_compile_definitions(lez-swap-preview PRIVATE
    LEZ_PREVIEW_SOURCE_ROOT="${CMAKE_CURRENT_SOURCE_DIR}/..")
