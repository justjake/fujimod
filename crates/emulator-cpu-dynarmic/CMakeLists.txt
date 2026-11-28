cmake_minimum_required(VERSION 3.22)
project(fujimod_dynarmic LANGUAGES C CXX)

set(BUILD_SHARED_LIBS OFF)
set(CMAKE_ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/lib")
set(DYNARMIC_TESTS OFF CACHE BOOL "")
set(DYNARMIC_USE_BUNDLED_EXTERNALS ON CACHE BOOL "")
set(DYNARMIC_WARNINGS_AS_ERRORS OFF CACHE BOOL "")
set(DYNARMIC_FRONTENDS A32 CACHE STRING "")
add_subdirectory(vendor/dynarmic "${CMAKE_CURRENT_BINARY_DIR}/dynarmic")

add_library(dynarmic_ffi STATIC csrc/dynarmic_ffi.cpp)
target_compile_features(dynarmic_ffi PRIVATE cxx_std_20)
# The adapter includes Dynarmic IR headers, which include Boost. Dynarmic
# keeps its Boost dependency private, so the adapter names it itself; on
# Linux the headers sit on the default include path, on macOS under Homebrew.
find_package(Boost 1.57 REQUIRED CONFIG)
target_link_libraries(dynarmic_ffi PRIVATE dynarmic merry::mcl Boost::headers)
