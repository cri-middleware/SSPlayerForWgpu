# DX12 verification viewer for the ssplayer-wgpu C API (SPEC §12 P5).
#
#   cmake -B build && cmake --build build --config Release
#   .\build\Release\ssviewer-dx12.exe
#
# The player is built by cargo (feature `ffi`), which also regenerates
# libs/ssplayer-wgpu/include/ssplayer_wgpu.h through cbindgen. This project is
# outside the Cargo workspace on purpose: it consumes the built library and the
# generated header, nothing else.

cmake_minimum_required(VERSION 3.20)
project(ssplayer-wgpu-viewer-dx12 LANGUAGES CXX)

if(NOT WIN32)
    message(FATAL_ERROR "This viewer is Windows/DX12 only.")
endif()

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# Single-config generators (Ninja, NMake) need a default; Visual Studio ignores
# this and picks its config at build time, which the generator expressions below
# follow either way.
get_property(SS_MULTI_CONFIG GLOBAL PROPERTY GENERATOR_IS_MULTI_CONFIG)
if(NOT SS_MULTI_CONFIG AND NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Release CACHE STRING "Build type" FORCE)
endif()

get_filename_component(REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../.." ABSOLUTE)

# Match cargo's profile to CMake's, since that decides the target/ subdirectory.
# As generator expressions, so a Visual Studio build of Debug still gets the
# debug library it links against.
set(CARGO_ARGS "$<$<NOT:$<CONFIG:Debug>>:--release>")
set(SS_LIB_DIR "${REPO_ROOT}/target/$<IF:$<CONFIG:Debug>,debug,release>")
# The cdylib's MSVC import library. `ssplayer_wgpu.lib` next to it is the
# staticlib — linking that one instead works too, but then every system library
# wgpu pulls in has to be named here by hand.
set(SS_IMPORT_LIB "${SS_LIB_DIR}/ssplayer_wgpu.dll.lib")
set(SS_DLL "${SS_LIB_DIR}/ssplayer_wgpu.dll")
set(SS_INCLUDE_DIR "${REPO_ROOT}/libs/ssplayer-wgpu/include")

# Always run: cargo is the authority on whether anything needs rebuilding, and
# it regenerates the header in the same pass.
add_custom_target(ssplayer_wgpu_ffi ALL
    COMMAND cargo build ${CARGO_ARGS} -p ssplayer-wgpu --features ffi
    WORKING_DIRECTORY "${REPO_ROOT}"
    BYPRODUCTS "${SS_IMPORT_LIB}" "${SS_DLL}" "${SS_INCLUDE_DIR}/ssplayer_wgpu.h"
    COMMENT "cargo build -p ssplayer-wgpu --features ffi"
    VERBATIM)

add_executable(ssviewer-dx12 src/main.cpp)
add_dependencies(ssviewer-dx12 ssplayer_wgpu_ffi)

target_compile_definitions(ssviewer-dx12 PRIVATE UNICODE _UNICODE)
if(MSVC)
    target_compile_options(ssviewer-dx12 PRIVATE /W4 /utf-8)
else()
    target_compile_options(ssviewer-dx12 PRIVATE -Wall -Wextra)
endif()
target_include_directories(ssviewer-dx12 PRIVATE "${SS_INCLUDE_DIR}")
# Lets the viewer run from any working directory; argv[1] still wins.
target_compile_definitions(ssviewer-dx12 PRIVATE SS_DEFAULT_ASSET_DIR="${REPO_ROOT}/assets/overall")
target_link_libraries(ssviewer-dx12 PRIVATE "${SS_IMPORT_LIB}" shell32)

# Windows has no rpath: the DLL has to sit beside the .exe.
add_custom_command(TARGET ssviewer-dx12 POST_BUILD
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${SS_DLL}" "$<TARGET_FILE_DIR:ssviewer-dx12>"
    COMMENT "copying ssplayer_wgpu.dll next to the viewer"
    VERBATIM)
