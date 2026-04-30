# Build:
#   mkdir build && cd build
#   cmake .. -DCMAKE_BUILD_TYPE=Release
#   cmake --build . -j$(nproc)
#   # Add the build dir to PATH, or copy rpresent-qml-host to /usr/local/bin

cmake_minimum_required(VERSION 3.16)
project(rpresent-qml-host LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_AUTOMOC ON)
set(CMAKE_AUTORCC ON)

# Prefer Qt6; fall back to Qt5.
find_package(Qt6 QUIET COMPONENTS Core Gui Quick Qml)
if(Qt6_FOUND)
    set(QT_LIBS Qt6::Core Qt6::Gui Qt6::Quick Qt6::Qml)
    message(STATUS "rpresent-qml-host: using Qt6")
else()
    find_package(Qt5 REQUIRED COMPONENTS Core Gui Quick Qml)
    set(QT_LIBS Qt5::Core Qt5::Gui Qt5::Quick Qt5::Qml)
    message(STATUS "rpresent-qml-host: using Qt5")
endif()

add_executable(rpresent-qml-host main.cpp qml.qrc)
target_link_libraries(rpresent-qml-host PRIVATE ${QT_LIBS})
