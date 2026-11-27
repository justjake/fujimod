cmake_minimum_required(VERSION 3.22)
project(fujimod_dynarmic LANGUAGES C CXX)

set(BUILD_SHARED_LIBS OFF)
set(CMAKE_ARCHIVE_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/lib")
set(DYNARMIC_TESTS OFF CACHE BOOL "")
set(DYNARMIC_USE_BUNDLED_EXTERNALS ON CACHE BOOL "")
set(DYNARMIC_WARNINGS_AS_ERRORS OFF CACHE BOOL "")
set(DYNARMIC_FRONTENDS A32 CACHE STRING "")
add_subdirectory(vendor/dynarmic)

add_library(dynarmic_ffi STATIC csrc/dynarmic_ffi.cpp)
target_compile_features(dynarmic_ffi PRIVATE cxx_std_20)
target_link_libraries(dynarmic_ffi PRIVATE dynarmic)
