cmake_minimum_required(VERSION 3.22)
project(fujimod_dynarmic LANGUAGES C CXX)

set(BUILD_SHARED_LIBS OFF)
set(CMAKE_ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/lib")
set(DYNARMIC_TESTS OFF CACHE BOOL "")
set(DYNARMIC_USE_BUNDLED_EXTERNALS ON CACHE BOOL "")
set(DYNARMIC_WARNINGS_AS_ERRORS OFF CACHE BOOL "")
set(DYNARMIC_FRONTENDS A32 CACHE STRING "")
# Apply the embedder changes to an out-of-tree copy of the pinned sources.
# This keeps recursive submodule checkouts unchanged on developer and CI hosts.
find_package(Git REQUIRED)
set(DYNARMIC_SOURCE "${CMAKE_CURRENT_BINARY_DIR}/dynarmic-source")
set(DYNARMIC_PATCHES
    "${CMAKE_CURRENT_SOURCE_DIR}/patches/a32-decoder-fixed-bits.patch"
    "${CMAKE_CURRENT_SOURCE_DIR}/patches/arm64-embedder.patch"
    "${CMAKE_CURRENT_SOURCE_DIR}/patches/x64-exact-budget.patch")
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS ${DYNARMIC_PATCHES})
file(REMOVE_RECURSE "${DYNARMIC_SOURCE}")
file(COPY "${CMAKE_CURRENT_SOURCE_DIR}/vendor/dynarmic/"
    DESTINATION "${DYNARMIC_SOURCE}"
    PATTERN ".git" EXCLUDE)
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env --unset=GIT_DIR --unset=GIT_WORK_TREE
        "GIT_CEILING_DIRECTORIES=${CMAKE_CURRENT_BINARY_DIR}"
        "${GIT_EXECUTABLE}" apply --no-index ${DYNARMIC_PATCHES}
    WORKING_DIRECTORY "${DYNARMIC_SOURCE}"
    COMMAND_ERROR_IS_FATAL ANY)
add_subdirectory("${DYNARMIC_SOURCE}" "${CMAKE_CURRENT_BINARY_DIR}/dynarmic")

find_package(Boost 1.57 REQUIRED)
add_library(dynarmic_ffi STATIC csrc/dynarmic_ffi.cpp)
target_compile_features(dynarmic_ffi PRIVATE cxx_std_20)
target_link_libraries(dynarmic_ffi PRIVATE dynarmic merry::mcl Boost::boost)
