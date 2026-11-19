# libprojectM's build script links the static library from
# <cmake prefix>/lib, but GNUInstallDirs installs it to lib64 on Fedora and
# other RPM-based distributions. Pin the install directory to the one the
# crate reads. CMake reads this file from CMAKE_TOOLCHAIN_FILE, which
# .cargo/config.toml sets for cargo builds.
set(CMAKE_INSTALL_LIBDIR "lib" CACHE PATH "Install path for libraries" FORCE)
