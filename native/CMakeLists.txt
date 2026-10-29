cmake_minimum_required(VERSION 3.24)
project(RhdBootstrapProbe LANGUAGES CXX)

if(NOT WIN32 OR NOT MSVC OR NOT CMAKE_SIZEOF_VOID_P EQUAL 8)
    message(FATAL_ERROR "The bootstrap probe requires the x64 MSVC toolchain on Windows.")
endif()

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")

set(detours_archive "${CMAKE_CURRENT_SOURCE_DIR}/Detours-1dbd4ea.zip")
file(SHA256 "${detours_archive}" detours_sha256)
if(NOT detours_sha256 STREQUAL "0f08f92a21a7d1d070a8960822adc4b608d5abeed115e55d22a5bf0a3a3f9955")
    message(FATAL_ERROR "Detours source archive SHA-256 mismatch.")
endif()
set(detours_root "${CMAKE_CURRENT_BINARY_DIR}/third_party/Detours-1dbd4ea281d73d478aa8d09a8b83fbd265f715f7")
if(NOT EXISTS "${detours_root}/src/detours.h")
    file(ARCHIVE_EXTRACT INPUT "${detours_archive}" DESTINATION "${CMAKE_CURRENT_BINARY_DIR}/third_party")
endif()
set(detours_src "${detours_root}/src")
add_library(detours STATIC
    "${detours_src}/detours.cpp" "${detours_src}/modules.cpp"
    "${detours_src}/disasm.cpp" "${detours_src}/image.cpp"
    "${detours_src}/creatwth.cpp" "${detours_src}/disolx86.cpp"
    "${detours_src}/disolx64.cpp" "${detours_src}/disolia64.cpp"
    "${detours_src}/disolarm.cpp" "${detours_src}/disolarm64.cpp")
target_include_directories(detours PUBLIC "${detours_src}")
target_compile_definitions(detours PRIVATE WIN32_LEAN_AND_MEAN _WIN32_WINNT=0x0601)

add_library(rhd_bootstrap SHARED bootstrap.cpp bootstrap.def)
target_link_libraries(rhd_bootstrap PRIVATE detours)
target_compile_definitions(rhd_bootstrap PRIVATE UNICODE _UNICODE WIN32_LEAN_AND_MEAN)

add_executable(rhd_fixture fixture.cpp)
target_compile_definitions(rhd_fixture PRIVATE UNICODE _UNICODE WIN32_LEAN_AND_MEAN)

set(fixture_identity "${CMAKE_CURRENT_BINARY_DIR}/$<CONFIG>/fixture_identity.h")
add_custom_command(OUTPUT "${fixture_identity}"
    COMMAND "${CMAKE_COMMAND}" "-DFIXTURE=$<TARGET_FILE:rhd_fixture>"
            "-DOUTPUT=${fixture_identity}" -P "${CMAKE_CURRENT_SOURCE_DIR}/fixture_hash.cmake"
    DEPENDS rhd_fixture "${CMAKE_CURRENT_SOURCE_DIR}/fixture_hash.cmake"
    VERBATIM)
add_executable(rhd_probe probe.cpp "${fixture_identity}")
target_include_directories(rhd_probe PRIVATE "${CMAKE_CURRENT_BINARY_DIR}/$<CONFIG>")
target_link_libraries(rhd_probe PRIVATE detours Advapi32)
target_compile_definitions(rhd_probe PRIVATE UNICODE _UNICODE WIN32_LEAN_AND_MEAN)
add_dependencies(rhd_probe rhd_bootstrap)

enable_testing()
foreach(case IN ITEMS success missing timeout wrong-version)
    add_test(NAME "bootstrap-${case}" COMMAND rhd_probe "${case}")
    set_tests_properties("bootstrap-${case}" PROPERTIES TIMEOUT 12)
endforeach()

add_executable(rhd_transaction_fixture transaction_fixture.cpp)
target_compile_definitions(rhd_transaction_fixture PRIVATE UNICODE _UNICODE WIN32_LEAN_AND_MEAN)
foreach(case IN ITEMS success no-sync version-mismatch preimage-mismatch range-invalid
        overlap protect-failure protect-race partial-write flush-failure restore-failure rollback-failure)
    add_test(NAME "transaction-${case}" COMMAND rhd_transaction_fixture "${case}")
    set_tests_properties("transaction-${case}" PROPERTIES TIMEOUT 12)
endforeach()

# ── 進程內八刀補丁（TR-BUG-458 路線 B2）──
# 本組原始碼含 UTF-8 中文註解：必須 /utf-8，否則 MSVC 以系統字碼頁解讀會吞掉括號。
add_library(rhd_inproc SHARED inproc.cpp inproc.def)
target_link_libraries(rhd_inproc PRIVATE detours)
target_compile_definitions(rhd_inproc PRIVATE UNICODE _UNICODE WIN32_LEAN_AND_MEAN _WIN32_WINNT=0x0601)
target_compile_options(rhd_inproc PRIVATE /utf-8)

add_executable(rhd_inproc_fixture inproc_fixture.cpp)
target_compile_definitions(rhd_inproc_fixture PRIVATE UNICODE _UNICODE WIN32_LEAN_AND_MEAN _WIN32_WINNT=0x0601)
target_compile_options(rhd_inproc_fixture PRIVATE /utf-8)

add_executable(rhd_inproc_probe inproc_probe.cpp)
target_compile_definitions(rhd_inproc_probe PRIVATE UNICODE _UNICODE WIN32_LEAN_AND_MEAN _WIN32_WINNT=0x0601)
target_compile_options(rhd_inproc_probe PRIVATE /utf-8)
add_dependencies(rhd_inproc_probe rhd_inproc rhd_inproc_fixture)

foreach(case IN ITEMS success preimage-mismatch version-timeout already-patched)
    add_test(NAME "inproc-${case}" COMMAND rhd_inproc_probe "${case}")
    set_tests_properties("inproc-${case}" PROPERTIES TIMEOUT 60)
endforeach()
