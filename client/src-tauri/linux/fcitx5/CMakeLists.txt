cmake_minimum_required(VERSION 3.21)
project(fcitx5-sayitcommit VERSION 0.1.7 LANGUAGES CXX)

find_package(Fcitx5Core 5.1.13 REQUIRED)
include("${FCITX_INSTALL_CMAKECONFIG_DIR}/Fcitx5Utils/Fcitx5CompilerSettings.cmake")

add_library(sayitcommit SHARED sayitcommit.cpp)
target_compile_features(sayitcommit PRIVATE cxx_std_17)
target_link_libraries(sayitcommit PRIVATE Fcitx5::Core)

install(TARGETS sayitcommit LIBRARY DESTINATION "${FCITX_INSTALL_ADDONDIR}")
install(
  FILES sayitcommit.conf
  DESTINATION "${FCITX_INSTALL_PKGDATADIR}/addon"
)
