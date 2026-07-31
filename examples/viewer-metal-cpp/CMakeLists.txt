# Metal verification viewer for the ssplayer-wgpu C API (SPEC §12 P5).
#
#   cmake -B build && cmake --build build && ./build/ssviewer-metal
#
# The player is built by cargo (feature `ffi`), which also regenerates
# libs/ssplayer-wgpu/include/ssplayer_wgpu.h through cbindgen. This project is
# outside the Cargo workspace on purpose: it consumes the built library and the
# generated header, nothing else.

cmake_minimum_required(VERSION 3.20)
project(ssplayer-wgpu-viewer-metal LANGUAGES OBJCXX)

if(NOT APPLE)
    message(FATAL_ERROR "This viewer is macOS/Metal only.")
endif()

set(CMAKE_OBJCXX_STANDARD 17)
set(CMAKE_OBJCXX_STANDARD_REQUIRED ON)

if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Release CACHE STRING "Build type" FORCE)
endif()

get_filename_component(REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../.." ABSOLUTE)

# Match cargo's profile to CMake's, since that decides the target/ subdirectory.
if(CMAKE_BUILD_TYPE STREQUAL "Debug")
    set(CARGO_ARGS "")
    set(CARGO_PROFILE_DIR debug)
else()
    set(CARGO_ARGS --release)
    set(CARGO_PROFILE_DIR release)
endif()

set(SS_LIB_DIR "${REPO_ROOT}/target/${CARGO_PROFILE_DIR}")
set(SS_LIB "${SS_LIB_DIR}/libssplayer_wgpu.dylib")
set(SS_INCLUDE_DIR "${REPO_ROOT}/libs/ssplayer-wgpu/include")

# Always run: cargo is the authority on whether anything needs rebuilding, and
# it regenerates the header in the same pass.
add_custom_target(ssplayer_wgpu_ffi ALL
    COMMAND cargo build ${CARGO_ARGS} -p ssplayer-wgpu --features ffi
    WORKING_DIRECTORY "${REPO_ROOT}"
    BYPRODUCTS "${SS_LIB}" "${SS_INCLUDE_DIR}/ssplayer_wgpu.h"
    COMMENT "cargo build -p ssplayer-wgpu --features ffi (${CARGO_PROFILE_DIR})"
    VERBATIM)

add_executable(ssviewer-metal src/main.mm)
add_dependencies(ssviewer-metal ssplayer_wgpu_ffi)

target_compile_options(ssviewer-metal PRIVATE -fobjc-arc -Wall -Wextra)
target_include_directories(ssviewer-metal PRIVATE "${SS_INCLUDE_DIR}")
# Lets the viewer run from any working directory; argv[1] still wins.
target_compile_definitions(ssviewer-metal PRIVATE SS_DEFAULT_ASSET_DIR="${REPO_ROOT}/assets/overall")
target_link_libraries(ssviewer-metal PRIVATE "${SS_LIB}" "-framework Cocoa" "-framework QuartzCore")
# The cdylib's install name is @rpath-relative, so point at where cargo put it.
set_target_properties(ssviewer-metal PROPERTIES BUILD_RPATH "${SS_LIB_DIR}")
