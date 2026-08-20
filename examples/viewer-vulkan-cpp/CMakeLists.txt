# Vulkan verification viewer for the ssplayer-wgpu C API (Linux).
#
#   cmake -B build && cmake --build build
#   ./build/ssviewer-vulkan
#
# The player is built by cargo (feature `ffi`), which also regenerates
# libs/ssplayer-wgpu/include/ssplayer_wgpu.h through cbindgen. This project is
# outside the Cargo workspace on purpose: it consumes the built library and the
# generated header, nothing else.
#
# C as well as C++: wayland-scanner emits the xdg-shell marshalling code as C.

cmake_minimum_required(VERSION 3.20)
project(ssplayer-wgpu-viewer-vulkan LANGUAGES C CXX)

if(NOT UNIX OR APPLE)
    message(FATAL_ERROR "This viewer is Linux only.")
endif()

set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Release CACHE STRING "Build type" FORCE)
endif()

get_filename_component(REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../.." ABSOLUTE)

# Match cargo's profile to CMake's, since that decides the target/ subdirectory.
set(CARGO_ARGS "$<$<NOT:$<CONFIG:Debug>>:--release>")
set(SS_LIB_DIR "${REPO_ROOT}/target/$<IF:$<CONFIG:Debug>,debug,release>")
set(SS_LIB "${SS_LIB_DIR}/libssplayer_wgpu.so")
set(SS_INCLUDE_DIR "${REPO_ROOT}/libs/ssplayer-wgpu/include")

# Always run: cargo is the authority on whether anything needs rebuilding, and
# it regenerates the header in the same pass.
add_custom_target(ssplayer_wgpu_ffi ALL
    COMMAND cargo build ${CARGO_ARGS} -p ssplayer-wgpu --features ffi
    WORKING_DIRECTORY "${REPO_ROOT}"
    BYPRODUCTS "${SS_LIB}" "${SS_INCLUDE_DIR}/ssplayer_wgpu.h"
    COMMENT "cargo build -p ssplayer-wgpu --features ffi"
    VERBATIM)

# --- window systems ---------------------------------------------------------
#
# Both are optional and detected, because they are what the machine happens to
# have installed rather than a choice the sample makes: a headless build box may
# carry neither, and either one alone still builds a viewer that runs. What is
# not optional is having one of them.

find_package(PkgConfig)
find_package(X11)

set(SS_WINDOW_SOURCES "")
set(SS_WINDOW_LIBS "")
set(SS_WINDOW_DEFINES "")

if(X11_FOUND)
    list(APPEND SS_WINDOW_LIBS X11::X11)
    list(APPEND SS_WINDOW_DEFINES SS_HAVE_X11=1)
else()
    message(WARNING "libX11 not found — building without the SS_WINDOW_XLIB path (apt install libx11-dev)")
endif()

if(PKG_CONFIG_FOUND)
    pkg_check_modules(WAYLAND_CLIENT IMPORTED_TARGET wayland-client)
    pkg_get_variable(WAYLAND_SCANNER wayland-scanner wayland_scanner)
    pkg_get_variable(WAYLAND_PROTOCOLS_DIR wayland-protocols pkgdatadir)
endif()

if(WAYLAND_CLIENT_FOUND AND WAYLAND_SCANNER AND WAYLAND_PROTOCOLS_DIR)
    # xdg-shell is how a wl_surface becomes a window the compositor will show,
    # and it is not part of libwayland: the protocol ships as XML and each client
    # generates its own marshalling code from it.
    set(XDG_SHELL_XML "${WAYLAND_PROTOCOLS_DIR}/stable/xdg-shell/xdg-shell.xml")
    set(XDG_SHELL_HEADER "${CMAKE_CURRENT_BINARY_DIR}/xdg-shell-client-protocol.h")
    set(XDG_SHELL_CODE "${CMAKE_CURRENT_BINARY_DIR}/xdg-shell-protocol.c")

    add_custom_command(OUTPUT "${XDG_SHELL_HEADER}"
        COMMAND "${WAYLAND_SCANNER}" client-header "${XDG_SHELL_XML}" "${XDG_SHELL_HEADER}"
        DEPENDS "${XDG_SHELL_XML}"
        COMMENT "wayland-scanner client-header xdg-shell"
        VERBATIM)
    add_custom_command(OUTPUT "${XDG_SHELL_CODE}"
        COMMAND "${WAYLAND_SCANNER}" private-code "${XDG_SHELL_XML}" "${XDG_SHELL_CODE}"
        DEPENDS "${XDG_SHELL_XML}"
        COMMENT "wayland-scanner private-code xdg-shell"
        VERBATIM)

    list(APPEND SS_WINDOW_SOURCES "${XDG_SHELL_HEADER}" "${XDG_SHELL_CODE}")
    list(APPEND SS_WINDOW_LIBS PkgConfig::WAYLAND_CLIENT)
    list(APPEND SS_WINDOW_DEFINES SS_HAVE_WAYLAND=1)
else()
    message(WARNING "libwayland-client / wayland-protocols not found — building without the SS_WINDOW_WAYLAND path "
                    "(apt install libwayland-dev wayland-protocols)")
endif()

if(NOT SS_WINDOW_DEFINES)
    message(FATAL_ERROR "neither X11 nor Wayland development files were found; there is no window to draw into")
endif()

# --- viewer -----------------------------------------------------------------

add_executable(ssviewer-vulkan src/main.cpp ${SS_WINDOW_SOURCES})
add_dependencies(ssviewer-vulkan ssplayer_wgpu_ffi)

target_compile_options(ssviewer-vulkan PRIVATE -Wall -Wextra)
target_compile_definitions(ssviewer-vulkan PRIVATE ${SS_WINDOW_DEFINES})
# Lets the viewer run from any working directory; the first argument still wins.
target_compile_definitions(ssviewer-vulkan PRIVATE SS_DEFAULT_ASSET_DIR="${REPO_ROOT}/assets/overall")
target_include_directories(ssviewer-vulkan PRIVATE "${SS_INCLUDE_DIR}" "${CMAKE_CURRENT_BINARY_DIR}")
target_link_libraries(ssviewer-vulkan PRIVATE "${SS_LIB}" ${SS_WINDOW_LIBS})

# The cdylib stays in target/; unlike Windows, an rpath means it does not have to
# be copied next to the executable to be found at run time.
set_target_properties(ssviewer-vulkan PROPERTIES BUILD_RPATH "${SS_LIB_DIR}")
